//! Bounded read/apply snapshots for numeric editors. Tokens are equality evidence only, never dereference authority.
use crate::protocol::{ProtocolError, Reader};

pub const MAX_FIELDS: usize = 4096;
pub const MAX_BYTES: usize = 768 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FieldSet {
    Currency = 0,
    Keys = 1,
    WovenEchoes = 2,
    PointsEarned = 3,
    MonolithRun = 4,
    FactionStanding = 5,
}
impl FieldSet {
    pub const COUNT: usize = 6;
    pub const ALL: [Self; Self::COUNT] = [Self::Currency, Self::Keys, Self::WovenEchoes, Self::PointsEarned, Self::MonolithRun, Self::FactionStanding];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Currency => "currency",
            Self::Keys => "keys",
            Self::WovenEchoes => "woven-echoes",
            Self::PointsEarned => "points-earned",
            Self::MonolithRun => "monolith-run",
            Self::FactionStanding => "faction-standing",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|set| set.id() == id)
    }
    pub(crate) fn decode(value: u8) -> Result<Self, ProtocolError> {
        Self::ALL.get(value as usize).copied().ok_or(ProtocolError::InvalidData)
    }
    pub const fn held_items(self) -> bool {
        matches!(self, Self::Keys | Self::WovenEchoes)
    }
    pub const fn field_count(self) -> Option<usize> {
        match self {
            Self::Currency | Self::MonolithRun => Some(3),
            Self::PointsEarned => Some(2),
            Self::FactionStanding => Some(12),
            _ => None,
        }
    }
    pub fn takes(self, index: usize, value: i32) -> bool {
        let maximum = match self {
            Self::PointsEarned => i32::from(u16::MAX),
            Self::MonolithRun if index == 1 => 10_000,
            Self::MonolithRun if index == 2 => 1,
            Self::FactionStanding if index % 3 == 1 => 999_999,
            _ => i32::MAX,
        };
        value >= 0 && value <= maximum && self.field_count().is_none_or(|count| index < count)
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
        if self.manager == 0 || self.container == 0 || self.fields.len() > MAX_FIELDS || self.set.field_count().is_some_and(|count| self.fields.len() != count) {
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
        if self.set.held_items() {
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
        if self.values.len() != self.expected.fields.len()
            || self
                .values
                .iter()
                .enumerate()
                .any(|(index, value)| value.is_some_and(|value| !self.expected.set.takes(index, value)))
        {
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
    #[test]
    fn fixed_numeric_sets_keep_zero_rows_and_validate_each_requested_field_range() {
        for set in [FieldSet::PointsEarned, FieldSet::MonolithRun, FieldSet::FactionStanding] {
            let count = set.field_count().unwrap();
            let expected = Snapshot {
                set,
                manager: 1,
                container: 2,
                fields: (0..count)
                    .map(|index| Field {
                        key: index as u64,
                        identity: 10 + index as u64,
                        item_type: 0,
                        sub_type: 0,
                        label: String::new(),
                        value: 1,
                    })
                    .collect(),
            };
            assert_eq!(Snapshot::decode(&expected.encode().unwrap()).unwrap(), expected);
            let edit = Edit {
                expected: expected.clone(),
                values: vec![Some(0); count],
            };
            assert!(edit.validate().is_ok());
            let mut actual = expected.clone();
            for field in &mut actual.fields {
                field.value = 0;
            }
            assert!(actual.keeps(&expected, &edit.values));
            actual.fields.pop();
            assert!(!actual.keeps(&expected, &edit.values));
            assert!(actual.encode().is_err());
            let (index, bad) = match set {
                FieldSet::PointsEarned => (0, 65536),
                FieldSet::MonolithRun => (2, 2),
                _ => (1, 1_000_000),
            };
            let mut invalid = edit;
            invalid.values[index] = Some(bad);
            assert!(invalid.validate().is_err());
        }
        assert!(FieldSet::MonolithRun.takes(1, 10_000));
        assert!(!FieldSet::MonolithRun.takes(1, 10_001));
        assert!(FieldSet::FactionStanding.takes(1, 999_999));
    }
}
