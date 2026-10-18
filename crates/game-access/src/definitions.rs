//! Literal item limits captured from game metadata; this is not a complete catalog.
//!
//! The payload is exactly 40 bytes: five (minimum, maximum) pairs of LE int32,
//! ordered forging potential, legendary potential, Weaver's Will, Weaver's Touch,
//! and level requirement. Values must fit a byte and minimum must not exceed
//! maximum. This intentionally excludes locale, game version and rarity names.

use crate::protocol::ProtocolError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueRange {
    pub minimum: u8,
    pub maximum: u8,
}

impl ValueRange {
    pub const fn contains(&self, value: u8) -> bool {
        self.minimum <= value && value <= self.maximum
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemLimits {
    pub forging_potential: ValueRange,
    pub legendary_potential: ValueRange,
    pub weavers_will: ValueRange,
    pub weavers_touch: ValueRange,
    pub level_requirement: ValueRange,
}

pub fn encode(limits: &ItemLimits) -> Result<Vec<u8>, ProtocolError> {
    let mut out = Vec::with_capacity(40);
    for range in [
        limits.forging_potential,
        limits.legendary_potential,
        limits.weavers_will,
        limits.weavers_touch,
        limits.level_requirement,
    ] {
        if range.minimum > range.maximum {
            return Err(ProtocolError::InvalidData);
        }
        out.extend_from_slice(&i32::from(range.minimum).to_le_bytes());
        out.extend_from_slice(&i32::from(range.maximum).to_le_bytes());
    }
    Ok(out)
}

pub fn decode(payload: &[u8]) -> Result<ItemLimits, ProtocolError> {
    let pairs: &[u8; 40] = payload.try_into().map_err(|_| ProtocolError::InvalidData)?;
    let mut ranges = [ValueRange { minimum: 0, maximum: 0 }; 5];
    for (range, pair) in ranges.iter_mut().zip(pairs.as_chunks::<8>().0) {
        let (minimum, maximum) = pair.split_at(4);
        let minimum = i32::from_le_bytes(minimum.try_into().map_err(|_| ProtocolError::InvalidData)?);
        let maximum = i32::from_le_bytes(maximum.try_into().map_err(|_| ProtocolError::InvalidData)?);
        *range = ValueRange {
            minimum: minimum.try_into().map_err(|_| ProtocolError::InvalidData)?,
            maximum: maximum.try_into().map_err(|_| ProtocolError::InvalidData)?,
        };
        if range.minimum > range.maximum {
            return Err(ProtocolError::InvalidData);
        }
    }
    let [forging_potential, legendary_potential, weavers_will, weavers_touch, level_requirement] = ranges;
    Ok(ItemLimits {
        forging_potential,
        legendary_potential,
        weavers_will,
        weavers_touch,
        level_requirement,
    })
}
