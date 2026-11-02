//! Bounded read/apply snapshots for count editors. Tokens are equality evidence only, never dereference authority.
use crate::protocol::{ProtocolError, Reader};

pub const MAX_FIELDS: usize = 4096;
pub const MAX_BYTES: usize = 768 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FieldSet {
    Currency = 0,
    Keys = 1,
    WovenEchoes = 2,
}
impl FieldSet {
    pub const ALL: [Self; 3] = [Self::Currency, Self::Keys, Self::WovenEchoes];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Currency => "currency",
            Self::Keys => "keys",
            Self::WovenEchoes => "woven-echoes",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|set| set.id() == id)
    }
    pub(crate) fn decode(value: u8) -> Result<Self, ProtocolError> {
        Self::ALL.get(value as usize).copied().ok_or(ProtocolError::InvalidData)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub key: u64,
    pub identity: u64,
    pub item_type: u16,
    pub sub_type: u16,
    pub label: String,
    pub value: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub set: FieldSet,
    pub manager: u64,
    pub container: u64,
    pub fields: Vec<Field>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub expected: Snapshot,
    pub values: Vec<Option<i32>>,
}

impl Snapshot {
    /// Names are presentation, not stale-write authority: a locale change or removal of a same-named entry may
    /// relabel a surviving row. Subject tokens/types/order and every quantity still have to agree.
    pub fn same_values(&self, other: &Self) -> bool {
        (self.set, self.manager, self.container) == (other.set, other.manager, other.container)
            && self.fields.len() == other.fields.len()
            && self
                .fields
                .iter()
                .zip(&other.fields)
                .all(|(a, b)| (a.key, a.identity, a.item_type, a.sub_type, a.value) == (b.key, b.identity, b.item_type, b.sub_type, b.value))
    }
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.manager == 0 || self.container == 0 || self.fields.len() > MAX_FIELDS || (self.set == FieldSet::Currency && self.fields.len() != 3) {
            return Err(ProtocolError::InvalidData);
        }
        let mut bytes = vec![self.set as u8];
        bytes.extend(self.manager.to_le_bytes());
        bytes.extend(self.container.to_le_bytes());
        bytes.extend((self.fields.len() as u32).to_le_bytes());
        let mut seen = std::collections::BTreeSet::new();
        for field in &self.fields {
            if field.value < 0 || field.label.len() > 512 || !seen.insert(field.key) {
                return Err(ProtocolError::InvalidData);
            }
            bytes.extend(field.key.to_le_bytes());
            bytes.extend(field.identity.to_le_bytes());
            bytes.extend(field.item_type.to_le_bytes());
            bytes.extend(field.sub_type.to_le_bytes());
            bytes.extend(field.value.to_le_bytes());
            bytes.extend((field.label.len() as u32).to_le_bytes());
            bytes.extend(field.label.as_bytes());
        }
        if bytes.len() > MAX_BYTES {
            return Err(ProtocolError::InvalidData);
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_BYTES {
            return Err(ProtocolError::InvalidData);
        }
        let mut r = Reader::new(bytes);
        let set = FieldSet::decode(r.byte()?)?;
        let manager = r.u64()?;
        let container = r.u64()?;
        let count = r.u32()? as usize;
        if count > MAX_FIELDS {
            return Err(ProtocolError::InvalidData);
        }
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            let key = r.u64()?;
            let identity = r.u64()?;
            let item_type = u16::from_le_bytes(r.array()?);
            let sub_type = u16::from_le_bytes(r.array()?);
            let value = i32::from_le_bytes(r.array()?);
            let length = r.u32()? as usize;
            if length > 512 {
                return Err(ProtocolError::InvalidData);
            }
            let label = std::str::from_utf8(r.take(length)?).map_err(|_| ProtocolError::InvalidData)?.to_owned();
            fields.push(Field {
                key,
                identity,
                item_type,
                sub_type,
                label,
                value,
            });
        }
        r.finish()?;
        let snapshot = Self { set, manager, container, fields };
        snapshot.encode()?;
        Ok(snapshot)
    }
    /// The same container/subjects, with only requested quantities changed or zeroed held entries removed.
    pub fn keeps(&self, expected: &Self, values: &[Option<i32>]) -> bool {
        if (self.set, self.manager, self.container) != (expected.set, expected.manager, expected.container) || values.len() != expected.fields.len() {
            return false;
        }
        let mut wanted = expected.fields.clone();
        for (field, value) in wanted.iter_mut().zip(values) {
            if let Some(value) = value {
                field.value = *value;
            }
        }
        if self.set != FieldSet::Currency {
            wanted.retain(|field| field.value != 0);
        }
        self.same_values(&Self {
            fields: wanted,
            ..expected.clone()
        })
    }
}
impl Edit {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.expected.encode()?;
        if self.values.len() != self.expected.fields.len() || self.values.iter().flatten().any(|value| *value < 0) {
            return Err(ProtocolError::InvalidData);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DispatcherSpec, Request};
    fn snapshot() -> Snapshot {
        Snapshot {
            set: FieldSet::Keys,
            manager: 1,
            container: 2,
            fields: (0..2)
                .map(|index| Field {
                    key: index + 3,
                    identity: index + 10,
                    item_type: 104,
                    sub_type: index as u16,
                    label: format!("Same #{}", index),
                    value: 20,
                })
                .collect(),
        }
    }
    #[test]
    fn count_snapshot_and_conditional_edit_roundtrip_without_accepting_malformed_counts() {
        let snapshot = snapshot();
        let bytes = snapshot.encode().unwrap();
        assert_eq!(Snapshot::decode(&bytes).unwrap(), snapshot);
        for end in 0..bytes.len() {
            assert!(Snapshot::decode(&bytes[..end]).is_err());
        }
        let dispatcher = DispatcherSpec { rva: 1, bytes: [0x90; 16] };
        for request in [
            Request::ReadFields {
                dispatcher,
                expected_manager: 1,
                set: FieldSet::Keys,
            },
            Request::WriteFields {
                dispatcher,
                edit: Box::new(Edit {
                    expected: snapshot.clone(),
                    values: vec![None, Some(0)],
                }),
            },
        ] {
            let bytes = request.encode();
            assert_eq!(Request::decode(&bytes).unwrap(), request);
        }
        for values in [vec![Some(-1), None], vec![Some(2)], vec![]] {
            assert!(
                Edit {
                    expected: snapshot.clone(),
                    values
                }
                .validate()
                .is_err()
            );
        }
        let mut invalid = snapshot.clone();
        invalid.fields[1].key = invalid.fields[0].key;
        assert!(invalid.encode().is_err());
        invalid = snapshot;
        invalid.fields[0].value = -1;
        assert!(invalid.encode().is_err());
    }
    #[test]
    fn removal_and_readback_follow_subject_identity_even_when_a_survivors_name_changes() {
        let before = snapshot();
        let mut after = before.clone();
        after.fields.remove(0);
        after.fields[0].label = "Same".into();
        after.fields[0].value = 40;
        assert!(after.keeps(&before, &[Some(0), Some(40)]));
        after.fields[0].identity += 1;
        assert!(!after.keeps(&before, &[Some(0), Some(40)]));
        after.fields[0].identity -= 1;
        after.container += 1;
        assert!(!after.keeps(&before, &[Some(0), Some(40)]));
    }
}
