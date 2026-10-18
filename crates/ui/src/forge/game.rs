//! What the forge asks of the game: the item, the captured game data, value tables, writes and the unique preview. The
//! trainer's session will answer it (TODO(forge)); until then the real app has no game and shows the connection's empty
//! state, and captures and tests answer it with a fixture.

use super::item::{AffixDefinition, ForgeItem, ItemEdit, ModifierState, RollRequest, ValueTable};

/// A base of an item type, from the captured item list.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseDefinition {
    pub sub_type: u16,
    pub name: String,
    pub level: u8,
    pub corrupted: bool,
    /// The implicits it grants, in the game's words with their ranges ("+10–16 Armor").
    pub implicits: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemTypeDefinition {
    pub item_type: u16,
    pub name: String,
    pub bases: Vec<BaseDefinition>,
}

/// The game's own ranges for item-level values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemLimits {
    pub forging_potential: (u8, u8),
    pub legendary_potential: (u8, u8),
    pub weavers_will: (u8, u8),
    pub weavers_touch: (u8, u8),
    pub level_requirement: (u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendaryType {
    LegendaryPotential,
    WeaversWill,
}

/// A unique or set item from the captured data.
#[derive(Clone, Debug, PartialEq)]
pub struct UniqueDefinition {
    pub id: u16,
    pub name: String,
    pub item_type: u16,
    /// The bases it comes on.
    pub sub_types: Vec<u16>,
    pub legendary_type: LegendaryType,
    pub is_set: bool,
    pub can_drop_as_legendary: bool,
}

/// The captured game data the forge reads.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub uniques: Vec<UniqueDefinition>,
    pub definitions: Vec<AffixDefinition>,
    /// Formatted tier ranges by (affix id, tier); a missing one is left blank, never guessed.
    pub tier_ranges: std::collections::HashMap<(u16, u8), String>,
    /// The game's rarity names by rarity byte; empty entries fall back to the app's.
    pub rarity_names: Vec<String>,
    pub limits: Option<ItemLimits>,
    pub item_types: Vec<ItemTypeDefinition>,
}

impl Catalog {
    pub fn definition(&self, id: u16) -> AffixDefinition {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
            .cloned()
            .unwrap_or_else(|| AffixDefinition::unknown(id))
    }

    pub fn tier_range(&self, id: u16, tier: u8) -> String {
        self.tier_ranges.get(&(id, tier)).cloned().unwrap_or_default()
    }

    pub fn rarity_name(&self, rarity: u8) -> Option<String> {
        self.rarity_names.get(rarity as usize).filter(|name| !name.is_empty()).cloned()
    }

    pub fn unique(&self, id: u16) -> Option<&UniqueDefinition> {
        self.uniques.iter().find(|unique| unique.id == id)
    }

    pub fn item_type(&self, item_type: u16) -> Option<&ItemTypeDefinition> {
        self.item_types.iter().find(|definition| definition.item_type == item_type)
    }

    pub fn base(&self, item_type: u16, sub_type: u16) -> Option<&BaseDefinition> {
        self.item_type(item_type)?.bases.iter().find(|base| base.sub_type == sub_type)
    }
}

/// A failed request, in the session's words.
pub type GameError = String;

pub trait ForgeGame {
    fn catalog(&self) -> &Catalog;

    /// The item in the forge now, or None when the forge is empty.
    fn read(&self) -> Option<ForgeItem>;

    /// The session is ready and can write (an offline character, nothing in flight).
    fn can_write(&self) -> bool {
        true
    }

    /// "I know what I'm doing" in Settings.
    fn unrestricted(&self) -> bool {
        false
    }

    /// The values a roll takes in its context (spec §6 Mapping). The editor never computes values itself.
    fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> Result<ValueTable, GameError>;

    /// Writes one edit to the item the draft was made on and reads it back (spec §11).
    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> Result<ForgeItem, GameError>;

    /// The unique modifiers formatted with drafted rolls.
    fn preview_unique(&self, expected: &ForgeItem, edit: &ItemEdit) -> Result<Vec<ModifierState>, GameError>;
}
