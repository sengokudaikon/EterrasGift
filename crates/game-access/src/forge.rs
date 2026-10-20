//! Owned read-only forge snapshots, introduced in protocol 22 operation 18.
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

use crate::protocol::{MAX_RESPONSE_SIZE, ProtocolError};

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
    /// Matches `native/item.h::fingerprint`: each value is an LE int32 mixed
    /// through FNV-1a 64, including the one-based affix tier. Set identity (rarity
    /// 8) is transmitted but deliberately excluded, as in the original helper.
    /// Names, special affix type, class requirement and individual ID are also
    /// outside that legacy fingerprint. Encode/decode enforce structural bounds.
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

pub fn encode(snapshot: Option<&ForgeSnapshot>) -> Result<Vec<u8>, ProtocolError> {
    let Some(snapshot) = snapshot else { return Ok(vec![0]) };
    snapshot.validate()?;
    let mut out = vec![1];
    out.extend_from_slice(&snapshot.item.to_le_bytes());
    out.extend_from_slice(&snapshot.individual_id.to_le_bytes());
    out.extend_from_slice(&snapshot.fingerprint.to_le_bytes());
    text(&mut out, &snapshot.name);
    out.push(snapshot.item_type);
    out.extend_from_slice(&snapshot.sub_type.to_le_bytes());
    out.push(snapshot.rarity);
    out.extend_from_slice(&snapshot.class_requirement.to_le_bytes());
    out.push(snapshot.affixes.len() as u8);
    for affix in &snapshot.affixes {
        out.push(affix.slot);
        out.extend_from_slice(&affix.id.to_le_bytes());
        out.extend_from_slice(&affix.tier.to_le_bytes());
        out.push(affix.roll);
        out.extend_from_slice(&affix.kind.to_le_bytes());
        out.extend_from_slice(&affix.seal.to_le_bytes());
        text(&mut out, &affix.name);
        out.extend_from_slice(&affix.special.to_le_bytes());
    }
    out.push(snapshot.implicit_rolls.len() as u8);
    out.extend_from_slice(&snapshot.implicit_rolls);
    out.push(u8::from(snapshot.identity.is_some()));
    if let Some(identity) = &snapshot.identity {
        out.extend_from_slice(&identity.id.to_le_bytes());
        out.push(identity.rolls.len() as u8);
        out.extend_from_slice(&identity.rolls);
    }
    out.push(snapshot.status.forging_potential);
    out.extend_from_slice(&snapshot.status.forging_potential_type.to_le_bytes());
    out.extend_from_slice(&[
        snapshot.status.legendary_potential,
        snapshot.status.weavers_will,
        snapshot.status.weavers_touch,
        u8::from(snapshot.status.corrupted),
        u8::from(snapshot.status.ruined),
    ]);
    out.extend_from_slice(&snapshot.status.level_requirement.to_le_bytes());
    if out.len() > MAX_RESPONSE_SIZE {
        return Err(ProtocolError::InvalidData);
    }
    Ok(out)
}

pub fn decode(payload: &[u8]) -> Result<Option<ForgeSnapshot>, ProtocolError> {
    if payload.len() > MAX_RESPONSE_SIZE {
        return Err(ProtocolError::InvalidData);
    }
    let mut reader = Reader { remaining: payload };
    if !reader.boolean()? {
        reader.finish()?;
        return Ok(None);
    }
    let item = u64::from_le_bytes(reader.array()?);
    let individual_id = u32::from_le_bytes(reader.array()?);
    let fingerprint = u64::from_le_bytes(reader.array()?);
    let name = reader.text()?;
    let item_type = reader.byte()?;
    let sub_type = u16::from_le_bytes(reader.array()?);
    let rarity = reader.byte()?;
    let class_requirement = i32::from_le_bytes(reader.array()?);
    let count = reader.count(MAX_AFFIXES)?;
    let mut affixes = Vec::with_capacity(count);
    for _ in 0..count {
        affixes.push(AffixSnapshot {
            slot: reader.byte()?,
            id: u16::from_le_bytes(reader.array()?),
            tier: u16::from_le_bytes(reader.array()?),
            roll: reader.byte()?,
            kind: i32::from_le_bytes(reader.array()?),
            seal: i32::from_le_bytes(reader.array()?),
            name: reader.text()?,
            special: i32::from_le_bytes(reader.array()?),
        });
    }
    let count = reader.count(MAX_IMPLICIT_ROLLS)?;
    let implicit_rolls = reader.take(count)?.to_vec();
    let identity = if reader.boolean()? {
        let id = u16::from_le_bytes(reader.array()?);
        let count = reader.count(MAX_UNIQUE_ROLLS)?;
        Some(UniqueIdentity {
            id,
            rolls: reader.take(count)?.to_vec(),
        })
    } else {
        None
    };
    let status = ItemStatus {
        forging_potential: reader.byte()?,
        forging_potential_type: i32::from_le_bytes(reader.array()?),
        legendary_potential: reader.byte()?,
        weavers_will: reader.byte()?,
        weavers_touch: reader.byte()?,
        corrupted: reader.boolean()?,
        ruined: reader.boolean()?,
        level_requirement: i32::from_le_bytes(reader.array()?),
    };
    reader.finish()?;
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

fn text(out: &mut Vec<u8>, value: &str) {
    // The enclosing snapshot was already validated against the 4096-byte limit.
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ProtocolError> {
        let (head, tail) = self.remaining.split_at_checked(count).ok_or(ProtocolError::InvalidData)?;
        self.remaining = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        self.take(N)?.try_into().map_err(|_| ProtocolError::InvalidData)
    }

    fn byte(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.array::<1>()?[0])
    }

    fn boolean(&mut self) -> Result<bool, ProtocolError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProtocolError::InvalidData),
        }
    }

    fn count(&mut self, limit: usize) -> Result<usize, ProtocolError> {
        let count = usize::from(self.byte()?);
        if count > limit { Err(ProtocolError::InvalidData) } else { Ok(count) }
    }

    fn text(&mut self) -> Result<String, ProtocolError> {
        let length = usize::from(u16::from_le_bytes(self.array()?));
        if length > MAX_NAME_BYTES {
            return Err(ProtocolError::InvalidData);
        }
        std::str::from_utf8(self.take(length)?).map(str::to_owned).map_err(|_| ProtocolError::InvalidData)
    }

    fn finish(self) -> Result<(), ProtocolError> {
        if self.remaining.is_empty() { Ok(()) } else { Err(ProtocolError::InvalidData) }
    }
}
