//! Bounded read/apply snapshots for numeric editors. Tokens are equality evidence only, never dereference authority.
use crate::{
    codec::{Reader, Wire, Writer, invalid},
    protocol::ProtocolError,
};

pub const MAX_FIELDS: usize = 4096;
pub const MAX_BYTES: usize = 768 * 1024;
pub const MAX_LABEL_BYTES: usize = 512;

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

/// What a field set is: its ID, whether its entries are held items (a zero removes one), how many fields it has when
/// that is fixed, and the largest value each field takes. `largest` repeats: field `i` takes `largest[i % len]`.
pub struct FieldSetSpec {
    pub set: FieldSet,
    pub id: &'static str,
    pub held_items: bool,
    pub fields: Option<usize>,
    largest: &'static [i32],
}

const fn spec(set: FieldSet, id: &'static str, held_items: bool, fields: Option<usize>, largest: &'static [i32]) -> FieldSetSpec {
    FieldSetSpec {
        set,
        id,
        held_items,
        fields,
        largest,
    }
}

/// Every field set, in the enum's order.
pub const SPECS: [FieldSetSpec; FieldSet::COUNT] = {
    use FieldSet::*;
    const ANY: &[i32] = &[i32::MAX];
    [
        spec(Currency, "currency", false, Some(3), ANY),
        spec(Keys, "keys", true, None, ANY),
        spec(WovenEchoes, "woven-echoes", true, None, ANY),
        spec(PointsEarned, "points-earned", false, Some(2), &[u16::MAX as i32]),
        // Corruption, the Gaze of Orobyss, and whether the echo options were rerolled.
        spec(MonolithRun, "monolith-run", false, Some(3), &[i32::MAX, 10_000, 1]),
        // Rank, favor and reputation for each of the four factions.
        spec(FactionStanding, "faction-standing", false, Some(12), &[i32::MAX, 999_999, i32::MAX]),
    ]
};

impl FieldSet {
    pub const COUNT: usize = 6;
    /// Every set by its number on the pipe, checked against [`SPECS`] when the crate builds.
    pub const ALL: [Self; Self::COUNT] = {
        let mut all = [Self::Currency; Self::COUNT];
        let mut index = 0;
        while index < Self::COUNT {
            assert!(SPECS[index].set as usize == index, "SPECS follows the enum's order");
            all[index] = SPECS[index].set;
            index += 1;
        }
        all
    };
    pub const fn spec(self) -> &'static FieldSetSpec {
        &SPECS[self as usize]
    }
    pub const fn id(self) -> &'static str {
        self.spec().id
    }
    pub fn from_id(id: &str) -> Option<Self> {
        SPECS.iter().find(|spec| spec.id == id).map(|spec| spec.set)
    }
    pub(crate) fn decode(value: u8) -> Result<Self, ProtocolError> {
        Self::ALL.get(value as usize).copied().ok_or(ProtocolError::InvalidData)
    }
    pub const fn held_items(self) -> bool {
        self.spec().held_items
    }
    pub const fn field_count(self) -> Option<usize> {
        self.spec().fields
    }
    /// Whether field `index` takes `value`: none is negative, and none is past the set's fields or its largest value.
    pub fn takes(self, index: usize, value: i32) -> bool {
        let spec = self.spec();
        value >= 0 && value <= spec.largest[index % spec.largest.len()] && spec.fields.is_none_or(|count| index < count)
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
            return invalid();
        }
        let mut w = Writer::default();
        w.byte(self.set as u8);
        w.u64(self.manager);
        w.u64(self.container);
        w.u32(self.fields.len() as u32);
        let mut seen = std::collections::BTreeSet::new();
        for field in &self.fields {
            if field.value < 0 || !seen.insert(field.key) {
                return invalid();
            }
            w.u64(field.key);
            w.u64(field.identity);
            w.u16(field.item_type);
            w.u16(field.sub_type);
            w.i32(field.value);
            w.text_within(&field.label, MAX_LABEL_BYTES)?;
        }
        if w.0.len() > MAX_BYTES {
            return invalid();
        }
        Ok(w.0)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_BYTES {
            return invalid();
        }
        let mut r = Reader::new(bytes);
        let set = FieldSet::decode(r.byte()?)?;
        let manager = r.u64()?;
        let container = r.u64()?;
        let count = r.count32(MAX_FIELDS)?;
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            fields.push(Field {
                key: r.u64()?,
                identity: r.u64()?,
                item_type: r.u16()?,
                sub_type: r.u16()?,
                value: r.i32()?,
                label: r.text(MAX_LABEL_BYTES)?.to_owned(),
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

/// An edit as a request carries it: the expected snapshot's encoding behind its length, then each field's value behind
/// a flag that says whether it changes.
impl Wire for Edit {
    fn write(&self, w: &mut Writer) {
        w.blob(&self.expected.encode().expect("validated field edit"));
        w.u32(self.values.len() as u32);
        for value in &self.values {
            w.flag(value.is_some());
            if let Some(value) = value {
                w.i32(*value);
            }
        }
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let expected = Snapshot::decode(r.blob(MAX_BYTES)?)?;
        if r.u32()? as usize != expected.fields.len() {
            return invalid();
        }
        let values = (0..expected.fields.len())
            .map(|_| if r.flag()? { r.i32().map(Some) } else { Ok(None) })
            .collect::<Result<_, _>>()?;
        let edit = Self { expected, values };
        edit.validate()?;
        Ok(edit)
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
