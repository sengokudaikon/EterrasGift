//! Owned read-only forge snapshots.
//!
//! All integers are little endian. An empty forge is the single byte 0. A present
//! item starts with 1, then item:u64, individual_id:u32, fingerprint:u64, name:text,
//! item_type:u8, sub_type:u16, rarity:u8, class_requirement:i32, affix_count:u8,
//! affixes, implicit_count:u8 and its bytes, identity_present:u8 and optional
//! identity, then status. Text is u16 byte length and strict UTF-8 (empty allowed).
//! An affix is slot:u8, id:u16, display tier:u16, roll:u8, kind:i32, seal:i32,
//! name:text, special:i32. An identity is id:u16, roll_count:u8 and roll bytes.
//! Status is forging_potential:u8, forging_potential_type:i32,
//! legendary_potential:u8, weavers_will:u8, weavers_touch:u8, corrupted:u8,
//! ruined:u8, level_requirement:i32. Presence and boolean bytes are exactly 0/1.
//!
//! No process pointer is dereferenced here. `item` is an opaque observation token.
//! Unknown enum values remain signed integers; interpreting them belongs to the
//! domain layer. The legacy fingerprint detects stale observations, not forgery.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Wire, Writer},
    protocol::ProtocolError,
};

/// The most implicits an item carries: the helper's bound on what it reads, the desktop's on what it keeps.
pub const MAX_IMPLICITS: usize = 8;

pub const MAX_AFFIXES: usize = 32;
pub const MAX_IMPLICIT_ROLLS: usize = 3;
pub const MAX_UNIQUE_ROLLS: usize = 8;
pub const MAX_NAME_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeSnapshot {
    pub item: u64,
    pub individual_id: u32,
    pub fingerprint: u64,
    pub name: String,
    pub item_type: u8,
    pub sub_type: u16,
    pub rarity: u8,
    pub class_requirement: i32,
    pub affixes: Vec<AffixSnapshot>,
    pub implicit_rolls: Vec<u8>,
    pub identity: Option<UniqueIdentity>,
    pub status: ItemStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixSnapshot {
    pub slot: u8,
    pub id: u16,
    /// One-based display tier; the runtime stores this value minus one as a byte.
    pub tier: u16,
    pub roll: u8,
    pub kind: i32,
    pub seal: i32,
    pub name: String,
    pub special: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueIdentity {
    pub id: u16,
    pub rolls: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStatus {
    pub forging_potential: u8,
    pub forging_potential_type: i32,
    pub legendary_potential: u8,
    pub weavers_will: u8,
    pub weavers_touch: u8,
    pub corrupted: bool,
    pub ruined: bool,
    pub level_requirement: i32,
}

impl ForgeSnapshot {
    /// The helper's item fingerprint: each value is an LE int32 mixed through
    /// FNV-1a 64, including the one-based affix tier. Set identity (rarity 8) is
    /// transmitted but excluded, as are names, special affix type, class
    /// requirement and individual ID. Encode/decode enforce structural bounds.
    pub fn legacy_fingerprint(&self) -> u64 {
        let mut hash = 14_695_981_039_346_656_037_u64;
        let mut mix = |value: i32| {
            for byte in value.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211);
            }
        };
        mix(i32::from(self.item_type));
        mix(self.affixes.len() as i32);
        for affix in &self.affixes {
            mix(i32::from(affix.id));
            mix(i32::from(affix.tier));
            mix(i32::from(affix.roll));
            mix(affix.kind);
            mix(affix.seal);
        }
        mix(i32::from(self.sub_type));
        mix(i32::from(self.rarity));
        let identity = self.identity.as_ref().filter(|_| matches!(self.rarity, 7 | 9));
        mix(identity.map_or(0, |identity| i32::from(identity.id)));
        mix(identity.map_or(0, |identity| identity.rolls.len() as i32));
        if let Some(identity) = identity {
            for roll in &identity.rolls {
                mix(i32::from(*roll));
            }
        }
        mix(self.implicit_rolls.len() as i32);
        for roll in &self.implicit_rolls {
            mix(i32::from(*roll));
        }
        mix(i32::from(self.status.forging_potential));
        mix(self.status.forging_potential_type);
        mix(i32::from(self.status.legendary_potential));
        mix(i32::from(self.status.weavers_will));
        mix(i32::from(self.status.weavers_touch));
        mix(i32::from(self.status.corrupted));
        mix(i32::from(self.status.ruined));
        mix(self.status.level_requirement);
        hash
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if self.item == 0
            || self.name.len() > MAX_NAME_BYTES
            || self.affixes.len() > MAX_AFFIXES
            || self.implicit_rolls.len() > MAX_IMPLICIT_ROLLS
            || self.identity.is_some() != matches!(self.rarity, 7..=9)
            || self.identity.as_ref().is_some_and(|identity| identity.rolls.len() > MAX_UNIQUE_ROLLS)
            || self
                .affixes
                .iter()
                .enumerate()
                .any(|(slot, affix)| usize::from(affix.slot) != slot || !(1..=256).contains(&affix.tier) || affix.name.len() > MAX_NAME_BYTES)
            || self.fingerprint != self.legacy_fingerprint()
        {
            return Err(ProtocolError::InvalidData);
        }
        Ok(())
    }
}

/// Immutable request snapshot, validated once when constructed or decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckedSnapshot {
    value: Box<ForgeSnapshot>,
    bytes: Vec<u8>,
}
impl std::ops::Deref for CheckedSnapshot {
    type Target = ForgeSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
impl CheckedSnapshot {
    fn check_length(length: usize) -> Result<(), ProtocolError> {
        if !(crate::write::MIN_EXPECTED_BYTES..=crate::write::MAX_EXPECTED_BYTES).contains(&length) {
            return Err(ProtocolError::InvalidData);
        }
        Ok(())
    }
    pub(crate) fn new(value: ForgeSnapshot) -> Result<Self, ProtocolError> {
        let bytes = encode(Some(&value))?;
        Self::check_length(bytes.len())?;
        Ok(Self { value: Box::new(value), bytes })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Self::check_length(bytes.len())?;
        let value = decode(bytes)?.ok_or(ProtocolError::InvalidData)?;
        Ok(Self {
            value: Box::new(value),
            bytes: bytes.to_vec(),
        })
    }
}

/// The item as a request expects it: its encoding behind a u32 length.
impl Wire for CheckedSnapshot {
    fn write(&self, w: &mut Writer) {
        w.blob(&self.bytes);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        Self::decode(r.blob(crate::write::MAX_EXPECTED_BYTES)?)
    }
}

pub fn encode(snapshot: Option<&ForgeSnapshot>) -> Result<Vec<u8>, ProtocolError> {
    let mut w = Writer::default();
    let Some(snapshot) = snapshot else {
        w.flag(false);
        return w.finish();
    };
    // Validation bounds every text to fit its u16 length.
    snapshot.validate()?;
    w.flag(true);
    w.u64(snapshot.item);
    w.u32(snapshot.individual_id);
    w.u64(snapshot.fingerprint);
    w.short_text(&snapshot.name);
    w.byte(snapshot.item_type);
    w.u16(snapshot.sub_type);
    w.byte(snapshot.rarity);
    w.i32(snapshot.class_requirement);
    w.byte(snapshot.affixes.len() as u8);
    for affix in &snapshot.affixes {
        w.byte(affix.slot);
        w.u16(affix.id);
        w.u16(affix.tier);
        w.byte(affix.roll);
        w.i32(affix.kind);
        w.i32(affix.seal);
        w.short_text(&affix.name);
        w.i32(affix.special);
    }
    w.byte(snapshot.implicit_rolls.len() as u8);
    w.raw(&snapshot.implicit_rolls);
    w.flag(snapshot.identity.is_some());
    if let Some(identity) = &snapshot.identity {
        w.u16(identity.id);
        w.byte(identity.rolls.len() as u8);
        w.raw(&identity.rolls);
    }
    let status = &snapshot.status;
    w.byte(status.forging_potential);
    w.i32(status.forging_potential_type);
    w.byte(status.legendary_potential);
    w.byte(status.weavers_will);
    w.byte(status.weavers_touch);
    w.flag(status.corrupted);
    w.flag(status.ruined);
    w.i32(status.level_requirement);
    w.finish()
}

pub fn decode(payload: &[u8]) -> Result<Option<ForgeSnapshot>, ProtocolError> {
    let mut r = Reader::reply(payload)?;
    if !r.flag()? {
        r.finish()?;
        return Ok(None);
    }
    let item = r.u64()?;
    let individual_id = r.u32()?;
    let fingerprint = r.u64()?;
    let name = r.short_text(MAX_NAME_BYTES)?.to_owned();
    let item_type = r.byte()?;
    let sub_type = r.u16()?;
    let rarity = r.byte()?;
    let class_requirement = r.i32()?;
    let count = r.count(MAX_AFFIXES)?;
    let mut affixes = Vec::with_capacity(count);
    for _ in 0..count {
        affixes.push(AffixSnapshot {
            slot: r.byte()?,
            id: r.u16()?,
            tier: r.u16()?,
            roll: r.byte()?,
            kind: r.i32()?,
            seal: r.i32()?,
            name: r.short_text(MAX_NAME_BYTES)?.to_owned(),
            special: r.i32()?,
        });
    }
    let count = r.count(MAX_IMPLICIT_ROLLS)?;
    let implicit_rolls = r.take(count)?.to_vec();
    let identity = if r.flag()? {
        let id = r.u16()?;
        let count = r.count(MAX_UNIQUE_ROLLS)?;
        Some(UniqueIdentity {
            id,
            rolls: r.take(count)?.to_vec(),
        })
    } else {
        None
    };
    let status = ItemStatus {
        forging_potential: r.byte()?,
        forging_potential_type: r.i32()?,
        legendary_potential: r.byte()?,
        weavers_will: r.byte()?,
        weavers_touch: r.byte()?,
        corrupted: r.flag()?,
        ruined: r.flag()?,
        level_requirement: r.i32()?,
    };
    r.finish()?;
    let snapshot = ForgeSnapshot {
        item,
        individual_id,
        fingerprint,
        name,
        item_type,
        sub_type,
        rarity,
        class_requirement,
        affixes,
        implicit_rolls,
        identity,
        status,
    };
    snapshot.validate()?;
    Ok(Some(snapshot))
}
