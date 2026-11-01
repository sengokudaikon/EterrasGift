//! Forge presentation. Edit intents, validation, projection and reversal use the trainer domain.

use eg_i18n::{Phrase, phrases};
use std::collections::BTreeMap;

/// Rarity bytes the game gives unique-backed items; lower bytes follow the unsealed affix count.
pub const UNIQUE: u8 = 7;
pub const SET: u8 = 8;
pub const LEGENDARY: u8 = 9;
/// The first tier that makes a crafted item exalted.
pub const EXALTED_TIER: u8 = 6;
/// The most affixes an edit leaves on an item, and on a unique-backed one.
pub const CAPACITY: usize = 32;
pub const UNIQUE_CAPACITY: usize = 7;

pub fn is_unique_backed(rarity: u8) -> bool {
    matches!(rarity, UNIQUE | SET | LEGENDARY)
}

/// A crafted item's rarity by the game's byte: none is normal, one or two magic, more rare.
pub fn crafted_name(rarity: u8) -> &'static str {
    phrases! {
        Normal: "rarity" => "Normal"
        Magic: "rarity" => "Magic"
        Rare: "rarity" => "Rare"
    }
    match rarity {
        0 => Normal.get(),
        1 | 2 => Magic.get(),
        _ => Rare.get(),
    }
}

pub use eg_trainer::item::edit::{AffixKind, ItemEdit, RollTarget, SealState as Seal};
use eg_trainer::{
    definitions::catalog::GameCatalog,
    item::edit::{self, ExpectedObservation},
};
use std::sync::Arc;

/// Complete owned domain inputs behind this presentation, including fields the UI does not show.
#[derive(Clone, Debug, PartialEq)]
pub struct ForgeContext {
    pub expected: ExpectedObservation,
    pub catalog: Arc<GameCatalog>,
}

/// The special category an affix belongs to; a slot keeps its category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Standard,
    Experimental,
    Personal,
    Set,
    IdolEnchantment,
    IdolWeaver,
    Corrupted,
    /// An affix shown as one of a unique's own modifiers.
    FakeUniqueMod,
    /// A type the game added after this build of the editor.
    Other(i32),
}

impl Category {
    pub fn as_raw(self) -> i32 {
        match self {
            Self::Standard => 0,
            Self::Experimental => 1,
            Self::Personal => 2,
            Self::Set => 3,
            Self::IdolEnchantment => 4,
            Self::IdolWeaver => 5,
            Self::Corrupted => 6,
            Self::FakeUniqueMod => 7,
            Self::Other(value) => value,
        }
    }
    pub fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Standard,
            1 => Self::Experimental,
            2 => Self::Personal,
            3 => Self::Set,
            4 => Self::IdolEnchantment,
            5 => Self::IdolWeaver,
            6 => Self::Corrupted,
            7 => Self::FakeUniqueMod,
            other => Self::Other(other),
        }
    }
    pub fn name(self) -> std::borrow::Cow<'static, str> {
        phrases! {
            Experimental: "Experimental"
            Personal: "Personal"
            Set: "Reforged set"
            IdolEnchantment: "Idol enchantment"
            IdolWeaver: "Weaver idol"
            Corrupted: "affix category" => "Corrupted"
            UniqueModifier: "Unique modifier"
            Other: "Special affix {value}"
        }
        match self {
            Category::Standard => "".into(),
            Category::Experimental => Experimental.get().into(),
            Category::Personal => Personal.get().into(),
            Category::Set => Set.get().into(),
            Category::IdolEnchantment => IdolEnchantment.get().into(),
            Category::IdolWeaver => IdolWeaver.get().into(),
            Category::Corrupted => Corrupted.get().into(),
            Category::FakeUniqueMod => UniqueModifier.get().into(),
            Category::Other(value) => Other.with(&[("value", &value)]).into(),
        }
    }
}

/// An affix as the game defines it.
#[derive(Clone, Debug, PartialEq)]
pub struct AffixDefinition {
    pub id: u16,
    pub name: String,
    pub kind: AffixKind,
    pub category: Category,
    /// The equipment types it rolls on.
    pub item_types: Vec<u16>,
    pub max_tier: u8,
    /// In the current game data with a type the editor can write (prefix or suffix); a placeholder for an ID the game
    /// data lacks is not.
    pub writable: bool,
    /// The game's class specificity: 0 for any class, bit 0 classless, bits 1–5 the classes (the game's order).
    pub class_specificity: i32,
    /// The item level it needs; a unique modifier's is its unique's.
    pub level: u8,
    /// The unique it was made from; none for an affix the game ships.
    pub source: Option<AffixSource>,
    /// Its stat tags (`picker::STAT_TAGS`), for the picker.
    pub stat_tags: u64,
    /// Each stat line in the game's words, beside the catalog's `tier_columns`.
    pub effects: Vec<AffixEffect>,
    /// The game's name, where `name` says what it does instead because the game's only says where it is from ("Ucenui's").
    pub title: Option<String>,
}

pub use eg_trainer::definitions::catalog::AffixEffect;

/// What an affix's lines say, each once.
pub fn effects_said(effects: &[AffixEffect]) -> String {
    let mut said: Vec<&str> = Vec::with_capacity(effects.len());
    for effect in effects {
        if !effect.heading.is_empty() && !said.contains(&effect.heading.as_str()) {
            said.push(&effect.heading);
        }
    }
    said.join(", ")
}

/// Where a unique modifier comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct AffixSource {
    pub unique: u16,
    pub name: String,
    /// The unique's equipment type.
    pub item_type: u16,
    /// The modifier's own words.
    pub line: String,
}

impl AffixDefinition {
    /// The tiers it has, which tell apart affixes that read the same: "T1 only", "T1–T8".
    pub fn tiers_text(&self) -> String {
        phrases! {
            OneTier: "T1 only"
            Tiers: "T1–T{max}"
        }
        if self.max_tier <= 1 { OneTier.into() } else { Tiers.with(&[("max", &self.max_tier)]) }
    }

    pub fn is_reforged(&self) -> bool {
        self.category == Category::Set
    }

    pub fn unknown(id: u16) -> AffixDefinition {
        AffixDefinition {
            id,
            name: UnknownAffix.with(&[("id", &id)]),
            kind: AffixKind::Prefix,
            category: Category::Standard,
            item_types: Vec::new(),
            max_tier: 8,
            writable: false,
            class_specificity: 0,
            level: 0,
            source: None,
            stat_tags: 0,
            effects: Vec::new(),
            title: None,
        }
    }
}

phrases! {
    DescriptionsUnavailable: "Descriptions unavailable."
    UnknownAffix: "Unknown #{id}"
}

/// A slot's kind and category as one phrase: "Prefix", "Experimental suffix", "Prefix (special affix 9)". Each is
/// whole, so a translation can order and inflect it.
pub fn kind_phrase(kind: AffixKind, category: Category) -> String {
    const EXPERIMENTAL: [Phrase; 2] = [Phrase::new("Experimental prefix"), Phrase::new("Experimental suffix")];
    const PERSONAL: [Phrase; 2] = [Phrase::new("Personal prefix"), Phrase::new("Personal suffix")];
    const SET: [Phrase; 2] = [Phrase::new("Reforged set prefix"), Phrase::new("Reforged set suffix")];
    const IDOL_ENCHANTMENT: [Phrase; 2] = [Phrase::new("Idol enchantment prefix"), Phrase::new("Idol enchantment suffix")];
    const IDOL_WEAVER: [Phrase; 2] = [Phrase::new("Weaver idol prefix"), Phrase::new("Weaver idol suffix")];
    const CORRUPTED: [Phrase; 2] = [Phrase::new("Corrupted prefix"), Phrase::new("Corrupted suffix")];
    const UNIQUE_MODIFIER: [Phrase; 2] = [Phrase::new("Unique modifier prefix"), Phrase::new("Unique modifier suffix")];
    const OTHER: [Phrase; 2] = [Phrase::new("Prefix (special affix {value})"), Phrase::new("Suffix (special affix {value})")];
    let side = kind as usize;
    match category {
        Category::Standard => kind.name().to_owned(),
        Category::Experimental => EXPERIMENTAL[side].into(),
        Category::Personal => PERSONAL[side].into(),
        Category::Set => SET[side].into(),
        Category::IdolEnchantment => IDOL_ENCHANTMENT[side].into(),
        Category::IdolWeaver => IDOL_WEAVER[side].into(),
        Category::Corrupted => CORRUPTED[side].into(),
        Category::FakeUniqueMod => UNIQUE_MODIFIER[side].into(),
        Category::Other(value) => OTHER[side].with(&[("value", &value)]),
    }
}

/// One affix the item holds. Slots are positions in the order the game loads them.
#[derive(Clone, Debug, PartialEq)]
pub struct AffixState {
    pub slot: usize,
    pub id: u16,
    pub tier: u8,
    pub roll: u8,
    pub kind: AffixKind,
    pub seal: Seal,
    pub category: Category,
    /// A value of it the editor cannot represent (its type, seal or tier): the affix is shown but not edited.
    pub unknown: Option<Unreadable>,
}

/// One line of the unique tooltip or one implicit, as the game formats it: its text, its range caption and the rolls it
/// reads.
pub use eg_trainer::backend::ModifierSnapshot;

#[derive(Clone, Debug, PartialEq)]
pub struct UniqueState {
    pub id: u16,
    pub rolls: Vec<u8>,
    pub modifiers: Vec<ModifierSnapshot>,
    /// The game could not format its own unique data.
    pub formatting: crate::pages::forge::game::Lookup<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ItemStatus {
    pub forging_potential: u8,
    pub legendary_potential: u8,
    pub weavers_will: u8,
    pub weavers_touch: u8,
    pub level_requirement: u8,
    pub corrupted: bool,
    pub ruined: bool,
}

pub use eg_trainer::item::edit::StatusField;

impl ItemStatus {
    pub fn read(&self, field: StatusField) -> u8 {
        match field {
            StatusField::ForgingPotential => self.forging_potential,
            StatusField::LegendaryPotential => self.legendary_potential,
            StatusField::WeaversWill => self.weavers_will,
            StatusField::WeaversTouch => self.weavers_touch,
            StatusField::LevelRequirement => self.level_requirement,
            StatusField::Corrupted => u8::from(self.corrupted),
            StatusField::Ruined => u8::from(self.ruined),
        }
    }
}

/// What the forge holds, as one read of the game.
#[derive(Clone, Debug, PartialEq)]
pub struct ForgeItem {
    /// Another token is another item; the same token changed by the game is the same item.
    pub token: String,
    pub context: Option<Arc<ForgeContext>>,
    pub name: String,
    pub item_type: u16,
    pub sub_type: u16,
    pub idol: bool,
    pub rarity: u8,
    pub affixes: Vec<AffixState>,
    pub unique: Option<UniqueState>,
    /// A set item's identity and rolls (the unique's move here when it becomes a set item, and back).
    pub set_item: Option<UniqueState>,
    /// None when the game did not describe the implicits.
    pub implicits: Option<Vec<ModifierSnapshot>>,
    pub implicit_rolls: Vec<u8>,
    pub status: Option<ItemStatus>,
}

impl ForgeItem {
    /// Original ForgeSnapshotComparer.StateEquals: optional tooltip text/ranges
    /// and formatting availability are presentation, never a stale-write or Undo key.
    pub fn same_state(&self, other: &Self) -> bool {
        fn identity(value: &Option<UniqueState>) -> Option<(u16, &[u8])> {
            value.as_ref().map(|state| (state.id, state.rolls.as_slice()))
        }
        self.token == other.token
            && self.name == other.name
            && self.item_type == other.item_type
            && self.sub_type == other.sub_type
            && self.idol == other.idol
            && self.rarity == other.rarity
            && self.affixes == other.affixes
            && self.implicit_rolls == other.implicit_rolls
            && self.status == other.status
            && identity(&self.unique) == identity(&other.unique)
            && identity(&self.set_item) == identity(&other.set_item)
    }
    /// Which unique or set item a unique-backed item is.
    pub fn identity_id(&self) -> Option<u16> {
        self.backing().map(|identity| identity.id)
    }

    /// The unique or set identity, with its rolls and the game's lines for them.
    pub fn backing(&self) -> Option<&UniqueState> {
        self.unique.as_ref().or(self.set_item.as_ref())
    }

    pub fn is_exalted(&self) -> bool {
        self.affixes.iter().any(|affix| affix.tier >= EXALTED_TIER)
    }

    pub fn affix(&self, slot: usize) -> Option<&AffixState> {
        self.affixes.iter().find(|affix| affix.slot == slot)
    }

    /// Free prefix and suffix slots as the game's crafting counts them.
    pub fn free_slots(&self) -> (i32, i32) {
        self.context.as_ref().map_or((0, 0), |c| edit::free_slots(c.expected.snapshot()))
    }
}

/// Where each affix went: old slot to new slot.
pub type Moves = BTreeMap<usize, usize>;

/// Two moves one after the other.
pub fn then(first: &Moves, next: &Moves) -> Moves {
    first.iter().map(|(from, to)| (*from, *next.get(to).unwrap_or(to))).collect()
}

pub fn roll_changes(changes: impl IntoIterator<Item = (usize, u8)>) -> Vec<edit::RollChange> {
    changes
        .into_iter()
        .map(|(index, value)| edit::RollChange {
            index: index as i32,
            value: i32::from(value),
        })
        .collect()
}

/// The rarity the game gives an item becoming a unique or set item: a set item is a set; with affixes, or already
/// legendary, a legendary; otherwise a unique.
pub fn identity_rarity(item: &ForgeItem, set_item: bool) -> u8 {
    item.context.as_ref().map_or(UNIQUE, |context| edit::rarity_for(context.expected.snapshot(), set_item))
}

/// Presentation adapters call the same planner as the worker. No edit rules live here.
pub trait EditPresentation {
    fn prepare_view_with_override(&self, before: &ForgeItem, override_enabled: bool) -> Result<edit::PreparedEdit, edit::EditError>;
    fn prepare_view(&self, before: &ForgeItem) -> Result<edit::PreparedEdit, edit::EditError> {
        // Preview an explicit override intent; submission rechecks permission.
        self.prepare_view_with_override(before, true)
    }
    fn project(&self, before: &ForgeItem) -> Result<ForgeItem, edit::EditError>;
    fn is_valid(&self, before: &ForgeItem) -> bool {
        self.prepare_view(before).is_ok()
    }
    fn can_reverse(&self, before: &ForgeItem) -> bool {
        self.prepare_view(before)
            .ok()
            .and_then(|p| {
                let reverse = eg_trainer::item::undo::ForgeUndo::reverse_intent(&p, p.projected()).ok()?;
                let after = before.with_snapshot(p.projected().clone()).ok()?;
                Some(reverse.is_valid(&after))
            })
            .unwrap_or(false)
    }
    fn moves(&self, before: &ForgeItem) -> Moves {
        self.prepare_view(before)
            .map(|p| p.moves().iter().map(|(a, b)| (usize::from(*a), usize::from(*b))).collect())
            .unwrap_or_default()
    }
}

impl EditPresentation for ItemEdit {
    fn prepare_view_with_override(&self, before: &ForgeItem, override_enabled: bool) -> Result<edit::PreparedEdit, edit::EditError> {
        let context = before.context.as_ref().ok_or(edit::EditError::InvalidObservation)?;
        let bindings = |lines: Option<&[ModifierSnapshot]>| match lines {
            Some(lines) => eg_trainer::definitions::catalog::Availability::Available(
                lines
                    .iter()
                    .map(|m| edit::RollBinding {
                        editable: m.editable,
                        roll_ids: m.roll_ids.iter().map(|i| *i as i32).collect(),
                    })
                    .collect(),
            ),
            None => eg_trainer::definitions::catalog::Availability::Unavailable {
                reason: DescriptionsUnavailable.into(),
            },
        };
        let evidence = edit::RollEvidence::from_runtime(
            &context.expected,
            bindings(before.backing().map(|u| u.modifiers.as_slice())),
            bindings(before.implicits.as_deref()),
        );
        edit::prepare(&context.expected, &context.catalog, self.clone(), override_enabled, Some(&evidence))
    }
    fn project(&self, before: &ForgeItem) -> Result<ForgeItem, edit::EditError> {
        let prepared = self.prepare_view(before)?;
        before.with_snapshot(prepared.projected().clone())
    }
}

impl ForgeItem {
    pub(crate) fn observed(context: Arc<ForgeContext>, name: String) -> Self {
        let expected = &context.expected;
        let process = expected.process();
        let snapshot = expected.snapshot();
        let mut item = Self {
            token: format!("{}:{}:{}:{}:{}", process.pid, process.created, expected.generation(), snapshot.item, snapshot.individual_id),
            name,
            item_type: 0,
            sub_type: 0,
            idol: false,
            rarity: 0,
            affixes: vec![],
            implicit_rolls: vec![],
            implicits: None,
            unique: None,
            set_item: None,
            status: None,
            context: Some(context.clone()),
        };
        item.copy_snapshot(snapshot);
        item
    }
    pub fn with_snapshot(&self, snapshot: eg_trainer::backend::ForgeSnapshot) -> Result<Self, edit::EditError> {
        let context = self.context.as_ref().ok_or(edit::EditError::InvalidObservation)?;
        let mut after = self.clone();
        after.copy_snapshot(&snapshot);
        after.context = Some(Arc::new(ForgeContext {
            expected: ExpectedObservation::new(
                context.expected.generation(),
                context.expected.process().clone(),
                context.expected.catalog_identity().clone(),
                snapshot,
            )?,
            catalog: context.catalog.clone(),
        }));
        Ok(after)
    }
    /// Takes every value the game reported. One the editor cannot represent is noted on its affix, never a reason to
    /// hide the item; the worker's planner refuses edits that would write it.
    fn copy_snapshot(&mut self, snapshot: &eg_trainer::backend::ForgeSnapshot) {
        self.item_type = u16::from(snapshot.item_type);
        self.idol = (25..=33).contains(&snapshot.item_type);
        self.sub_type = snapshot.sub_type;
        self.rarity = snapshot.rarity;
        self.affixes = snapshot
            .affixes
            .iter()
            .map(|a| {
                let tier = u8::try_from(a.tier).ok();
                let kind = match a.kind {
                    0 => Some(AffixKind::Prefix),
                    1 => Some(AffixKind::Suffix),
                    _ => None,
                };
                let seal = usize::try_from(a.seal).ok().and_then(|seal| Seal::ALL.get(seal).copied());
                let unknown = if tier.is_none() {
                    Some(Unreadable {
                        field: "tier",
                        value: a.tier.into(),
                    })
                } else if kind.is_none() {
                    Some(Unreadable {
                        field: "type",
                        value: a.kind.into(),
                    })
                } else if seal.is_none() {
                    Some(Unreadable {
                        field: "seal",
                        value: a.seal.into(),
                    })
                } else {
                    None
                };
                AffixState {
                    slot: usize::from(a.slot),
                    id: a.id,
                    tier: tier.unwrap_or(u8::MAX),
                    roll: a.roll,
                    kind: kind.unwrap_or(AffixKind::Prefix),
                    seal: seal.unwrap_or(Seal::Normal),
                    category: Category::from_raw(a.special),
                    unknown,
                }
            })
            .collect();
        self.implicit_rolls = snapshot.implicit_rolls.clone();
        let identity = snapshot.identity.as_ref().map(|i| {
            let mut view = self
                .unique
                .as_ref()
                .or(self.set_item.as_ref())
                .filter(|old| old.id == i.id)
                .cloned()
                .unwrap_or(UniqueState {
                    id: i.id,
                    rolls: vec![],
                    modifiers: vec![],
                    formatting: crate::pages::forge::game::Lookup::Ready(()),
                });
            view.rolls = i.rolls.clone();
            view
        });
        if snapshot.rarity == SET {
            self.set_item = identity;
            self.unique = None;
        } else {
            self.unique = identity;
            self.set_item = None;
        }
        let status = &snapshot.status;
        self.status = Some(ItemStatus {
            forging_potential: status.forging_potential,
            legendary_potential: status.legendary_potential,
            weavers_will: status.weavers_will,
            weavers_touch: status.weavers_touch,
            // The game recalculates it on reload; a value past the field's range shows at its end.
            level_requirement: status.level_requirement.clamp(0, 255) as u8,
            corrupted: status.corrupted,
            ruined: status.ruined,
        });
    }
}

/// A value of an affix read from the game that the editor cannot represent, e.g. "seal 7".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable {
    pub field: &'static str,
    pub value: i64,
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        phrases! {
            Tier: "Unknown tier {value}"
            Type: "Unknown type {value}"
            Seal: "Unknown seal {value}"
        }
        let phrase = match self.field {
            "tier" => Tier,
            "type" => Type,
            _ => Seal,
        };
        f.write_str(&phrase.with(&[("value", &self.value)]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compatibility {
    Allowed,
    /// Allowed only by "I know what I'm doing": the game would not roll it here.
    Override,
    Rejected(Rejection),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    ClassRequirement,
    SlotType,
    EquipmentType,
    SpecialCategory,
    AlreadyPresent,
    /// The game data lacks the affix's equipment types, or holds a value the rules don't know.
    IncompleteData,
}

impl Compatibility {
    pub fn is_allowed(self) -> bool {
        matches!(self, Compatibility::Allowed | Compatibility::Override)
    }

    /// Writable with the picker's "Allow incompatible affixes".
    pub fn allows_edit(self, allow_incompatible: bool) -> bool {
        self.is_allowed() || (allow_incompatible && self != Compatibility::Unknown)
    }
}

/// Both authored fixtures and production evaluate the canonical class/category/idol rules.
pub fn compatibility(item: &ForgeItem, definition: &AffixDefinition, slot: Option<&AffixState>, unrestricted: bool) -> Compatibility {
    use edit::CompatibilityReason as Reason;
    let Some(context) = &item.context else { return Compatibility::Unknown };
    let Some(definition) = context.catalog.affixes.get(&definition.id) else {
        return Compatibility::Unknown;
    };
    let snapshot = context.expected.snapshot();
    let slot = slot.and_then(|s| snapshot.affixes.iter().find(|a| usize::from(a.slot) == s.slot));
    match edit::compatibility(&context.catalog, snapshot, definition, slot, None, unrestricted).reason {
        Reason::None => Compatibility::Allowed,
        Reason::Override => Compatibility::Override,
        Reason::ClassRequirement => Compatibility::Rejected(Rejection::ClassRequirement),
        Reason::SlotType => Compatibility::Rejected(Rejection::SlotType),
        Reason::EquipmentType => Compatibility::Rejected(Rejection::EquipmentType),
        Reason::SpecialCategory => Compatibility::Rejected(Rejection::SpecialCategory),
        Reason::AlreadyPresent => Compatibility::Rejected(Rejection::AlreadyPresent),
        Reason::MissingMetadata | Reason::UnknownGameValue => Compatibility::Rejected(Rejection::IncompleteData),
    }
}

/// The slots of the affixes a base change would make incompatible: allowed on the item now but not once it is
/// `item_type`'s `sub_type` with that base's `class_requirement`. Incompatibility the item already carries is not the new
/// base's doing, and the override plays no part.
pub fn broken_by_base(item: &ForgeItem, item_type: u16, sub_type: u16, class_requirement: Option<i32>) -> Vec<usize> {
    let Some(context) = &item.context else { return Vec::new() };
    let catalog = &context.catalog;
    let now = context.expected.snapshot();
    let rebased = eg_trainer::backend::ForgeSnapshot {
        item_type: u8::try_from(item_type).unwrap_or(now.item_type),
        sub_type,
        class_requirement: class_requirement.unwrap_or(now.class_requirement),
        ..now.clone()
    };
    // Each affix is judged alone, without the other affixes' definitions: by the item's own class requirement only.
    let allowed = |item: &eg_trainer::backend::ForgeSnapshot, affix: &eg_trainer::backend::AffixSnapshot, definition| {
        let alone = eg_trainer::backend::ForgeSnapshot {
            affixes: vec![affix.clone()],
            ..item.clone()
        };
        edit::compatibility(catalog, &alone, definition, Some(affix), Some(item.class_requirement), false).allowed
    };
    now.affixes
        .iter()
        // A duplicate is rejected on either base.
        .filter(|affix| !now.affixes.iter().any(|other| other.id == affix.id && other.slot != affix.slot))
        .filter(|affix| {
            catalog
                .affixes
                .get(&affix.id)
                .is_some_and(|definition| allowed(now, affix, definition) && !allowed(&rebased, affix, definition))
        })
        .map(|affix| usize::from(affix.slot))
        .collect()
}

/// A tier's value range from the captured numeric data, the way the game's tooltips show it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TierProperty {
    /// Modifier type 0, rounded to whole numbers when `whole`.
    Flat { min: f64, max: f64, whole: bool },
    /// Modifier type 1, fractions shown as percentages.
    Increased { min: f64, max: f64 },
    /// Modifier type 0 shown as a percentage.
    AddedPercent { min: f64, max: f64 },
}

impl TierProperty {
    /// As the game shows it on an item that scales affix values by `scale`.
    pub fn scaled(self, scale: f64) -> Self {
        match self {
            Self::Flat { min, max, whole } => Self::Flat {
                min: min * scale,
                max: max * scale,
                whole,
            },
            Self::Increased { min, max } => Self::Increased {
                min: min * scale,
                max: max * scale,
            },
            Self::AddedPercent { min, max } => Self::AddedPercent {
                min: min * scale,
                max: max * scale,
            },
        }
    }

    /// Its size without its sign, for a line whose words already say "reduced" or "less".
    pub fn unsigned(self) -> Self {
        match self {
            Self::Increased { min, max } if max <= 0.0 => Self::Increased { min: -max, max: -min },
            other => other,
        }
    }
}

pub fn format_tier_range(properties: &[TierProperty]) -> Option<String> {
    let trim = |value: f64| eg_i18n::locale::numbers().plain(value);
    let percent = |value: f64| trim((value * 1000.0).round() / 10.0);
    fn span(min: String, max: String) -> String {
        if min == max { min } else { format!("{min}–{max}") }
    }
    if properties.is_empty() {
        return None;
    }
    let parts: Option<Vec<String>> = properties
        .iter()
        .map(|property| match *property {
            TierProperty::Increased { min, max } if min.is_finite() && max.is_finite() => Some(span(percent(min), percent(max)) + "%"),
            TierProperty::AddedPercent { min, max } if min.is_finite() && max.is_finite() => Some(format!("+{}%", span(percent(min), percent(max)))),
            TierProperty::Flat { min, max, whole } if min.is_finite() && max.is_finite() => {
                let number = |value: f64| trim(if whole { value.round() } else { value });
                Some(format!("+{}", span(number(min), number(max))))
            }
            _ => None,
        })
        .collect();
    parts.map(|parts| parts.join(" / ").replace("+-", "-"))
}

/// One roll position's value in the game's own words.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueSample {
    pub value: f64,
    pub display: String,
    pub effects: Vec<String>,
}

/// The values the game produces for one roll, a sample per position 0–255.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueTable {
    pub label: String,
    pub samples: Vec<ValueSample>,
}

phrases! {
    RollPosition: "Roll position"
    Roll: "Roll {roll}"
}

thread_local! {
    static POSITIONS: std::rc::Rc<ValueTable> = std::rc::Rc::new(ValueTable {
        label: RollPosition.into(),
        samples: (0..=255)
            .map(|roll| ValueSample {
                value: f64::from(roll),
                display: Roll.with(&[("roll", &roll)]),
                effects: Vec::new(),
            })
            .collect(),
    });
}

impl ValueTable {
    /// The roll positions themselves, 0 to 255: what an affix's value cell edits when the game can't say what its rolls
    /// are worth. Built once and shared.
    pub fn positions() -> std::rc::Rc<ValueTable> {
        POSITIONS.with(std::rc::Rc::clone)
    }

    /// 256 finite samples, all one value: the roll changes nothing.
    pub fn is_fixed(&self) -> bool {
        let samples = &self.samples;
        samples.len() == 256 && samples[0].value.is_finite() && samples.iter().all(|sample| sample.value == samples[0].value)
    }

    /// 256 finite samples, at least two distinct values, monotonic.
    pub fn is_usable(&self) -> bool {
        let samples = &self.samples;
        if samples.len() != 256 || samples.iter().any(|sample| !sample.value.is_finite()) {
            return false;
        }
        let distinct = samples.iter().any(|sample| sample.value != samples[0].value);
        let rising = samples.windows(2).all(|pair| pair[0].value <= pair[1].value);
        let falling = samples.windows(2).all(|pair| pair[0].value >= pair[1].value);
        distinct && (rising || falling)
    }

    pub fn minimum(&self) -> &ValueSample {
        self.samples.iter().min_by(|a, b| a.value.total_cmp(&b.value)).expect("a usable table has samples")
    }

    pub fn maximum(&self) -> &ValueSample {
        self.samples.iter().max_by(|a, b| a.value.total_cmp(&b.value)).expect("a usable table has samples")
    }

    /// The roll with the closest sample; ties go to the roll closest to the current one, then to the lower roll.
    pub fn nearest(&self, value: f64, current: u8) -> u8 {
        let mut best = 0usize;
        for (roll, sample) in self.samples.iter().enumerate() {
            let distance = (sample.value - value).abs();
            let best_distance = (self.samples[best].value - value).abs();
            let closer = distance < best_distance || (distance == best_distance && (roll as i32 - current as i32).abs() < (best as i32 - current as i32).abs());
            if closer {
                best = roll;
            }
        }
        best as u8
    }
}

/// What a value table is asked for: a roll and the draft it is read in.
#[derive(Clone, Debug, PartialEq)]
pub struct RollRequest {
    pub target: RollTarget,
    /// The affix slot, or the roll index.
    pub index: usize,
    /// For an affix: the drafted id, tier and seal. For a unique roll: the other drafted rolls.
    pub draft: Option<ItemEdit>,
    /// For a unique roll: the tooltip line it maps through.
    pub line: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn affix(slot: usize, id: u16, seal: Seal) -> AffixState {
        AffixState {
            slot,
            id,
            tier: 4,
            roll: 100,
            kind: AffixKind::Suffix,
            seal,
            category: Category::Standard,
            unknown: None,
        }
    }

    fn item(affixes: Vec<AffixState>) -> ForgeItem {
        ForgeItem {
            token: "t".into(),
            context: None,
            name: "Boots".into(),
            item_type: 1,
            sub_type: 0,
            idol: false,
            rarity: 0,
            affixes,
            unique: None,
            implicits: None,
            implicit_rolls: Vec::new(),
            set_item: None,
            status: None,
        }
    }

    #[test]
    fn item_state_ignores_optional_tooltips_but_keeps_identity_and_roll_changes() {
        let before = item(vec![affix(0, 25, Seal::Normal)]);
        let mut formatted = before.clone();
        formatted.implicits = Some(vec![ModifierSnapshot {
            index: 0,
            text: "Armour +5".into(),
            range: "[5 to 5]".into(),
            roll_ids: vec![],
            editable: true,
        }]);
        assert_ne!(before, formatted);
        assert!(before.same_state(&formatted));
        formatted.implicit_rolls.push(1);
        assert!(!before.same_state(&formatted));
        let mut unique = before.clone();
        unique.unique = Some(UniqueState {
            id: 7,
            rolls: vec![12],
            modifiers: vec![],
            formatting: crate::pages::forge::game::Lookup::Ready(()),
        });
        let mut error = unique.clone();
        error.unique.as_mut().unwrap().formatting = crate::pages::forge::game::Lookup::Unavailable("unavailable".into());
        assert!(unique.same_state(&error));
        error.unique.as_mut().unwrap().id = 8;
        assert!(!unique.same_state(&error));
        error = unique.clone();
        error.unique.as_mut().unwrap().rolls[0] = 13;
        assert!(!unique.same_state(&error));
    }

    #[test]
    fn tier_ranges_read_as_the_game_shows_them() {
        assert_eq!(
            format_tier_range(&[TierProperty::Flat {
                min: 61.0,
                max: 90.0,
                whole: true
            }])
            .as_deref(),
            Some("+61–90")
        );
        assert_eq!(format_tier_range(&[TierProperty::Increased { min: 0.06, max: 0.09 }]).as_deref(), Some("6–9%"));
        assert_eq!(format_tier_range(&[TierProperty::AddedPercent { min: 0.2, max: 0.24 }]).as_deref(), Some("+20–24%"));
        assert_eq!(format_tier_range(&[TierProperty::Flat { min: 5.0, max: 5.0, whole: true }]).as_deref(), Some("+5"));
        assert_eq!(format_tier_range(&[]), None);
    }

    #[test]
    fn nearest_prefers_the_current_roll_then_the_lower() {
        let table = ValueTable {
            label: "Value".into(),
            samples: (0..256)
                .map(|roll| ValueSample {
                    value: (roll / 2) as f64,
                    display: String::new(),
                    effects: Vec::new(),
                })
                .collect(),
        };
        assert!(table.is_usable());
        assert_eq!(table.nearest(10.0, 21), 21);
        assert_eq!(table.nearest(10.0, 0), 20);
    }
}
