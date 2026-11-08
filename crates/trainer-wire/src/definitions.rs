//! Literal item limits captured from game metadata; this is not a complete catalog.
//!
//! The payload is exactly 40 bytes: five (minimum, maximum) pairs of LE int32,
//! ordered forging potential, legendary potential, Weaver's Will, Weaver's Touch,
//! and level requirement. Values must fit a byte and minimum must not exceed
//! maximum. This intentionally excludes locale, game version and rarity names.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Writer, invalid},
    protocol::ProtocolError,
};

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
    let mut w = Writer::default();
    for range in limits.ranges() {
        if range.minimum > range.maximum {
            return invalid();
        }
        w.i32(i32::from(range.minimum));
        w.i32(i32::from(range.maximum));
    }
    Ok(w.0)
}

pub fn decode(payload: &[u8]) -> Result<ItemLimits, ProtocolError> {
    let mut r = Reader::new(payload);
    let mut ranges = [ValueRange { minimum: 0, maximum: 0 }; 5];
    for range in &mut ranges {
        let mut value = || u8::try_from(r.i32()?).map_err(|_| ProtocolError::InvalidData);
        *range = ValueRange {
            minimum: value()?,
            maximum: value()?,
        };
        if range.minimum > range.maximum {
            return invalid();
        }
    }
    r.finish()?;
    let [forging_potential, legendary_potential, weavers_will, weavers_touch, level_requirement] = ranges;
    Ok(ItemLimits {
        forging_potential,
        legendary_potential,
        weavers_will,
        weavers_touch,
        level_requirement,
    })
}

impl ItemLimits {
    /// Its five ranges, in the order they are written.
    fn ranges(&self) -> [ValueRange; 5] {
        [
            self.forging_potential,
            self.legendary_potential,
            self.weavers_will,
            self.weavers_touch,
            self.level_requirement,
        ]
    }
}
