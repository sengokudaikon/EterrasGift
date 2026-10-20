//! What the forge asks of the game: the item, the captured game data, value tables, writes and the unique preview. The
//! production adapter reads owned snapshots and submits commands to the worker.
//! Write outcomes are typed; accepted commands are polled without blocking the UI.

pub const UNIQUE_LOADING: &str = "Reading unique descriptions…";
pub const PREVIEW_LOADING: &str = "Updating preview…";

use super::item::{AffixDefinition, ForgeItem, ItemEdit, ModifierState, RollRequest, ValueTable};
pub use eg_trainer::writes::WriteOutcome;

/// An independently observed item can be undisplayable by this editor. That
/// presentation limitation never downgrades the completed write certainty.
pub type EditOutcome = WriteOutcome<Option<ForgeItem>>;

pub fn refused(message: impl Into<String>) -> EditOutcome {
    WriteOutcome::NotApplied {
        code: eg_trainer::writes::ResponseCode::Refused,
        message: message.into(),
    }
}

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

    /// Production and authored captures use the same class, category and idol rules.
    fn compatibility(&self, item: &ForgeItem, definition: &AffixDefinition, slot: Option<&super::item::AffixState>, unrestricted: bool) -> super::item::Compatibility {
        super::item::compatibility(item, definition, slot, unrestricted)
    }

    /// Persistent explanation when an observation provider offers no writes.
    fn read_only_reason(&self) -> Option<&str> {
        None
    }

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

    /// An already running worker read may finish before the next draft step.
    fn is_busy(&self) -> bool {
        false
    }

    /// The values a roll takes in its context (spec §6 Mapping). The editor never computes values itself.
    fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> Result<ValueTable, GameError>;

    /// Writes one edit to the item the draft was made on and reads it back (spec §11).
    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome;

    /// Start a new Apply history only when its first edit is confirmed.
    fn begin_batch(&mut self) -> Result<(), GameError> {
        Ok(())
    }
    fn undo_depth(&self) -> usize {
        0
    }
    fn undo_needs_override(&self) -> bool {
        false
    }
    fn undo_intent(&self) -> Option<ItemEdit> {
        None
    }
    /// Restore the latest canonical token, never a UI-authored reverse write.
    fn undo(&mut self, _: &ForgeItem) -> EditOutcome {
        refused("Undo is unavailable for this observation.")
    }

    /// Completion of the single accepted command. None means still pending.
    fn poll_write(&mut self) -> Option<EditOutcome> {
        None
    }

    fn supports(&self, _: &ItemEdit) -> bool {
        true
    }

    /// Discard an unsubmitted preview when its draft is cleared; accepted work still drains.
    fn clear_unique_preview(&self) {}
    /// The unique modifiers formatted with drafted rolls.
    fn preview_unique(&self, expected: &ForgeItem, edit: &ItemEdit) -> Result<Vec<ModifierState>, GameError>;
}
