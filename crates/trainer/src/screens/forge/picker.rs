//! The affix picker's filters and rows, as pure logic over the catalog. `editor` holds a [`Filters`] and judges which
//! definitions fit the item once; a keystroke only runs [`run`] over those candidates, against the [`Index`] built once
//! per catalog. Rows are built by [`entry`] when the list asks for one, so a long list costs only the rows in view.
//!
//! **Stat tags** come from a definition's stat lines. A line's `tags` are the game's `AT` flags (`LE.dll`, read from the
//! build's metadata; see `crates/game-files/README.md`) where its property reads them as flags
//! ([`crate::definitions::files::ordinary`]); the rest name the stat itself, by the game's property list.
//!
//! **What stays hidden.** Unique modifiers repeat shipped stats ("Fractured Crown: Health" beside "Health"), so the default
//! [`SourceChoice::Game`] leaves them out until Source or Kind asks. A search of [`REVEAL_LENGTH`] or more characters that
//! matches a unique's *name* lists that unique's modifiers too; a search that matches only their stat does not.

use super::game::Catalog;
use super::item::{AffixDefinition, AffixKind, Category, effects_said};
use spiderland_phrases::{Conjunction, Phrase, list, phrases};
use std::collections::BTreeMap;

/// A search this long may bring in a unique's modifiers by the unique's name; a shorter one would match too many names.
pub const REVEAL_LENGTH: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Prefixes,
    Suffixes,
}

/// The prefix or suffix choices, in [`KindFilter`]'s order.
pub const PICKER_SIDES: [Phrase; 3] = [Phrase::new("Prefix or suffix"), Phrase::new("Prefixes"), Phrase::new("Suffixes")];

/// The affix picker's kind choices: any, or one of the game's affix types.
pub const PICKER_CATEGORIES: [(Phrase, Option<Category>); 9] = [
    (Phrase::new("Any kind"), None),
    (Phrase::new("Standard"), Some(Category::Standard)),
    (Phrase::new("Experimental"), Some(Category::Experimental)),
    (Phrase::new("Personal"), Some(Category::Personal)),
    (Phrase::new("Reforged set"), Some(Category::Set)),
    (Phrase::new("Idol enchantment"), Some(Category::IdolEnchantment)),
    (Phrase::new("Weaver idol"), Some(Category::IdolWeaver)),
    (Phrase::ctx("affix category", "Corrupted"), Some(Category::Corrupted)),
    (Phrase::new("Unique modifier"), Some(Category::FakeUniqueMod)),
];

/// The affix picker's class choices: any, affixes no class owns, or one class's own (the game's class order).
pub const PICKER_CLASSES: [Phrase; 7] = [
    Phrase::new("Any class"),
    Phrase::new("No class requirement"),
    Phrase::new("Primalist"),
    Phrase::new("Mage"),
    Phrase::new("Sentinel"),
    Phrase::new("Acolyte"),
    Phrase::new("Rogue"),
];

/// Whether an affix of class specificity `specificity` (bit 0 classless, bits 1–5 the classes) passes class choice `choice`.
fn class_fits(specificity: i32, choice: usize) -> bool {
    match choice {
        0 => true,
        1 => specificity & 0b111110 == 0,
        class => specificity & (1 << (class - 1)) != 0,
    }
}

/// One stat tag: the `AT` flags that carry it, and the properties (the game's property list) that are it.
#[derive(Clone, Copy, Debug)]
pub struct StatTag {
    pub name: Phrase,
    pub flags: i32,
    pub properties: &'static [u8],
}

const fn flags_and(name: Phrase, flags: i32, properties: &'static [u8]) -> StatTag {
    StatTag { name, flags, properties }
}

/// Every stat tag; a definition's mask has bit `i` for `STAT_TAGS[i]`. Two `AT` flags are left out: `PetResisted`, whose
/// meaning the game's data does not establish, and the `*_deprecated` ones.
pub const STAT_TAGS: [StatTag; 39] = [
    flags_and(Phrase::new("Damage"), 0, &[0, 41, 115, 117]),
    flags_and(Phrase::new("Physical"), 1, &[64, 106]),
    flags_and(Phrase::new("Lightning"), 2, &[15]),
    flags_and(Phrase::new("Cold"), 4, &[14]),
    flags_and(Phrase::new("Fire"), 8, &[13]),
    flags_and(Phrase::new("Void"), 16, &[26, 106]),
    flags_and(Phrase::new("Necrotic"), 32, &[27, 107]),
    flags_and(Phrase::new("Poison"), 64, &[28, 107]),
    flags_and(Phrase::new("Elemental"), 128, &[52]),
    flags_and(Phrase::new("Spell"), 256, &[3, 41]),
    flags_and(Phrase::new("Melee"), 512, &[]),
    flags_and(Phrase::new("Throwing"), 1024, &[]),
    flags_and(Phrase::new("Bow"), 2048, &[]),
    flags_and(Phrase::new("Damage over time"), 4096, &[]),
    flags_and(Phrase::new("Minion"), 8192, &[]),
    flags_and(Phrase::new("Totem"), 16384, &[]),
    flags_and(Phrase::new("Potion"), 65536, &[47, 48, 49, 90, 91]),
    flags_and(Phrase::new("Buff"), 131072, &[]),
    flags_and(Phrase::new("Channelling"), 262144, &[25]),
    flags_and(Phrase::new("Transform"), 524288, &[]),
    flags_and(Phrase::new("Low health"), 1048576, &[]),
    flags_and(Phrase::new("High health"), 2097152, &[]),
    flags_and(Phrase::new("Full health"), 4194304, &[]),
    flags_and(Phrase::new("On hit"), 8388608, &[]),
    flags_and(Phrase::new("Curse"), 16777216, &[]),
    flags_and(Phrase::new("Ailment"), 33554432, &[1, 42, 43, 100, 128]),
    flags_and(Phrase::new("Health"), 0, &[7, 17, 38, 44, 48, 51, 60, 93, 102]),
    flags_and(Phrase::new("Mana"), 0, &[8, 18, 24, 25, 40, 66, 69, 94]),
    flags_and(Phrase::new("Ward"), 0, &[16, 39, 90, 91, 92, 97, 99, 119]),
    flags_and(Phrase::new("Armor"), 0, &[10, 118]),
    flags_and(Phrase::new("Dodge"), 0, &[11]),
    flags_and(Phrase::new("Block"), 0, &[29, 53, 111]),
    flags_and(Phrase::new("Resistance"), 0, &[13, 14, 15, 26, 27, 28, 30, 52, 64, 106, 107]),
    flags_and(Phrase::new("Speed"), 0, &[2, 3, 9, 70]),
    flags_and(Phrase::new("Critical strike"), 0, &[4, 5, 89, 112, 114, 132, 133]),
    flags_and(Phrase::new("Attributes"), 0, &[19, 20, 21, 22, 23, 46]),
    flags_and(Phrase::new("Skill"), 0, &[58, 88]),
    flags_and(Phrase::new("Stun"), 0, &[12, 45, 55, 56, 71, 95]),
    flags_and(Phrase::new("Endurance"), 0, &[75, 76, 96]),
];

/// A definition's stat tags from its stat lines, each its property and its raw `tags`.
pub fn stat_tags(lines: impl Iterator<Item = (u8, i32)>) -> u64 {
    let mut mask = 0;
    for (property, flags) in lines {
        let reads_flags = crate::definitions::files::ordinary(property);
        for (index, tag) in STAT_TAGS.iter().enumerate() {
            if (reads_flags && flags & tag.flags != 0) || tag.properties.contains(&property) {
                mask |= 1u64 << index;
            }
        }
    }
    mask
}

/// Source: where a definition comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SourceChoice {
    #[default]
    Game,
    Uniques,
    All,
    Unique(u16),
}

/// Level requirement: what an affix may ask of its item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LevelChoice {
    #[default]
    Any,
    /// At most the level the item being edited requires.
    FitsItem,
    UpTo(u8),
}

/// The fixed caps Level offers after "Any level" and "Fits this item".
pub const LEVEL_CAPS: [u8; 4] = [20, 40, 60, 80];

/// Every filter but the search.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Filters {
    pub side: KindFilter,
    /// Index into [`PICKER_CATEGORIES`].
    pub category: usize,
    /// Index into [`PICKER_CLASSES`].
    pub class: usize,
    /// The equipment type; none for any slot.
    pub slot: Option<u16>,
    pub level: LevelChoice,
    /// Stat tags that must all be present, by [`STAT_TAGS`] bit.
    pub tags: u64,
    pub source: SourceChoice,
}

impl Filters {
    /// The picker's defaults: this item's slot, game affixes, anything else.
    pub fn for_slot(own_slot: Option<u16>) -> Self {
        Self {
            slot: own_slot,
            ..Self::default()
        }
    }

    fn asks_for_unique_kind(&self) -> bool {
        PICKER_CATEGORIES[self.category].1 == Some(Category::FakeUniqueMod)
    }

    /// The Source in force: asking for the Unique modifier kind brings them in, so a game-only Source gives way.
    pub fn effective_source(&self) -> SourceChoice {
        if self.source == SourceChoice::Game && self.asks_for_unique_kind() {
            SourceChoice::All
        } else {
            self.source
        }
    }

    /// Whether this can list nothing but unique modifiers, which need "I know what I'm doing".
    fn only_uniques(&self) -> bool {
        self.asks_for_unique_kind() || matches!(self.source, SourceChoice::Uniques | SourceChoice::Unique(_))
    }

    /// Takes Source back to game affixes; a kind that would overrule it goes too.
    pub fn set_source(&mut self, source: SourceChoice) {
        self.source = source;
        if source == SourceChoice::Game && self.asks_for_unique_kind() {
            self.category = 0;
        }
    }
}

/// One definition's lowercased search text, its unique's name apart so a search can tell them.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchKey {
    /// Name, category, id, what its lines say and its title; the separator never comes from a search box, so no match spans two parts.
    pub text: String,
    /// The source unique's name; empty for a shipped affix.
    pub unique: String,
}

impl SearchKey {
    pub fn of(definition: &AffixDefinition) -> Self {
        Self {
            text: format!(
                "{}\0{}\0{}\0{}\0{}",
                definition.name.to_lowercase(),
                definition.category.name().to_lowercase(),
                definition.id,
                effects_said(&definition.effects).to_lowercase(),
                definition.title.as_deref().unwrap_or_default().to_lowercase(),
            ),
            unique: definition.source.as_ref().map_or_else(String::new, |source| source.name.to_lowercase()),
        }
    }
}

/// What the picker computes once per catalog.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub keys: Vec<SearchKey>,
    /// The slots any definition rolls on or comes from, in the game's order, with their names.
    pub slots: Vec<(u16, String)>,
    /// The stat tags any definition has, by [`STAT_TAGS`] index.
    pub tags: Vec<usize>,
}

impl Index {
    pub fn build(catalog: &Catalog) -> Self {
        let keys = catalog.definitions.iter().map(SearchKey::of).collect();
        let mut slots = BTreeMap::new();
        let mut mask = 0u64;
        for definition in &catalog.definitions {
            mask |= definition.stat_tags;
            match &definition.source {
                Some(source) => {
                    slots.insert(source.item_type, ());
                }
                None => slots.extend(definition.item_types.iter().map(|slot| (*slot, ()))),
            }
        }
        Self {
            keys,
            slots: slots.into_keys().map(|slot| (slot, slot_name(catalog, slot))).collect(),
            tags: (0..STAT_TAGS.len()).filter(|index| mask & (1u64 << index) != 0).collect(),
        }
    }
}

/// A slot's name in the game's words.
pub fn slot_name(catalog: &Catalog, slot: u16) -> String {
    phrases! { Unnamed: "Item type {id}" }
    catalog
        .item_type(slot)
        .filter(|definition| !definition.name.is_empty())
        .map_or_else(|| Unnamed.with(&[("id", &slot)]), |definition| definition.name.clone())
}

/// What the search and filters apply to.
#[derive(Clone, Copy, Debug)]
pub struct Query<'a> {
    pub filters: &'a Filters,
    /// Lowercased.
    pub search: &'a str,
    /// The item's type and the types its idol shape also takes (`shares_slots`); empty without an item.
    pub own_types: &'a [u16],
    pub item_level: Option<u8>,
}

impl Query<'_> {
    fn reveals(&self, definition: &AffixDefinition, key: &SearchKey) -> bool {
        definition.source.is_some() && self.search.chars().count() >= REVEAL_LENGTH && key.unique.contains(self.search)
    }

    fn source_admits(&self, definition: &AffixDefinition, key: &SearchKey) -> bool {
        match self.filters.effective_source() {
            SourceChoice::Game => definition.source.is_none() || self.reveals(definition, key),
            SourceChoice::Uniques => definition.source.is_some(),
            SourceChoice::All => true,
            SourceChoice::Unique(unique) => definition.source.as_ref().is_some_and(|source| source.unique == unique),
        }
    }

    /// Slot, for a shipped affix, is where it rolls. A unique modifier rolls anywhere, so its slot is its unique's. The
    /// default slot gives way to a search that names a unique on another slot; a chosen one does not.
    fn slot_admits(&self, definition: &AffixDefinition, key: &SearchKey) -> bool {
        let Some(slot) = self.filters.slot else {
            return true;
        };
        let own = self.own_types.contains(&slot);
        let matches = |types: &[u16]| if own { types.iter().any(|t| self.own_types.contains(t)) } else { types.contains(&slot) };
        match &definition.source {
            Some(source) => matches(&[source.item_type]) || (own && self.reveals(definition, key)),
            None => matches(&definition.item_types),
        }
    }

    fn level_admits(&self, definition: &AffixDefinition) -> bool {
        match self.filters.level {
            LevelChoice::Any => true,
            LevelChoice::FitsItem => self.item_level.is_none_or(|level| definition.level <= level),
            LevelChoice::UpTo(level) => definition.level <= level,
        }
    }

    /// Everything but Source, with the search last: it is the only part that touches text.
    fn rest_admits(&self, definition: &AffixDefinition, key: &SearchKey) -> bool {
        let filters = self.filters;
        (match filters.side {
            KindFilter::All => true,
            KindFilter::Prefixes => definition.kind == AffixKind::Prefix,
            KindFilter::Suffixes => definition.kind == AffixKind::Suffix,
        }) && PICKER_CATEGORIES[filters.category].1.is_none_or(|category| definition.category == category)
            && class_fits(definition.class_specificity, filters.class)
            && definition.stat_tags & filters.tags == filters.tags
            && self.level_admits(definition)
            && self.slot_admits(definition, key)
            && (self.search.is_empty() || key.text.contains(self.search) || key.unique.contains(self.search))
    }
}

/// What the search and filters leave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub list: Vec<usize>,
    /// Unique modifiers that pass every filter but the default Source.
    pub hidden_uniques: usize,
}

/// Runs `query` over `candidates`, indices into `definitions` and `keys`, keeping their order.
pub fn run(query: &Query<'_>, candidates: &[usize], definitions: &[AffixDefinition], keys: &[SearchKey]) -> Outcome {
    let mut outcome = Outcome::default();
    for &index in candidates {
        let (Some(definition), Some(key)) = (definitions.get(index), keys.get(index)) else {
            continue;
        };
        if !query.rest_admits(definition, key) {
            continue;
        }
        if query.source_admits(definition, key) {
            outcome.list.push(index);
        } else if definition.source.is_some() {
            outcome.hidden_uniques += 1;
        }
    }
    outcome
}

/// The uniques whose modifiers are among `candidates`, within `slot` when one is chosen, sorted by name.
pub fn source_uniques(candidates: &[usize], definitions: &[AffixDefinition], slot: Option<u16>, own_types: &[u16]) -> Vec<(u16, String)> {
    let mut uniques = BTreeMap::new();
    let own = slot.is_some_and(|slot| own_types.contains(&slot));
    for source in candidates
        .iter()
        .filter_map(|index| definitions.get(*index))
        .filter_map(|definition| definition.source.as_ref())
    {
        if slot.is_none_or(|slot| source.item_type == slot || (own && own_types.contains(&source.item_type))) {
            uniques.entry(source.unique).or_insert_with(|| source.name.clone());
        }
    }
    let mut uniques: Vec<(u16, String)> = uniques.into_iter().collect();
    uniques.sort_by(|a, b| a.1.cmp(&b.1));
    uniques
}

/// The level choices for an item that requires `item_level`, each with its words.
pub fn level_choices(item_level: Option<u8>) -> Vec<(LevelChoice, String)> {
    phrases! {
        Any: "Any level"
        Fits: "Fits this item (level {level})"
        UpTo: "Up to level {level}"
    }
    std::iter::once((LevelChoice::Any, Any.into()))
        .chain(item_level.map(|level| (LevelChoice::FitsItem, Fits.with(&[("level", &level)]))))
        .chain(LEVEL_CAPS.iter().map(|level| (LevelChoice::UpTo(*level), UpTo.with(&[("level", level)]))))
        .collect()
}

phrases! {
    /// Slot's first choice.
    pub AnySlot: "Any slot"
}

/// The fixed Source choices, before the uniques.
pub fn source_words() -> [&'static str; 3] {
    phrases! {
        Game: "Game affixes"
        Uniques: "Unique modifiers"
        All: "All sources"
    }
    [Game.get(), Uniques.get(), All.get()]
}

/// A filter an active-filter chip stands for; removing the chip clears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Side,
    Category,
    Class,
    Slot,
    Level,
    Tag(usize),
    Source,
}

/// The filters off their defaults, in the order the filter bar shows them. `own_slot` is the default slot.
pub fn chips(filters: &Filters, own_slot: Option<u16>) -> Vec<Chip> {
    let mut chips = Vec::new();
    if filters.slot != own_slot {
        chips.push(Chip::Slot);
    }
    if filters.level != LevelChoice::Any {
        chips.push(Chip::Level);
    }
    if filters.source != SourceChoice::Game {
        chips.push(Chip::Source);
    }
    chips.extend((0..STAT_TAGS.len()).filter(|index| filters.tags & (1u64 << index) != 0).map(Chip::Tag));
    if filters.side != KindFilter::All {
        chips.push(Chip::Side);
    }
    if filters.category != 0 {
        chips.push(Chip::Category);
    }
    if filters.class != 0 {
        chips.push(Chip::Class);
    }
    chips
}

/// Clears what `chip` stands for.
pub fn clear_chip(filters: &mut Filters, chip: Chip, own_slot: Option<u16>) {
    match chip {
        Chip::Side => filters.side = KindFilter::All,
        Chip::Category => filters.category = 0,
        Chip::Class => filters.class = 0,
        Chip::Slot => filters.slot = own_slot,
        Chip::Level => filters.level = LevelChoice::Any,
        Chip::Tag(index) => filters.tags &= !(1u64 << index),
        Chip::Source => filters.set_source(SourceChoice::Game),
    }
}

/// A chip's words: each names its filter's value.
pub fn chip_text(chip: Chip, filters: &Filters, catalog: &Catalog) -> String {
    phrases! {
        Slot: "Slot: {name}"
        FitsItem: "Fits this item"
        UpTo: "Up to level {level}"
        From: "From {name}"
    }
    match chip {
        Chip::Side => PICKER_SIDES[filters.side as usize].into(),
        Chip::Category => PICKER_CATEGORIES[filters.category].0.into(),
        Chip::Class => PICKER_CLASSES[filters.class].into(),
        Chip::Slot => filters.slot.map_or_else(|| AnySlot.into(), |slot| Slot.with(&[("name", &slot_name(catalog, slot))])),
        Chip::Level => match filters.level {
            LevelChoice::UpTo(level) => UpTo.with(&[("level", &level)]),
            _ => FitsItem.into(),
        },
        Chip::Tag(index) => STAT_TAGS[index].name.into(),
        Chip::Source => match filters.source {
            SourceChoice::Unique(unique) => From.with(&[("name", &catalog.source_name(unique))]),
            SourceChoice::All => source_words()[2].to_owned(),
            _ => source_words()[1].to_owned(),
        },
    }
}

/// What Tags shows: "Any tag", or the tags chosen.
pub fn tags_summary(filters: &Filters) -> String {
    phrases! { AnyTag: "Any tag" }
    let names: Vec<&str> = (0..STAT_TAGS.len())
        .filter(|index| filters.tags & (1u64 << index) != 0)
        .map(|index| STAT_TAGS[index].name.get())
        .collect();
    if names.is_empty() { AnyTag.into() } else { names.join(", ") }
}

/// The list's size and the unique modifiers the default Source keeps out, for the line above it.
pub fn count_text(count: usize) -> String {
    phrases! { Count: "{n} affix" | "{n} affixes" }
    Count.count(count)
}

pub fn hidden_text(hidden: usize) -> String {
    phrases! { Hidden: "Show {n} unique modifier" | "Show {n} unique modifiers" }
    Hidden.count(hidden)
}

/// What an empty list says and which ways out it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Empty {
    pub reason: String,
    pub clear_search: bool,
    pub clear_filters: bool,
    pub allow: bool,
}

/// Why nothing is listed. `candidates` is how many definitions the item let through; `more_if_allowed` whether "Allow
/// incompatible affixes" would list any under this search and these filters; `unrestricted` is "I know what I'm doing".
pub fn empty(query: &Query<'_>, own_slot: Option<u16>, candidates: usize, more_if_allowed: bool, unrestricted: bool) -> Empty {
    phrases! {
        NothingFits: "No affix fits this item."
        NeedsOverride: "Unique modifiers need \"I know what I'm doing\" in Settings."
        OtherSlot: "These affixes roll on another slot."
        SearchAndFilters: "No affix matches this search with these filters."
        Search: "No affix matches this search."
        Filters: "No affix matches these filters."
    }
    let has_filters = !chips(query.filters, own_slot).is_empty();
    let has_search = !query.search.is_empty();
    let locked_out = query.filters.only_uniques() && !unrestricted;
    let reason: String = if locked_out {
        NeedsOverride.into()
    } else if candidates == 0 {
        NothingFits.into()
    } else if more_if_allowed && query.filters.slot.is_some_and(|slot| Some(slot) != own_slot) {
        OtherSlot.into()
    } else if has_search && has_filters {
        SearchAndFilters.into()
    } else if has_search {
        Search.into()
    } else {
        Filters.into()
    };
    Empty {
        reason,
        clear_search: has_search,
        clear_filters: has_filters,
        allow: more_if_allowed && !locked_out,
    }
}

/// What a row shows beyond its name, as pills, in order: slot, kind, tiers, category, source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Reforged,
    Unique,
}

/// A row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub pills: Vec<(String, Tone)>,
    /// Everything else, one fact a line: the tooltip and the (?).
    pub detail: String,
    /// The pills and the detail, for a screen reader, which sees neither.
    pub spoken: String,
}

/// The slot a row names: the one the Slot filter asks for when it rolls there, else its first, and how many more.
fn slot_pill(definition: &AffixDefinition, catalog: &Catalog, slot: Option<u16>, own_types: &[u16]) -> Option<String> {
    phrases! { More: "{slot} +{more}" }
    if let Some(source) = &definition.source {
        return Some(slot_name(catalog, source.item_type));
    }
    let types = &definition.item_types;
    let first = slot
        .filter(|slot| types.contains(slot))
        .or_else(|| types.iter().find(|t| own_types.contains(t)).copied())
        .or_else(|| types.first().copied())?;
    let name = slot_name(catalog, first);
    Some(if types.len() > 1 {
        More.with(&[("slot", &name), ("more", &(types.len() - 1))])
    } else {
        name
    })
}

/// Every tier's value range of a definition, "T1 +5–14, T2 +15–24"; one range alone when its tiers all read the same (a
/// unique modifier's do); empty where the game data has none.
pub fn ladder(definition: &AffixDefinition, catalog: &Catalog, base: Option<(u16, u16)>) -> String {
    // A tier and its range wrap together: a no-break space after the tier, a word joiner after the dash.
    phrases! { LadderStep: "T{tier}\u{a0}{range}" }
    let ranges: Vec<(u8, String)> = (1..=definition.max_tier)
        .filter_map(|tier| {
            let range = catalog.tier_range(definition.id, tier, base);
            (!range.is_empty()).then(|| (tier, range.replace('–', "–\u{2060}")))
        })
        .collect();
    match ranges.as_slice() {
        [(_, first), rest @ ..] if rest.iter().all(|(_, range)| range == first) => first.clone(),
        _ => ranges
            .iter()
            .map(|(tier, range)| LadderStep.with(&[("tier", tier), ("range", range)]))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// How many slot names a tooltip lists before it counts the rest.
const LISTED_SLOTS: usize = 8;

/// A definition as the list shows it. `slot` is the Slot filter's, so the row names what was asked for; `base` is the item's,
/// so its values read as they would on it.
pub fn entry(definition: &AffixDefinition, catalog: &Catalog, slot: Option<u16>, own_types: &[u16], base: Option<(u16, u16)>) -> Entry {
    phrases! {
        From: "From {unique} ({slot})"
        RollsOn: "Rolls on {slots}"
        RollsOnMore: "Rolls on {slots} and {more} more"
        Level: "Requires level {level}"
        Classes: "Class: {classes}"
        Tags: "Tags: {tags}"
    }

    let name = definition.source.as_ref().map_or_else(|| definition.name.clone(), |source| source.line.clone());
    let mut pills = Vec::with_capacity(5);
    if let Some(slot) = slot_pill(definition, catalog, slot, own_types) {
        pills.push((slot, Tone::Neutral));
    }
    pills.push((definition.kind.name().to_owned(), Tone::Neutral));
    // A unique modifier's tiers all hold the same values, so they tell nothing apart.
    if definition.source.is_none() {
        pills.push((definition.tiers_text(), Tone::Neutral));
    }
    if !matches!(definition.category, Category::Standard | Category::FakeUniqueMod) {
        let tone = if definition.is_reforged() { Tone::Reforged } else { Tone::Neutral };
        pills.push((definition.category.name().into_owned(), tone));
    }
    if let Some(source) = &definition.source {
        pills.push((source.name.clone(), Tone::Unique));
    }
    if let Some(title) = &definition.title {
        pills.push((title.clone(), Tone::Neutral));
    }

    let mut lines = vec![name.clone()];
    for line in catalog.effect_lines(definition, base) {
        if !line.eq_ignore_ascii_case(&name) {
            lines.push(line);
        }
    }
    for effect in &definition.effects {
        if !effect.explanation.is_empty() && !lines.contains(&effect.explanation) {
            lines.push(effect.explanation.clone());
        }
    }
    match &definition.source {
        Some(source) => lines.push(From.with(&[("unique", &source.name), ("slot", &slot_name(catalog, source.item_type))])),
        None if !definition.item_types.is_empty() => {
            let shown: Vec<String> = definition.item_types.iter().take(LISTED_SLOTS).map(|slot| slot_name(catalog, *slot)).collect();
            let more = definition.item_types.len().saturating_sub(LISTED_SLOTS);
            let slots = list(&shown, Conjunction::And);
            lines.push(if more == 0 {
                RollsOn.with(&[("slots", &slots)])
            } else {
                RollsOnMore.with(&[("slots", &shown.join(", ")), ("more", &more)])
            });
        }
        None => {}
    }
    if definition.level > 0 {
        lines.push(Level.with(&[("level", &definition.level)]));
    }
    let classes: Vec<&str> = (2..PICKER_CLASSES.len())
        .filter(|choice| definition.class_specificity & (1 << (choice - 1)) != 0)
        .map(|choice| PICKER_CLASSES[choice].get())
        .collect();
    if !classes.is_empty() {
        lines.push(Classes.with(&[("classes", &classes.join(", "))]));
    }
    let tags: Vec<&str> = (0..STAT_TAGS.len())
        .filter(|index| definition.stat_tags & (1u64 << index) != 0)
        .map(|index| STAT_TAGS[index].name.get())
        .collect();
    if !tags.is_empty() {
        lines.push(Tags.with(&[("tags", &tags.join(", "))]));
    }
    // A unique modifier's values are on its lines above.
    let ranges = if definition.source.is_some() && !definition.effects.is_empty() {
        String::new()
    } else {
        ladder(definition, catalog, base)
    };
    if !ranges.is_empty() {
        lines.push(ranges);
    }
    let detail = lines.join("\n");
    let spoken = pills
        .iter()
        .map(|(text, _)| text.as_str())
        .chain(std::iter::once(detail.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    Entry { name, pills, detail, spoken }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::forge::game::ItemTypeDefinition;
    use crate::screens::forge::item::{AffixEffect, AffixSource, TierProperty};

    const BOOTS: u16 = 3;
    const HELMET: u16 = 0;
    const RING: u16 = 20;
    const FIRE: usize = 4;
    const SPELL: usize = 9;

    fn tag_mask(names: &[usize]) -> u64 {
        names.iter().fold(0, |mask, index| mask | (1u64 << index))
    }

    fn shipped(id: u16, name: &str, kind: AffixKind, slots: &[u16]) -> AffixDefinition {
        AffixDefinition {
            id,
            name: name.into(),
            kind,
            category: Category::Standard,
            item_types: slots.to_vec(),
            max_tier: 8,
            writable: true,
            class_specificity: 0,
            level: 0,
            source: None,
            stat_tags: 0,
            effects: Vec::new(),
            title: None,
        }
    }

    fn modifier(id: u16, unique: u16, unique_name: &str, slot: u16, line: &str) -> AffixDefinition {
        AffixDefinition {
            source: Some(AffixSource {
                unique,
                name: unique_name.into(),
                item_type: slot,
                line: line.into(),
            }),
            category: Category::FakeUniqueMod,
            ..shipped(id, &format!("{unique_name}: {line}"), AffixKind::Prefix, &[HELMET, BOOTS, RING])
        }
    }

    /// Five shipped affixes and four unique modifiers, one of them repeating Health and one of several lines.
    fn definitions() -> Vec<AffixDefinition> {
        vec![
            AffixDefinition {
                stat_tags: tag_mask(&[26]),
                ..shipped(25, "Health", AffixKind::Suffix, &[BOOTS, RING])
            },
            AffixDefinition {
                level: 25,
                stat_tags: tag_mask(&[FIRE, 32]),
                ..shipped(13, "Fire Resistance", AffixKind::Suffix, &[BOOTS])
            },
            AffixDefinition {
                level: 42,
                stat_tags: tag_mask(&[FIRE, SPELL, 0]),
                ..shipped(38, "Increased Fire Spell Damage", AffixKind::Prefix, &[RING])
            },
            AffixDefinition {
                category: Category::Experimental,
                ..shipped(90, "Haste on Hit", AffixKind::Prefix, &[BOOTS])
            },
            AffixDefinition {
                class_specificity: 1 << 2,
                ..shipped(91, "Mage's Focus", AffixKind::Prefix, &[BOOTS])
            },
            AffixDefinition {
                stat_tags: tag_mask(&[26]),
                level: 30,
                ..modifier(1500, 1, "Fractured Crown", HELMET, "Health")
            },
            modifier(1501, 1, "Fractured Crown", HELMET, "15-30% increased speed"),
            AffixDefinition {
                stat_tags: tag_mask(&[26]),
                ..modifier(1502, 77, "Wayfarer's Boots", BOOTS, "Health")
            },
            modifier(1503, 80, "Ember Band", RING, "Burns on hit"),
        ]
    }

    fn catalog() -> Catalog {
        Catalog {
            item_types: [(HELMET, "Helmet"), (BOOTS, "Boots"), (RING, "Ring")]
                .into_iter()
                .map(|(item_type, name)| ItemTypeDefinition {
                    item_type,
                    name: name.into(),
                    bases: vec![],
                })
                .collect(),
            definitions: definitions(),
            ..Catalog::default()
        }
    }

    fn keys() -> Vec<SearchKey> {
        definitions().iter().map(SearchKey::of).collect()
    }

    /// The names listed from every definition (Allow incompatible's candidates), with the item a level-20 pair of boots.
    fn listed(filters: &Filters, search: &str) -> Outcome {
        let definitions = definitions();
        let candidates: Vec<usize> = (0..definitions.len()).collect();
        run(
            &Query {
                filters,
                search,
                own_types: &[BOOTS],
                item_level: Some(20),
            },
            &candidates,
            &definitions,
            &keys(),
        )
    }

    fn ids(outcome: &Outcome) -> Vec<u16> {
        let definitions = definitions();
        outcome.list.iter().map(|index| definitions[*index].id).collect()
    }

    fn defaults() -> Filters {
        Filters::for_slot(Some(BOOTS))
    }

    #[test]
    fn the_default_lists_this_slots_game_affixes_and_counts_the_unique_modifiers_it_hides() {
        let outcome = listed(&defaults(), "");
        assert_eq!(ids(&outcome), [25, 13, 90, 91], "no unique modifier, and nothing for another slot");
        assert_eq!(outcome.hidden_uniques, 1, "only Wayfarer's Boots's own modifier passes the slot");
    }

    #[test]
    fn a_search_for_a_stat_leaves_duplicates_hidden_and_a_uniques_name_brings_its_modifiers() {
        let by_stat = listed(&defaults(), "health");
        assert_eq!(ids(&by_stat), [25], "Health alone: the unique modifiers repeating it stay out");
        assert_eq!(by_stat.hidden_uniques, 1);
        assert_eq!(ids(&listed(&defaults(), "wayfarer")), [1502], "by the unique's name, in this slot");
        assert_eq!(ids(&listed(&defaults(), "fractured")), [1500, 1501], "the default slot gives way: the unique is a helmet");
        let all_sources = Filters {
            source: SourceChoice::All,
            ..defaults()
        };
        assert_eq!(ids(&listed(&all_sources, "fractured")), [1500, 1501], "and it does under any Source");
    }

    #[test]
    fn a_search_too_short_to_name_a_unique_reveals_nothing() {
        assert!(ids(&listed(&defaults(), "fr")).iter().all(|id| *id < 1500), "{:?}", ids(&listed(&defaults(), "fr")));
        assert_eq!(ids(&listed(&defaults(), "fra")), [1500, 1501]);
    }

    #[test]
    fn a_chosen_slot_holds_against_a_search_for_a_unique() {
        let filters = Filters { slot: Some(RING), ..defaults() };
        assert!(ids(&listed(&filters, "fractured")).is_empty(), "the search does not undo a chosen slot");
    }

    #[test]
    fn slot_reads_where_an_affix_rolls_and_where_a_unique_modifier_comes_from() {
        let ring = Filters { slot: Some(RING), ..defaults() };
        assert_eq!(ids(&listed(&ring, "")), [25, 38]);
        let ring_uniques = Filters {
            source: SourceChoice::Uniques,
            ..ring
        };
        assert_eq!(ids(&listed(&ring_uniques, "")), [1503], "every modifier rolls on rings; this one comes from one");
        let any = Filters { slot: None, ..defaults() };
        assert_eq!(ids(&listed(&any, "")), [25, 13, 38, 90, 91]);
        assert_eq!(listed(&any, "").hidden_uniques, 4);
    }

    #[test]
    fn an_idol_shape_counts_the_types_it_shares_as_its_own_slot() {
        let definitions = definitions();
        let candidates: Vec<usize> = (0..definitions.len()).collect();
        let filters = Filters::for_slot(Some(29));
        let query = Query {
            filters: &filters,
            search: "",
            own_types: &[29, BOOTS],
            item_level: None,
        };
        let outcome = run(&query, &candidates, &definitions, &keys());
        assert!(
            outcome.list.iter().any(|index| definitions[*index].id == 13),
            "a boots affix counts for a shape sharing boots"
        );
    }

    #[test]
    fn level_fits_the_item_or_a_cap() {
        let fits = Filters {
            level: LevelChoice::FitsItem,
            ..defaults()
        };
        assert_eq!(ids(&listed(&fits, "")), [25, 90, 91], "Fire Resistance asks for 25, the item 20");
        let cap = Filters {
            level: LevelChoice::UpTo(40),
            ..defaults()
        };
        assert_eq!(ids(&listed(&cap, "")), [25, 13, 90, 91]);
        let unknown = Filters {
            level: LevelChoice::FitsItem,
            ..defaults()
        };
        let definitions = definitions();
        let query = Query {
            filters: &unknown,
            search: "",
            own_types: &[BOOTS],
            item_level: None,
        };
        assert_eq!(run(&query, &[1], &definitions, &keys()).list, [1], "without the item's level nothing is judged by it");
    }

    #[test]
    fn stat_tags_all_have_to_be_present() {
        let any = |tags: &[usize]| Filters {
            slot: None,
            tags: tag_mask(tags),
            ..defaults()
        };
        assert_eq!(ids(&listed(&any(&[FIRE]), "")), [13, 38]);
        assert_eq!(ids(&listed(&any(&[FIRE, SPELL]), "")), [38]);
    }

    #[test]
    fn source_chooses_game_affixes_unique_modifiers_or_one_unique() {
        let source = |source| Filters { slot: None, source, ..defaults() };
        assert_eq!(ids(&listed(&source(SourceChoice::Uniques), "")), [1500, 1501, 1502, 1503]);
        assert_eq!(ids(&listed(&source(SourceChoice::Unique(1)), "")), [1500, 1501]);
        assert_eq!(listed(&source(SourceChoice::All), "").list.len(), 9);
        assert_eq!(listed(&source(SourceChoice::All), "").hidden_uniques, 0);
    }

    #[test]
    fn asking_for_the_unique_modifier_kind_brings_them_in_and_game_affixes_gives_it_up() {
        let mut filters = Filters {
            slot: None,
            category: 8,
            ..defaults()
        };
        assert_eq!(filters.effective_source(), SourceChoice::All);
        assert_eq!(ids(&listed(&filters, "")), [1500, 1501, 1502, 1503]);
        filters.set_source(SourceChoice::Game);
        assert_eq!((filters.category, filters.effective_source()), (0, SourceChoice::Game));
    }

    #[test]
    fn kind_class_and_side_filter_and_combine_with_the_rest() {
        let prefixes = Filters {
            side: KindFilter::Prefixes,
            ..defaults()
        };
        assert_eq!(ids(&listed(&prefixes, "")), [90, 91]);
        let suffixes = Filters {
            side: KindFilter::Suffixes,
            ..defaults()
        };
        assert_eq!(ids(&listed(&suffixes, "")), [25, 13]);
        let experimental = Filters { category: 2, ..defaults() };
        assert_eq!(ids(&listed(&experimental, "")), [90]);
        let mage = Filters { class: 3, ..defaults() };
        assert_eq!(ids(&listed(&mage, "")), [91]);
        let combined = Filters {
            side: KindFilter::Suffixes,
            level: LevelChoice::UpTo(40),
            tags: tag_mask(&[FIRE]),
            ..defaults()
        };
        assert_eq!(ids(&listed(&combined, "fire")), [13]);
        assert!(ids(&listed(&combined, "health")).is_empty());
    }

    #[test]
    fn candidates_bound_the_list_and_keep_their_order() {
        let definitions = definitions();
        let filters = defaults();
        let query = Query {
            filters: &filters,
            search: "",
            own_types: &[BOOTS],
            item_level: None,
        };
        assert_eq!(run(&query, &[3, 0], &definitions, &keys()).list, [3, 0]);
        assert!(run(&query, &[99], &definitions, &keys()).list.is_empty(), "an index past the catalog is skipped");
    }

    #[test]
    fn chips_name_what_is_off_its_default_and_clear_it() {
        let mut filters = Filters {
            slot: None,
            level: LevelChoice::UpTo(40),
            tags: tag_mask(&[FIRE]),
            source: SourceChoice::Unique(1),
            side: KindFilter::Suffixes,
            category: 2,
            class: 3,
        };
        let own = Some(BOOTS);
        let found = chips(&filters, own);
        assert_eq!(found, [Chip::Slot, Chip::Level, Chip::Source, Chip::Tag(FIRE), Chip::Side, Chip::Category, Chip::Class]);
        for chip in found {
            clear_chip(&mut filters, chip, own);
        }
        assert_eq!(filters, Filters::for_slot(own));
        assert!(chips(&filters, own).is_empty(), "the defaults show no chip");
    }

    #[test]
    fn an_empty_list_says_why_and_offers_a_way_out() {
        let query = |filters, search| Query {
            filters,
            search,
            own_types: &[BOOTS],
            item_level: None,
        };
        let own = Some(BOOTS);
        let plain = defaults();
        let searched = empty(&query(&plain, "nope"), own, 5, false, true);
        assert_eq!(searched.reason, "No affix matches this search.");
        assert!(searched.clear_search && !searched.clear_filters && !searched.allow);

        let filtered = Filters {
            level: LevelChoice::UpTo(20),
            ..defaults()
        };
        let by_filters = empty(&query(&filtered, ""), own, 5, false, true);
        assert_eq!(by_filters.reason, "No affix matches these filters.");
        assert!(!by_filters.clear_search && by_filters.clear_filters);
        assert_eq!(
            empty(&query(&filtered, "x"), own, 5, false, true).reason,
            "No affix matches this search with these filters."
        );

        let other_slot = Filters { slot: Some(RING), ..defaults() };
        let elsewhere = empty(&query(&other_slot, ""), own, 5, true, true);
        assert_eq!(elsewhere.reason, "These affixes roll on another slot.");
        assert!(elsewhere.allow && elsewhere.clear_filters);

        let uniques = Filters {
            source: SourceChoice::Uniques,
            ..defaults()
        };
        let locked = empty(&query(&uniques, ""), own, 5, true, false);
        assert_eq!(locked.reason, "Unique modifiers need \"I know what I'm doing\" in Settings.");
        assert!(!locked.allow, "the list would show them, but they could not be used");
        assert_eq!(empty(&query(&plain, ""), own, 0, true, false).reason, "No affix fits this item.");
    }

    #[test]
    fn at_flags_count_only_where_the_property_reads_them_and_properties_name_their_stat() {
        let names = |mask: u64| -> Vec<&str> { (0..STAT_TAGS.len()).filter(|i| mask & (1u64 << i) != 0).map(|i| STAT_TAGS[i].name.get()).collect() };
        assert_eq!(names(stat_tags([(0, 8 | 256)].into_iter())), ["Damage", "Fire", "Spell"]);
        assert_eq!(names(stat_tags([(13, 0)].into_iter())), ["Fire", "Resistance"]);
        assert_eq!(names(stat_tags([(7, 0)].into_iter())), ["Health"]);
        assert_eq!(names(stat_tags([(98, 8)].into_iter())), [] as [&str; 0], "a player property's tags are an index");
        assert_eq!(names(stat_tags([(58, 8)].into_iter())), ["Skill"], "an ability's tags are its ID");
        assert_eq!(names(stat_tags([(1, 32768)].into_iter())), ["Ailment"], "PetResisted has no tag");
    }

    #[test]
    fn a_row_shows_pills_and_keeps_the_rest_for_its_detail() {
        let catalog = catalog();
        let health = entry(&catalog.definitions[0], &catalog, Some(BOOTS), &[BOOTS], None);
        assert_eq!(health.name, "Health");
        let pills: Vec<&str> = health.pills.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(pills, ["Boots +1", "Suffix", "T1–T8"]);
        assert!(health.detail.contains("Rolls on Boots and Ring"), "{}", health.detail);
        assert!(health.detail.contains("Tags: Health"), "{}", health.detail);

        let duplicate = entry(&catalog.definitions[5], &catalog, Some(BOOTS), &[BOOTS], None);
        assert_eq!(duplicate.name, "Health", "its unique's name moves to its pill");
        assert_eq!(duplicate.pills.last(), Some(&("Fractured Crown".to_owned(), Tone::Unique)));
        assert_eq!(duplicate.pills[0].0, "Helmet", "a unique modifier names its unique's slot");
        assert!(duplicate.detail.contains("From Fractured Crown (Helmet)"), "{}", duplicate.detail);
        assert!(duplicate.detail.contains("Requires level 30"), "{}", duplicate.detail);
    }

    /// A one-line affix on a catalog whose only base scales its values by `modifier`.
    fn scaled_catalog(modifier: f32, standard: f32) -> Catalog {
        let mut catalog = catalog();
        catalog.item_types = vec![ItemTypeDefinition {
            item_type: 12,
            name: "2H Axes".into(),
            bases: vec![crate::screens::forge::game::BaseDefinition {
                sub_type: 0,
                name: "Axe".into(),
                level: 0,
                corrupted: false,
                class_requirement: 0,
                implicits: vec![],
                affix_effect_modifier: modifier,
            }],
        }];
        catalog.standard_effects.insert(1500, standard);
        catalog
    }

    fn penetration(id: u16) -> AffixDefinition {
        let effect = |heading: &str, words: &str, explanation: &str| AffixEffect {
            heading: heading.into(),
            words: words.into(),
            explanation: explanation.into(),
            hides_value: false,
            no_plus: false,
        };
        AffixDefinition {
            max_tier: 7,
            effects: vec![
                effect("Cold Penetration", "Cold Penetration", "It reduces resistance."),
                effect("Lightning Penetration", "Lightning Penetration", "It reduces resistance."),
            ],
            ..shipped(id, "Ucenui's", AffixKind::Prefix, &[BOOTS])
        }
    }

    #[test]
    fn the_tip_writes_each_line_with_its_value_on_this_item() {
        let mut catalog = scaled_catalog(2.2, 0.0);
        for tier in 1..=7 {
            let column = Some(TierProperty::AddedPercent { min: 0.1, max: 0.2 });
            catalog.tier_columns.insert((1500, tier), vec![column, column]);
        }
        let definition = penetration(1500);
        let on_nothing = entry(&definition, &catalog, Some(BOOTS), &[BOOTS], None);
        assert!(on_nothing.detail.contains("+10–20% Cold Penetration"), "{}", on_nothing.detail);
        let on_two_handed = entry(&definition, &catalog, Some(BOOTS), &[BOOTS], Some((12, 0)));
        assert!(on_two_handed.detail.contains("+32–64% Lightning Penetration"), "{}", on_two_handed.detail);
        assert_eq!(
            on_two_handed.detail.matches("It reduces resistance.").count(),
            1,
            "an explanation reads once: {}",
            on_two_handed.detail
        );
    }

    #[test]
    fn the_games_scale_follows_the_item_and_what_the_affix_was_written_for() {
        assert_eq!(scaled_catalog(0.0, 0.0).value_scale(1500, Some((12, 0))), 1.0);
        assert_eq!(scaled_catalog(2.2, 2.2).value_scale(1500, Some((12, 0))), 1.0, "written for this base");
        assert!((scaled_catalog(2.2, 0.0).value_scale(1500, Some((12, 0))) - 3.2).abs() < 1e-6);
        assert!((scaled_catalog(0.0, 1.0).value_scale(1500, Some((12, 0))) - 0.5).abs() < 1e-6, "the formula's halving");
        assert_eq!(scaled_catalog(2.2, 0.0).value_scale(1500, None), 1.0, "no item, the values as written");
    }

    #[test]
    fn a_line_where_tiers_differ_or_the_game_hides_the_value_is_named_alone() {
        let mut catalog = catalog();
        for tier in 1..=7u8 {
            let first = Some(TierProperty::Flat {
                min: f64::from(tier),
                max: f64::from(tier),
                whole: true,
            });
            let second = Some(TierProperty::Increased { min: -0.5, max: -0.25 });
            catalog.tier_columns.insert((1500, tier), vec![first, second]);
        }
        let mut definition = penetration(1500);
        definition.effects[1].words = "reduced Mana".into();
        assert_eq!(catalog.effect_lines(&definition, None), ["Cold Penetration", "25–50% reduced Mana"]);
        definition.effects[1].hides_value = true;
        assert_eq!(catalog.effect_lines(&definition, None)[1], "Lightning Penetration");
    }

    #[test]
    fn a_title_goes_to_a_pill_and_search_reads_what_an_affix_does() {
        let mut definition = penetration(705);
        definition.name = effects_said(&definition.effects);
        definition.title = Some("Ucenui's".into());
        let entry = entry(&definition, &catalog(), Some(BOOTS), &[BOOTS], None);
        assert_eq!(entry.name, "Cold Penetration, Lightning Penetration");
        assert_eq!(entry.pills.last(), Some(&("Ucenui's".to_owned(), Tone::Neutral)));
        let key = SearchKey::of(&definition);
        assert!(key.text.contains("lightning penetration") && key.text.contains("ucenui's"), "{key:?}");
    }

    #[test]
    fn a_ladder_with_one_range_reads_once() {
        let mut catalog = catalog();
        for tier in 1..=8 {
            let flat = |min: f64, max: f64| vec![Some(TierProperty::Flat { min, max, whole: true })];
            catalog.tier_columns.insert((1500, tier), flat(5.0, 14.0));
            catalog.tier_columns.insert((25, tier), flat(f64::from(tier), f64::from(tier)));
        }
        let unique = catalog.definitions[5].clone();
        assert_eq!(ladder(&unique, &catalog, None), "+5–\u{2060}14");
        let shipped = catalog.definitions[0].clone();
        assert!(ladder(&shipped, &catalog, None).starts_with("T1\u{a0}+1, T2\u{a0}+2"));
    }

    #[test]
    fn the_index_lists_slots_in_the_games_order_and_only_tags_in_use() {
        let index = Index::build(&catalog());
        assert_eq!(index.slots.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(), [HELMET, BOOTS, RING]);
        assert_eq!(index.slots[1].1, "Boots");
        assert_eq!(index.tags, [0, FIRE, SPELL, 26, 32]);
        assert_eq!(index.keys.len(), definitions().len());
    }

    #[test]
    fn source_uniques_follow_the_slot_and_sort_by_name() {
        let definitions = definitions();
        let candidates: Vec<usize> = (0..definitions.len()).collect();
        let names = |slot| {
            source_uniques(&candidates, &definitions, slot, &[BOOTS])
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(None), ["Ember Band", "Fractured Crown", "Wayfarer's Boots"]);
        assert_eq!(names(Some(BOOTS)), ["Wayfarer's Boots"]);
    }

    #[test]
    fn level_choices_offer_the_item_only_when_its_level_is_known() {
        assert_eq!(level_choices(None).len(), 1 + LEVEL_CAPS.len());
        let known = level_choices(Some(20));
        assert_eq!(known[1], (LevelChoice::FitsItem, "Fits this item (level 20)".to_owned()));
    }
}
