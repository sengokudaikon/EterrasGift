//! Affix definitions the helper adds to the game's list while it is in the game: a unique's modifiers, offered as
//! affixes. The game saves an affix by ID alone, so an ID stays with its modifier for good (the desktop's ledger).
//!
//! The IDs lie above the game's own (1156 today, and they grow) and around the block Loom's ledger takes first (3000 up);
//! the game saves up to 4095. The name's marker is how the helper knows its own entries again, across its own unloads,
//! and it takes them all out before it leaves.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Wire, Writer, invalid},
    protocol::ProtocolError,
};

pub const ID_RANGES: [std::ops::RangeInclusive<u16>; 2] = [1500..=2999, 3200..=4095];
/// How every added definition's internal name starts.
pub const MARKER: &str = "EG ";
/// Every added affix has this many tiers, all alike: the forge's tiers 1 to 7 all hold it, and none means more.
pub const TIERS: usize = 7;
/// More than any unique's roll group needs; the game's own affixes stop at two.
pub const MAX_PROPERTIES: usize = 16;
pub const MAX_PER_REQUEST: usize = 8;
pub const MAX_TEXT: usize = 512;

pub fn is_added_id(id: u16) -> bool {
    ID_RANGES.iter().any(|range| range.contains(&id))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatLine {
    pub property: u8,
    pub special_tag: u8,
    pub tags: i32,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub minimum: f32,
    pub maximum: f32,
}
// `NewAffix::validate` refuses a nonfinite value, so equality is total on every line a request holds.
impl Eq for StatLine {}

#[derive(Clone, Debug, PartialEq)]
pub struct NewAffix {
    pub id: u16,
    /// The internal name: starts with [`MARKER`].
    pub name: String,
    /// What the game shows. Empty on a one-line affix: the game writes it from the stat.
    pub display_name: String,
    /// `standardAffixEffectModifier`: the item modifier its values are written for. On an item with modifier `m` the game
    /// shows a value times `(1 + m) / (1 + this)`.
    pub standard_effect: f32,
    pub properties: Vec<StatLine>,
}
// `NewAffix::validate` refuses a nonfinite value here too.
impl Eq for NewAffix {}

impl NewAffix {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let text = |text: &str| text.len() <= MAX_TEXT && !text.contains('\0');
        let valid = is_added_id(self.id)
            && self.name.starts_with(MARKER)
            && text(&self.name)
            && text(&self.display_name)
            && (1..=MAX_PROPERTIES).contains(&self.properties.len())
            && (!self.display_name.is_empty() || self.properties.len() == 1)
            && self.standard_effect.is_finite()
            && self.standard_effect > -1.0
            && self.properties.iter().all(|line| line.minimum.is_finite() && line.maximum.is_finite());
        if valid { Ok(()) } else { Err(ProtocolError::InvalidData) }
    }
}

/// An affix as a request carries it: its ID, two texts, the modifier's bits, then its lines behind their count.
impl Wire for NewAffix {
    fn write(&self, w: &mut Writer) {
        w.u16(self.id);
        w.text(&self.name);
        w.text(&self.display_name);
        w.f32(self.standard_effect);
        w.byte(self.properties.len() as u8);
        for line in &self.properties {
            w.byte(line.property);
            w.byte(line.special_tag);
            w.i32(line.tags);
            w.i32(line.extra_tag);
            w.i32(line.modifier_type);
            w.f32(line.minimum);
            w.f32(line.maximum);
        }
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let id = r.u16()?;
        let name = r.text(MAX_TEXT)?.to_owned();
        let display_name = r.text(MAX_TEXT)?.to_owned();
        let standard_effect = r.f32()?;
        let count = r.count(MAX_PROPERTIES)?;
        if count == 0 {
            return invalid();
        }
        let mut properties = Vec::with_capacity(count);
        for _ in 0..count {
            properties.push(StatLine {
                property: r.byte()?,
                special_tag: r.byte()?,
                tags: r.i32()?,
                extra_tag: r.i32()?,
                modifier_type: r.i32()?,
                minimum: r.f32()?,
                maximum: r.f32()?,
            });
        }
        let affix = Self {
            id,
            name,
            display_name,
            standard_effect,
            properties,
        };
        affix.validate()?;
        Ok(affix)
    }
}
