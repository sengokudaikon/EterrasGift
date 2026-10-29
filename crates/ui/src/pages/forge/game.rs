//! The game's data as the forge shows it (`Catalog` and its definitions) and the shapes of the game's answers: a lookup
//! that may still be on its way (`Lookup`) and a typed write outcome (`EditOutcome`). The game itself is
//! `worker::ForgeGame`.

use eg_core::phrase::Phrase;

pub const UNIQUE_LOADING: Phrase = Phrase::new("Reading unique descriptions…");
pub const PREVIEW_LOADING: Phrase = Phrase::new("Updating preview…");

/// Optional game data has an explicit pending state; display copy never controls readiness.
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup<T> {
    Ready(T),
    Loading,
    Unavailable(String),
}
impl<T> From<Result<T, String>> for Lookup<T> {
    fn from(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Self::Ready(value),
            Err(reason) => Self::Unavailable(reason),
        }
    }
}

use super::item::{AffixDefinition, ForgeItem};
pub use eg_trainer::write::WriteOutcome;

/// An independently observed item can be undisplayable by this editor. That
/// presentation limitation never downgrades the completed write certainty.
pub type EditOutcome = WriteOutcome<Option<ForgeItem>>;

pub fn refused(message: impl Into<String>) -> EditOutcome {
    EditOutcome::refused(message)
}

/// A base of an item type, from the captured item list.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseDefinition {
    pub sub_type: u16,
    pub name: String,
    pub level: u8,
    pub corrupted: bool,
    /// The classes an item on this base is for (the game's `ItemClassRequirements` bits, 0 for any class). A base change
    /// judges the affixes with it.
    pub class_requirement: i32,
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

/// The fallback shown when the game gave no text for an implicit.
pub const IMPLICIT_TEXT_UNAVAILABLE: Phrase = Phrase::new("Implicit (the game's text is unavailable)");

/// One base implicit as a line: the game's text at both ends of its range, joined as a range, or the fallback when the
/// game gave neither.
pub fn implicit_line(minimum: Option<&str>, maximum: Option<&str>) -> String {
    let text = modifier_range(minimum.unwrap_or_default(), maximum.unwrap_or_default());
    if text.is_empty() { IMPLICIT_TEXT_UNAVAILABLE.into() } else { text }
}

/// The game's text with the rolled number shown as its range, for example "+10–16 Armor" from "+10 Armor" and
/// "+16 Armor". Texts that differ in more than a number read "minimum – maximum".
pub fn modifier_range(minimum: &str, maximum: &str) -> String {
    if minimum == maximum || minimum.is_empty() || maximum.is_empty() {
        return if minimum.is_empty() { maximum } else { minimum }.to_owned();
    }
    let low: Vec<char> = minimum.chars().collect();
    let high: Vec<char> = maximum.chars().collect();
    let number_part = |c: char| c.is_numeric() || c == '.' || c == ',';
    let mut prefix = 0;
    while prefix < low.len() && prefix < high.len() && low[prefix] == high[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < low.len() - prefix && suffix < high.len() - prefix && low[low.len() - suffix - 1] == high[high.len() - suffix - 1] {
        suffix += 1;
    }
    while prefix > 0 && number_part(low[prefix - 1]) {
        prefix -= 1;
    }
    while suffix > 0 && number_part(low[low.len() - suffix]) {
        suffix -= 1;
    }
    let is_number = |text: &[char]| text.first().is_some_and(|c| c.is_numeric()) && text.last().is_some_and(|c| c.is_numeric()) && text.iter().all(|c| number_part(*c));
    let (from, to) = (&low[prefix..low.len() - suffix], &high[prefix..high.len() - suffix]);
    if !is_number(from) || !is_number(to) {
        return format!("{minimum} – {maximum}");
    }
    let text = |chars: &[char]| chars.iter().collect::<String>();
    format!("{}{}–{}{}", text(&low[..prefix]), text(from), text(to), text(&low[low.len() - suffix..]))
}

/// A failed request, in the session's words.
pub type GameError = String;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_lines_join_the_games_texts_as_a_range() {
        assert_eq!(modifier_range("+10 Armor", "+16 Armor"), "+10–16 Armor");
        assert_eq!(modifier_range("+12% Fire Resistance", "+18% Fire Resistance"), "+12–18% Fire Resistance");
        assert_eq!(modifier_range("+1.5 Mana Regen", "+12.5 Mana Regen"), "+1.5–12.5 Mana Regen");
        assert_eq!(modifier_range("+5 Ward", "+5 Ward"), "+5 Ward");
        assert_eq!(modifier_range("Armour +5", "Ward +9"), "Armour +5 – Ward +9");
        assert_eq!(implicit_line(Some(""), Some("+16 Armor")), "+16 Armor");
        assert_eq!(implicit_line(Some("+10 Armor"), None), "+10 Armor");
        assert_eq!(implicit_line(None, None), IMPLICIT_TEXT_UNAVAILABLE.get());
    }
}
