//! Forge presentation. Edit intents, validation, projection and reversal use the trainer domain.

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
    match rarity {
        0 => "Normal",
        1 | 2 => "Magic",
        _ => "Rare",
    }
}

pub use eg_trainer::edits::{AffixKind, ItemEdit, RollTarget, SealState as Seal};
use eg_trainer::{
    catalog::GameCatalog,
    edits::{self, ExpectedObservation},
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
    Corrupted,
}

impl Category {
    pub fn as_raw(self) -> i32 {
        match self {
            Self::Standard => 0,
            Self::Experimental => 1,
            Self::Personal => 2,
            Self::Set => 3,
            Self::Corrupted => 6,
        }
    }
    pub fn from_raw(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Standard,
            1 => Self::Experimental,
            2 => Self::Personal,
            3 => Self::Set,
            6 => Self::Corrupted,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Category::Standard => "",
            Category::Experimental => "Experimental",
            Category::Personal => "Personal",
            Category::Set => "Set · Reforged",
            Category::Corrupted => "Corrupted",
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
    /// Read from the running game (RuntimeVerified); only such definitions can be written.
    pub verified: bool,
}

impl AffixDefinition {
    /// "type · category · class", e.g. "Prefix · Set · Reforged".
    pub fn kind_description(&self) -> String {
        join_parts(&[self.kind.name(), self.category.name()])
    }

    pub fn is_reforged(&self) -> bool {
        self.category == Category::Set
    }

    pub fn unknown(id: u16) -> AffixDefinition {
        AffixDefinition {
            id,
            name: format!("Unknown #{id}"),
            kind: AffixKind::Prefix,
            category: Category::Standard,
            item_types: Vec::new(),
            max_tier: 8,
            verified: false,
        }
    }
}

pub fn join_parts(parts: &[&str]) -> String {
    parts.iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" · ")
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
}

/// One line of the unique tooltip or one implicit: its text, its range caption and the rolls it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ModifierState {
    pub index: usize,
    pub text: String,
    pub range: String,
    pub roll_ids: Vec<usize>,
    pub editable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniqueState {
    pub id: u16,
    pub rolls: Vec<u8>,
    pub modifiers: Vec<ModifierState>,
    /// The game could not format its own unique data.
    pub formatting: crate::forge::game::Lookup<()>,
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

pub use eg_trainer::edits::StatusField;

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
    pub implicits: Option<Vec<ModifierState>>,
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
        self.unique.as_ref().or(self.set_item.as_ref()).map(|identity| identity.id)
    }

    pub fn is_exalted(&self) -> bool {
        self.affixes.iter().any(|affix| affix.tier >= EXALTED_TIER)
    }

    pub fn affix(&self, slot: usize) -> Option<&AffixState> {
        self.affixes.iter().find(|affix| affix.slot == slot)
    }

    /// Free prefix and suffix slots as the game's crafting counts them (spec §15.1.8).
    pub fn free_slots(&self) -> (i32, i32) {
        self.context.as_ref().map_or((0, 0), |c| edits::free_slots(c.expected.snapshot()))
    }
}

/// Where each affix went: old slot to new slot.
pub type Moves = BTreeMap<usize, usize>;

/// Two moves one after the other.
pub fn then(first: &Moves, next: &Moves) -> Moves {
    first.iter().map(|(from, to)| (*from, *next.get(to).unwrap_or(to))).collect()
}

pub fn roll_changes(changes: impl IntoIterator<Item = (usize, u8)>) -> Vec<edits::RollChange> {
    changes
        .into_iter()
        .map(|(index, value)| edits::RollChange {
            index: index as i32,
            value: i32::from(value),
        })
        .collect()
}

/// The rarity the game gives an item becoming a unique or set item: a set item is a set; with affixes, or already
/// legendary, a legendary; otherwise a unique.
pub fn identity_rarity(item: &ForgeItem, set_item: bool) -> u8 {
    item.context.as_ref().map_or(UNIQUE, |context| edits::rarity_for(context.expected.snapshot(), set_item))
}

/// Presentation adapters call the same planner as the worker. No edit rules live here.
pub trait EditPresentation {
    fn prepare_view_with_override(&self, before: &ForgeItem, override_enabled: bool) -> Result<edits::PreparedEdit, edits::EditError>;
    fn prepare_view(&self, before: &ForgeItem) -> Result<edits::PreparedEdit, edits::EditError> {
        // Preview an explicit override intent; submission rechecks permission.
        self.prepare_view_with_override(before, true)
    }
    fn project(&self, before: &ForgeItem) -> Result<ForgeItem, edits::EditError>;
    fn is_valid(&self, before: &ForgeItem) -> bool {
        self.prepare_view(before).is_ok()
    }
    fn can_reverse(&self, before: &ForgeItem) -> bool {
        self.prepare_view(before)
            .ok()
            .and_then(|p| {
                let reverse = eg_trainer::undo::ForgeUndo::reverse_intent(&p, p.projected()).ok()?;
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
    fn prepare_view_with_override(&self, before: &ForgeItem, override_enabled: bool) -> Result<edits::PreparedEdit, edits::EditError> {
        let context = before.context.as_ref().ok_or(edits::EditError::InvalidObservation)?;
        let bindings = |lines: Option<&[ModifierState]>| match lines {
            Some(lines) => eg_trainer::catalog::Availability::Available(
                lines
                    .iter()
                    .map(|m| edits::RollBinding {
                        editable: m.editable,
                        roll_ids: m.roll_ids.iter().map(|i| *i as i32).collect(),
                    })
                    .collect(),
            ),
            None => eg_trainer::catalog::Availability::Unavailable {
                reason: "Descriptions unavailable.".into(),
            },
        };
        let evidence = edits::RollEvidence::from_runtime(
            &context.expected,
            bindings(before.unique.as_ref().map(|u| u.modifiers.as_slice())),
            bindings(before.implicits.as_deref()),
        );
        edits::prepare(
            &context.expected,
            &context.catalog,
            &edits::CatalogProvenance::RuntimeRead {
                generation: context.expected.generation(),
                process: context.expected.process().clone(),
            },
            self.clone(),
            override_enabled,
            Some(&evidence),
        )
    }
    fn project(&self, before: &ForgeItem) -> Result<ForgeItem, edits::EditError> {
        let prepared = self.prepare_view(before)?;
        before.with_snapshot(prepared.projected().clone())
    }
}

impl ForgeItem {
    pub fn with_snapshot(&self, snapshot: eg_trainer::backend::ForgeSnapshot) -> Result<Self, edits::EditError> {
        let context = self.context.as_ref().ok_or(edits::EditError::InvalidObservation)?;
        let mut after = self.clone();
        after.item_type = u16::from(snapshot.item_type);
        after.sub_type = snapshot.sub_type;
        after.rarity = snapshot.rarity;
        after.affixes = snapshot
            .affixes
            .iter()
            .map(|a| {
                Ok(AffixState {
                    slot: usize::from(a.slot),
                    id: a.id,
                    tier: u8::try_from(a.tier).map_err(|_| edits::EditError::InvalidValues)?,
                    roll: a.roll,
                    kind: match a.kind {
                        0 => AffixKind::Prefix,
                        1 => AffixKind::Suffix,
                        _ => return Err(edits::EditError::InvalidValues),
                    },
                    seal: *Seal::ALL.get(a.seal as usize).ok_or(edits::EditError::InvalidValues)?,
                    category: Category::from_raw(a.special).ok_or(edits::EditError::InvalidValues)?,
                })
            })
            .collect::<Result<_, _>>()?;
        after.implicit_rolls = snapshot.implicit_rolls.clone();
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
                    formatting: crate::forge::game::Lookup::Ready(()),
                });
            view.rolls = i.rolls.clone();
            view
        });
        if snapshot.rarity == SET {
            after.set_item = identity;
            after.unique = None;
        } else {
            after.unique = identity;
            after.set_item = None;
        }
        let status = &snapshot.status;
        after.status = Some(ItemStatus {
            forging_potential: status.forging_potential,
            legendary_potential: status.legendary_potential,
            weavers_will: status.weavers_will,
            weavers_touch: status.weavers_touch,
            level_requirement: u8::try_from(status.level_requirement).map_err(|_| edits::EditError::InvalidStatus)?,
            corrupted: status.corrupted,
            ruined: status.ruined,
        });
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
    use edits::CompatibilityReason as Reason;
    let Some(context) = &item.context else { return Compatibility::Unknown };
    let Some(definition) = context.catalog.affixes.get(&definition.id) else {
        return Compatibility::Unknown;
    };
    let snapshot = context.expected.snapshot();
    let slot = slot.and_then(|s| snapshot.affixes.iter().find(|a| usize::from(a.slot) == s.slot));
    match edits::compatibility(&context.catalog, snapshot, definition, slot, None, unrestricted).reason {
        Reason::None => Compatibility::Allowed,
        Reason::Override => Compatibility::Override,
        Reason::ClassRequirement => Compatibility::Rejected(Rejection::ClassRequirement),
        Reason::SlotType => Compatibility::Rejected(Rejection::SlotType),
        Reason::EquipmentType => Compatibility::Rejected(Rejection::EquipmentType),
        Reason::SpecialCategory => Compatibility::Rejected(Rejection::SpecialCategory),
        Reason::AlreadyPresent => Compatibility::Rejected(Rejection::AlreadyPresent),
        Reason::MissingMetadata | Reason::UnknownGameValue => Compatibility::Unknown,
    }
}

/// A tier's value range from the captured numeric data, the way the game's tooltips show it (spec §15.7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TierProperty {
    /// Modifier type 0, rounded to whole numbers when `whole`.
    Flat { min: f64, max: f64, whole: bool },
    /// Modifier type 1, fractions shown as percentages.
    Increased { min: f64, max: f64 },
    /// Modifier type 0 shown as a percentage.
    AddedPercent { min: f64, max: f64 },
}

pub fn format_tier_range(properties: &[TierProperty]) -> Option<String> {
    fn trim(value: f64) -> String {
        let text = format!("{value:.2}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        if text == "-0" { "0".into() } else { text.into() }
    }
    fn percent(value: f64) -> String {
        trim((value * 1000.0).round() / 10.0)
    }
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
                let number = |value: f64| if whole { format!("{}", value.round()) } else { trim((value * 100.0).round() / 100.0) };
                Some(format!("+{}", span(number(min), number(max))))
            }
            _ => None,
        })
        .collect();
    parts.map(|parts| parts.join(" / "))
}

/// One roll position's value in the game's own words.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueSample {
    pub value: f64,
    pub display: String,
    pub effects: Vec<String>,
}

/// The values the game produces for one roll, a sample per position 0–255 (spec §15.5).
#[derive(Clone, Debug, PartialEq)]
pub struct ValueTable {
    pub label: String,
    pub samples: Vec<ValueSample>,
}

impl ValueTable {
    /// 256 finite samples, at least two distinct values, monotonic.
    pub fn is_usable(&self) -> bool {
        if self.samples.len() != 256 || self.samples.iter().any(|sample| !sample.value.is_finite()) {
            return false;
        }
        let values: Vec<f64> = self.samples.iter().map(|sample| sample.value).collect();
        let distinct = values.iter().any(|value| *value != values[0]);
        let rising = values.windows(2).all(|pair| pair[0] <= pair[1]);
        let falling = values.windows(2).all(|pair| pair[0] >= pair[1]);
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
        formatted.implicits = Some(vec![ModifierState {
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
            formatting: crate::forge::game::Lookup::Ready(()),
        });
        let mut error = unique.clone();
        error.unique.as_mut().unwrap().formatting = crate::forge::game::Lookup::Unavailable("unavailable".into());
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
