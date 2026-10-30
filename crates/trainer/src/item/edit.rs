//! Owned forge edit planning. This module performs no I/O and authorizes no
//! native write. A prepared edit must still pass the process-bound log/runtime
//! gates, exact helper admission, main-thread stale check, and strict readback.
//! Uncertain writes must never be retried.
//!
//! Every rebuilding operation keeps affix IDs within the 12 bits the game saves,
//! and no result may exceed the 32 affixes a reader accepts, even when the
//! observed layout is already malformed.

use crate::definitions::catalog::{CatalogIdentity, GameCatalog};
use eg_core::phrase::Phrase;
use eg_trainer_wire::{
    catalog::{AffixDefinition, Availability},
    forge::{AffixSnapshot, ForgeSnapshot},
    process::ProcessIdentity,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const MAX_WRITABLE_AFFIX_ID: i32 = 4095;
pub const MAX_ROLL_CHANGES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
pub enum SealState {
    Normal = 0,
    Sealed = 1,
    Primordial = 2,
    Corrupted = 3,
}
impl SealState {
    pub const ALL: [Self; 4] = [Self::Normal, Self::Sealed, Self::Primordial, Self::Corrupted];
    pub fn name(self) -> &'static str {
        const NAMES: [Phrase; 4] = [
            Phrase::ctx("seal", "Normal"),
            Phrase::ctx("seal", "Sealed"),
            Phrase::ctx("seal", "Primordial"),
            Phrase::ctx("seal", "Corrupted"),
        ];
        NAMES[self as usize].get()
    }
    pub fn index(self) -> usize {
        self as usize
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum AffixKind {
    Prefix = 0,
    Suffix = 1,
}
impl AffixKind {
    pub fn name(self) -> &'static str {
        const NAMES: [Phrase; 2] = [Phrase::new("Prefix"), Phrase::new("Suffix")];
        NAMES[self as usize].get()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum RollTarget {
    Affix = 0,
    Unique = 1,
    Implicit = 2,
}
pub use eg_trainer_wire::write::StatusField;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixEdit {
    pub slot: i32,
    pub id: i32,
    pub tier: i32,
    pub roll: i32,
    pub seal: SealState,
    pub allow_incompatible: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddAffixEdit {
    pub id: i32,
    pub tier: i32,
    pub roll: i32,
    pub seal: SealState,
    pub kind: AffixKind,
    pub category: Option<i32>,
    pub allow_incompatible: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveAffixEdit {
    pub slot: i32,
    pub id: i32,
    /// The legendary potential a legendary gets back when this removal leaves it a unique: Undo of the upgrade an
    /// added affix made. None otherwise.
    pub restore_potential: Option<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollChange {
    pub index: i32,
    pub value: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollEdit {
    pub target: RollTarget,
    pub changes: Vec<RollChange>,
    pub allow_incompatible: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEdit {
    pub field: StatusField,
    pub value: i32,
    pub unrestricted: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseTypeEdit {
    pub sub_type: i32,
    pub allow_incompatible: bool,
    pub item_type: Option<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueIdentityEdit {
    pub unique_id: i32,
    pub sub_type: i32,
    pub rarity: i32,
    pub unrestricted: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemEdit {
    Affix(AffixEdit),
    Roll(RollEdit),
    Status(StatusEdit),
    BaseType(BaseTypeEdit),
    AddAffix(AddAffixEdit),
    RemoveAffix(RemoveAffixEdit),
    UniqueIdentity(UniqueIdentityEdit),
}

impl ItemEdit {
    pub fn needs_override(&self) -> bool {
        match self {
            Self::Affix(e) => e.allow_incompatible,
            Self::Roll(e) => e.allow_incompatible,
            Self::Status(e) => e.unrestricted,
            Self::BaseType(e) => e.allow_incompatible || e.item_type.is_some(),
            Self::AddAffix(e) => e.allow_incompatible,
            Self::RemoveAffix(_) => false,
            Self::UniqueIdentity(e) => e.unrestricted,
        }
    }
    /// Carry slot references through earlier edits; an absent move keeps its label.
    /// The next preparation still rejects a missing slot.
    pub fn follow(&self, moves: &BTreeMap<u8, u8>) -> Self {
        let moved = |slot: i32| u8::try_from(slot).ok().and_then(|s| moves.get(&s)).map_or(slot, |s| i32::from(*s));
        match self {
            Self::Affix(e) => Self::Affix(AffixEdit { slot: moved(e.slot), ..e.clone() }),
            Self::RemoveAffix(e) => Self::RemoveAffix(RemoveAffixEdit { slot: moved(e.slot), ..e.clone() }),
            Self::Roll(e) if e.target == RollTarget::Affix => Self::Roll(RollEdit {
                changes: e
                    .changes
                    .iter()
                    .map(|c| RollChange {
                        index: moved(c.index),
                        value: c.value,
                    })
                    .collect(),
                ..e.clone()
            }),
            _ => self.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidObservation,
    CatalogNotVerified,
    IdentityChanged,
    StaleItem,
    OfflineRequired,
    OverrideRequired,
    InvalidValues,
    MissingDefinition,
    Incompatible(CompatibilityReason),
    TierUnavailable,
    NewLayoutProblem(LayoutProblem),
    MissingRollEvidence,
    InvalidRollMapping,
    InvalidStatus,
    InvalidBase(BaseProblem),
    InvalidUniqueIdentity(IdentityProblem),
    AffixIdNotSerializable(u16),
    ProjectedSnapshotOutOfBounds,
}
/// Why a base change does not fit the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseProblem {
    /// Unique, set and legendary items keep their base.
    UniqueBacked,
    Unchanged,
    /// Only an idol changes size, to another idol.
    OnlyIdols,
    /// The base is not in the game's data, or the request is malformed.
    Unknown,
}
/// Why a unique or set identity does not fit the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityProblem {
    /// Only unique, set and legendary items have an identity to change.
    NotUniqueBacked,
    Unchanged,
    /// A set item holds no affixes.
    SetHoldsAffixes,
    /// The item would keep legendary potential or Weaver's Will the identity never holds, or become a legendary the game
    /// never drops; the override allows it.
    NeedsOverride {
        set: bool,
    },
    /// Another base, subtype or rarity than the identity has.
    Mismatch,
}
impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidObservation => "The item could not be read reliably. Refresh before editing.",
            Self::CatalogNotVerified => "Read the game definitions before editing this item.",
            Self::IdentityChanged => "The process, character or game definitions changed. Refresh before editing.",
            Self::StaleItem => "The item changed. Discard the stale draft and edit it again.",
            Self::OfflineRequired => "Load an offline character to continue.",
            Self::OverrideRequired => "This edit requires \"I know what I'm doing\" in Settings. Nothing was applied.",
            Self::InvalidValues => "Enter valid item values and integer roll positions from 0 to 255.",
            Self::MissingDefinition => "This ID has no verified definition in the current build.",
            Self::Incompatible(reason) => match reason {
                CompatibilityReason::ClassRequirement => "This affix belongs to another class.",
                CompatibilityReason::SlotType => "This affix belongs in the other slot (prefix or suffix).",
                CompatibilityReason::EquipmentType => "This affix rolls on another kind of item.",
                CompatibilityReason::SpecialCategory => "This kind of affix stays in its own slots.",
                CompatibilityReason::AlreadyPresent => "The item already has this affix.",
                _ => "Compatibility could not be confirmed for this affix.",
            },
            Self::TierUnavailable => "This affix does not support that tier.",
            Self::NewLayoutProblem(problem) => match problem {
                LayoutProblem::Capacity => "This edit exceeds the item's affix capacity.",
                LayoutProblem::UniqueCorrupted => "Unique and set items do not keep a corrupted affix.",
                LayoutProblem::LegendaryUncorrupted => "A legendary keeps a corrupted affix only while it is corrupted.",
                LayoutProblem::UniqueSealed => "Unique, set and legendary items have no sealed affix slot.",
                LayoutProblem::MultipleSealed => "An item holds one sealed affix.",
                LayoutProblem::MultipleCorrupted => "An item holds one corrupted affix.",
                LayoutProblem::PrimordialTier => "Only tier 8 affixes are primordial.",
                LayoutProblem::NormalTierEight => "A tier 8 affix is primordial unless it is sealed or corrupted. Choose Primordial.",
            },
            Self::MissingRollEvidence | Self::InvalidRollMapping => "Actual roll mappings are unavailable for this modifier.",
            Self::InvalidStatus => "That status value does not apply to this item.",
            Self::InvalidBase(problem) => match problem {
                BaseProblem::UniqueBacked => "Bases can change only on items that are not unique, set or legendary.",
                BaseProblem::Unchanged => "That is the item's base already.",
                BaseProblem::OnlyIdols => "Only an idol changes to another idol size.",
                BaseProblem::Unknown => "This base does not fit the current item.",
            },
            Self::InvalidUniqueIdentity(problem) => match problem {
                IdentityProblem::NotUniqueBacked => "Only a unique, set or legendary item can become another one, with the rarity the game gives it.",
                IdentityProblem::Unchanged => "That is the item's identity already.",
                IdentityProblem::SetHoldsAffixes => "A set item holds no affixes; remove them first.",
                IdentityProblem::NeedsOverride { set: true } => "A set item holds no legendary potential or Weaver's Will; keeping them needs the override.",
                IdentityProblem::NeedsOverride { set: false } => {
                    "That unique would keep legendary potential or Weaver's Will it never holds, or be a legendary the game never drops; that needs the override."
                }
                IdentityProblem::Mismatch => "That unique does not fit this item.",
            },
            Self::AffixIdNotSerializable(_) => "The game saves affix IDs up to 4095.",
            Self::ProjectedSnapshotOutOfBounds => "The edited item would exceed the supported item limits.",
        })
    }
}
impl std::error::Error for EditError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedObservation {
    generation: u64,
    process: ProcessIdentity,
    catalog: CatalogIdentity,
    snapshot: ForgeSnapshot,
}
impl ExpectedObservation {
    pub fn new(generation: u64, process: ProcessIdentity, catalog: CatalogIdentity, snapshot: ForgeSnapshot) -> Result<Self, EditError> {
        if generation == 0 || process.pid == 0 || process.created == 0 || !process.executable.is_absolute() || eg_trainer_wire::forge::encode(Some(&snapshot)).is_err() {
            return Err(EditError::InvalidObservation);
        }
        Ok(Self {
            generation,
            process,
            catalog,
            snapshot,
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn process(&self) -> &ProcessIdentity {
        &self.process
    }
    pub fn catalog_identity(&self) -> &CatalogIdentity {
        &self.catalog
    }
    pub fn snapshot(&self) -> &ForgeSnapshot {
        &self.snapshot
    }
    pub fn matches(&self, current: &Self) -> bool {
        self == current
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollBinding {
    pub editable: bool,
    pub roll_ids: Vec<i32>,
}
/// Actual tooltip/implicit mappings, bound to the entire observation. Catalog
/// endpoints alone do not supply this evidence. Construction performs no query;
/// the connected session binds the returned descriptions to this observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollEvidence {
    observation: ExpectedObservation,
    unique: Availability<Vec<RollBinding>>,
    implicits: Availability<Vec<RollBinding>>,
}
impl RollEvidence {
    pub fn from_runtime(observation: &ExpectedObservation, unique: Availability<Vec<RollBinding>>, implicits: Availability<Vec<RollBinding>>) -> Self {
        Self {
            observation: observation.clone(),
            unique,
            implicits,
        }
    }
    /// The same bindings for another observation of the same item and identity. Which rolls a line reads is the
    /// unique's and the base's; a roll edit changes neither, so the bindings carry over to its result.
    pub fn rebound(&self, observation: &ExpectedObservation) -> Self {
        Self {
            observation: observation.clone(),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedEdit {
    before: ExpectedObservation,
    edit: ItemEdit,
    projected: ForgeSnapshot,
    moves: BTreeMap<u8, u8>,
    /// The roll bindings a unique or implicit roll edit was checked against, kept for its Undo.
    evidence: Option<RollEvidence>,
}
impl PreparedEdit {
    pub fn evidence(&self) -> Option<&RollEvidence> {
        self.evidence.as_ref()
    }
    pub fn before(&self) -> &ExpectedObservation {
        &self.before
    }
    pub fn edit(&self) -> &ItemEdit {
        &self.edit
    }
    pub fn projected(&self) -> &ForgeSnapshot {
        &self.projected
    }
    pub fn moves(&self) -> &BTreeMap<u8, u8> {
        &self.moves
    }
    /// Pure final comparison only. It does not replace the runtime/log gate or
    /// the helper's main-thread expected-state check.
    pub fn recheck(&self, current: &ExpectedObservation, offline: bool, override_enabled: bool) -> Result<(), EditError> {
        if !offline {
            return Err(EditError::OfflineRequired);
        }
        if self.before.generation != current.generation || self.before.process != current.process || self.before.catalog != current.catalog {
            return Err(EditError::IdentityChanged);
        }
        if self.before.snapshot != current.snapshot {
            return Err(EditError::StaleItem);
        }
        if self.edit.needs_override() && !override_enabled {
            return Err(EditError::OverrideRequired);
        }
        Ok(())
    }
    pub fn inspect_readback(&self, after: Option<&ForgeSnapshot>) -> ReadbackDiagnostics {
        inspect_readback(&self.before.snapshot, &self.edit, &self.projected, after)
    }
}

fn backed(rarity: u8) -> bool {
    matches!(rarity, 7..=9)
}
fn idol(item_type: u8) -> bool {
    (25..=33).contains(&item_type)
}
fn byte(value: i32) -> bool {
    (0..=255).contains(&value)
}
fn index(item: &ForgeSnapshot, slot: i32) -> Result<usize, EditError> {
    usize::try_from(slot)
        .ok()
        .filter(|s| item.affixes.get(*s).is_some_and(|a| i32::from(a.slot) == slot))
        .ok_or(EditError::InvalidValues)
}
fn affix_values(id: i32, tier: i32, roll: i32) -> Result<(), EditError> {
    if !(0..=MAX_WRITABLE_AFFIX_ID).contains(&id) || !(1..=8).contains(&tier) || !byte(roll) {
        return Err(EditError::InvalidValues);
    }
    Ok(())
}

pub use eg_trainer_wire::rules::LayoutProblem;
use eg_trainer_wire::rules::{affix_order_key, layout_problems};
/// Settles an affix list on the projected item: the rarity and status it leaves (`affix_list_outcome`), then any layout
/// problem the item did not already have, judged on the item as it will be (a unique that gains an affix is legendary).
fn settle_affixes(before: &ForgeSnapshot, after: &mut ForgeSnapshot, affixes: &[AffixSnapshot]) -> Result<(), EditError> {
    (after.rarity, after.status) = eg_trainer_wire::rules::affix_list_outcome(before, affixes);
    let old = layout_problems(before, &before.affixes);
    match layout_problems(after, affixes).into_iter().find(|p| !old.contains(p)) {
        Some(p) => Err(EditError::NewLayoutProblem(p)),
        None => Ok(()),
    }
}
fn order(affixes: &mut [AffixSnapshot], rarity: u8) {
    affixes.sort_by_key(|a| affix_order_key(rarity, a.seal));
}

pub fn free_slots(item: &ForgeSnapshot) -> (i32, i32) {
    // A unique takes a legendary's slots: an affix makes it one. There are no legendary idols.
    let each = if item.rarity == 8 || (item.rarity == 7 && idol(item.item_type)) {
        0
    } else if idol(item.item_type) {
        1
    } else {
        2
    };
    (
        each - item.affixes.iter().filter(|a| a.seal == 0 && a.kind == 0).count() as i32,
        each - item.affixes.iter().filter(|a| a.seal == 0 && a.kind == 1).count() as i32,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityReason {
    None,
    MissingMetadata,
    UnknownGameValue,
    SlotType,
    EquipmentType,
    ClassRequirement,
    SpecialCategory,
    AlreadyPresent,
    Override,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compatibility {
    pub allowed: bool,
    pub reason: CompatibilityReason,
}
fn weaver_idol(item: &ForgeSnapshot) -> bool {
    matches!((item.item_type, item.sub_type), (25, 2) | (26, 1) | (27, 1) | (28, 1))
}
fn accepts_category(item: &ForgeSnapshot, slot: i32, affix: i32) -> bool {
    slot == affix || (weaver_idol(item) && matches!(slot, 0 | 5) && matches!(affix, 0 | 5))
}
fn equipment(allowed: &[i32], item: &ForgeSnapshot) -> bool {
    allowed.contains(&i32::from(item.item_type))
        || if (10..=14).contains(&item.sub_type) || item.sub_type == 17 {
            match item.item_type {
                29 => allowed.iter().any(|t| matches!(t, 31 | 33)),
                30 => allowed.contains(&32),
                _ => false,
            }
        } else {
            false
        }
}
fn matches_class(requirement: i32, specificity: i32) -> bool {
    requirement == 31
        || specificity == 0
        || if requirement == 0 {
            specificity & 1 != 0
        } else {
            (requirement << 1) & specificity != 0
        }
}
fn imposed_class(definition: Option<&AffixDefinition>) -> i32 {
    definition.filter(|d| d.class_requirement & 1 == 0).map_or(0, |d| (d.class_requirement >> 1) & 31)
}
pub fn compatibility(
    catalog: &GameCatalog,
    item: &ForgeSnapshot,
    definition: &AffixDefinition,
    slot: Option<&AffixSnapshot>,
    base_class: Option<i32>,
    unrestricted: bool,
) -> Compatibility {
    let Some(types) = &definition.item_types else {
        return Compatibility {
            allowed: false,
            reason: CompatibilityReason::MissingMetadata,
        };
    };
    let metadata = (0..=41).contains(&item.item_type)
        && (0..=1).contains(&definition.kind)
        && (0..=7).contains(&definition.special)
        && types.iter().all(|t| (0..=41).contains(t))
        && item.class_requirement & !31 == 0
        && definition.class_requirement & !63 == 0
        && slot.is_none_or(|s| (0..=1).contains(&s.kind) && (0..=7).contains(&s.special));
    if !metadata {
        return Compatibility {
            allowed: false,
            reason: CompatibilityReason::UnknownGameValue,
        };
    }
    let class_allowed = if let Some(slot) = slot {
        let others = item
            .affixes
            .iter()
            .filter(|a| a.slot != slot.slot)
            .fold(0, |bits, a| bits | imposed_class(catalog.affixes.get(&a.id)));
        base_class.map_or_else(
            || matches_class(item.class_requirement, definition.class_requirement) || matches_class(others, definition.class_requirement),
            |base| matches_class(base | others, definition.class_requirement),
        )
    } else {
        matches_class(item.class_requirement, definition.class_requirement)
    };
    let reason = if slot.is_some_and(|s| s.kind != definition.kind) {
        CompatibilityReason::SlotType
    } else if !equipment(types, item) {
        CompatibilityReason::EquipmentType
    } else if !class_allowed {
        CompatibilityReason::ClassRequirement
    } else if !accepts_category(item, slot.map_or(0, |s| s.special), definition.special) {
        CompatibilityReason::SpecialCategory
    } else if item.affixes.iter().any(|a| a.id == definition.id && slot.is_none_or(|s| s.slot != a.slot)) {
        CompatibilityReason::AlreadyPresent
    } else {
        CompatibilityReason::None
    };
    let override_allows = slot.is_none_or(|s| s.kind == definition.kind)
        && !item.affixes.iter().any(|a| a.id == definition.id && slot.is_none_or(|s| s.slot != a.slot))
        && if idol(item.item_type) {
            types.iter().any(|t| (25..=33).contains(t))
        } else {
            equipment(types, item)
        };
    if reason == CompatibilityReason::None {
        Compatibility { allowed: true, reason }
    } else if unrestricted && override_allows {
        Compatibility {
            allowed: true,
            reason: CompatibilityReason::Override,
        }
    } else {
        Compatibility { allowed: false, reason }
    }
}
fn definition(catalog: &GameCatalog, id: i32, tier: i32) -> Result<&AffixDefinition, EditError> {
    let definition = u16::try_from(id).ok().and_then(|id| catalog.affixes.get(&id)).ok_or(EditError::MissingDefinition)?;
    if tier > i32::from(definition.tier_count) {
        return Err(EditError::TierUnavailable);
    }
    Ok(definition)
}
fn validate_affix(catalog: &GameCatalog, item: &ForgeSnapshot, id: i32, tier: i32, slot: Option<&AffixSnapshot>, allow: bool) -> Result<(), EditError> {
    let d = definition(catalog, id, tier)?;
    let result = compatibility(catalog, item, d, slot, None, false);
    // The per-edit override permits every incompatibility, even unknown metadata;
    // an absent current definition never gets that exception.
    if !result.allowed && !allow {
        return Err(EditError::Incompatible(result.reason));
    }
    Ok(())
}

pub fn prepare(
    expected: &ExpectedObservation,
    catalog: &GameCatalog,
    edit: ItemEdit,
    override_enabled: bool,
    roll_evidence: Option<&RollEvidence>,
) -> Result<PreparedEdit, EditError> {
    if catalog.identity != expected.catalog {
        return Err(EditError::IdentityChanged);
    }
    if edit.needs_override() && !override_enabled {
        return Err(EditError::OverrideRequired);
    }
    let before = &expected.snapshot;
    let mut after = before.clone();
    let mut ordered = before.affixes.clone();
    match &edit {
        ItemEdit::Affix(e) => {
            affix_values(e.id, e.tier, e.roll)?;
            let slot = index(before, e.slot)?;
            // The affix the item holds keeps its place whatever its compatibility: only its definition and tier are
            // checked. A replacement must fit the slot.
            if i32::from(before.affixes[slot].id) == e.id {
                definition(catalog, e.id, e.tier)?;
            } else {
                validate_affix(catalog, before, e.id, e.tier, Some(&before.affixes[slot]), e.allow_incompatible)?;
            }
            let a = &mut ordered[slot];
            a.id = e.id as u16;
            a.tier = e.tier as u16;
            a.roll = e.roll as u8;
            a.seal = e.seal as i32;
            settle_affixes(before, &mut after, &ordered)?;
            order(&mut ordered, before.rarity);
        }
        ItemEdit::AddAffix(e) => {
            affix_values(e.id, e.tier, e.roll)?;
            validate_affix(catalog, before, e.id, e.tier, None, e.allow_incompatible)?;
            ordered.push(AffixSnapshot {
                slot: before.affixes.len() as u8,
                id: e.id as u16,
                tier: e.tier as u16,
                roll: e.roll as u8,
                kind: e.kind as i32,
                seal: e.seal as i32,
                name: String::new(),
                special: e.category.unwrap_or(0),
            });
            settle_affixes(before, &mut after, &ordered)?;
            order(&mut ordered, before.rarity);
        }
        ItemEdit::RemoveAffix(e) => {
            let slot = index(before, e.slot)?;
            if i32::from(before.affixes[slot].id) != e.id {
                return Err(EditError::InvalidValues);
            }
            ordered.remove(slot);
            settle_affixes(before, &mut after, &ordered)?;
            if let Some(potential) = e.restore_potential {
                if before.rarity != 9 || after.rarity != 7 || !catalog.limits.legendary_potential.contains(potential) {
                    return Err(EditError::InvalidValues);
                }
                after.status.legendary_potential = potential;
            }
            order(&mut ordered, before.rarity);
        }
        ItemEdit::Roll(e) => {
            if e.changes.is_empty()
                || e.changes.len() > MAX_ROLL_CHANGES
                || e.changes.iter().any(|c| !byte(c.value))
                || e.changes.iter().map(|c| c.index).collect::<BTreeSet<_>>().len() != e.changes.len()
            {
                return Err(EditError::InvalidValues);
            }
            for change in &e.changes {
                match e.target {
                    RollTarget::Affix => {
                        let slot = index(before, change.index)?;
                        let a = &before.affixes[slot];
                        definition(catalog, i32::from(a.id), i32::from(a.tier))?;
                        ordered[slot].roll = change.value as u8;
                    }
                    RollTarget::Unique | RollTarget::Implicit => {
                        let evidence = roll_evidence.filter(|r| r.observation == *expected).ok_or(EditError::MissingRollEvidence)?;
                        let (bindings, length) = if e.target == RollTarget::Unique {
                            if !backed(before.rarity) {
                                return Err(EditError::InvalidRollMapping);
                            }
                            (&evidence.unique, before.identity.as_ref().map_or(0, |u| u.rolls.len()))
                        } else {
                            (&evidence.implicits, before.implicit_rolls.len())
                        };
                        let Availability::Available(bindings) = bindings else {
                            return Err(EditError::MissingRollEvidence);
                        };
                        let valid = change.index >= 0
                            && (change.index as usize) < length
                            && bindings.iter().any(|b| {
                                b.editable
                                    && b.roll_ids.contains(&change.index)
                                    && if e.target == RollTarget::Implicit {
                                        b.roll_ids == [change.index]
                                    } else {
                                        b.roll_ids.iter().all(|id| *id >= 0 && (*id as usize) < length)
                                    }
                            });
                        if !valid {
                            return Err(EditError::InvalidRollMapping);
                        }
                        if e.target == RollTarget::Unique {
                            after.identity.as_mut().ok_or(EditError::InvalidRollMapping)?.rolls[change.index as usize] = change.value as u8
                        } else {
                            after.implicit_rolls[change.index as usize] = change.value as u8
                        }
                    }
                }
            }
        }
        ItemEdit::Status(e) => {
            validate_status(before, catalog, e)?;
            match e.field {
                StatusField::ForgingPotential => after.status.forging_potential = e.value as u8,
                StatusField::LegendaryPotential => after.status.legendary_potential = e.value as u8,
                StatusField::WeaversWill => after.status.weavers_will = e.value as u8,
                StatusField::Corrupted => after.status.corrupted = e.value != 0,
                StatusField::Ruined => after.status.ruined = e.value != 0,
                StatusField::LevelRequirement => after.status.level_requirement = e.value,
                StatusField::WeaversTouch => after.status.weavers_touch = e.value as u8,
            }
        }
        ItemEdit::BaseType(e) => {
            if backed(before.rarity) {
                return Err(EditError::InvalidBase(BaseProblem::UniqueBacked));
            }
            if !(0..=65535).contains(&e.sub_type) {
                return Err(EditError::InvalidBase(BaseProblem::Unknown));
            }
            let item_type = if let Some(t) = e.item_type {
                let t = u8::try_from(t).map_err(|_| EditError::InvalidBase(BaseProblem::Unknown))?;
                if t == before.item_type && e.sub_type == i32::from(before.sub_type) {
                    return Err(EditError::InvalidBase(BaseProblem::Unchanged));
                }
                if t == before.item_type || !idol(t) || !idol(before.item_type) {
                    return Err(EditError::InvalidBase(BaseProblem::OnlyIdols));
                }
                t
            } else {
                if e.sub_type == i32::from(before.sub_type) {
                    return Err(EditError::InvalidBase(BaseProblem::Unchanged));
                }
                before.item_type
            };
            if !catalog
                .bases
                .get(&item_type)
                .is_some_and(|b| b.sub_types.iter().any(|s| i32::from(s.sub_type) == e.sub_type))
            {
                return Err(EditError::InvalidBase(BaseProblem::Unknown));
            }
            after.item_type = item_type;
            after.sub_type = e.sub_type as u16;
            if !e.allow_incompatible && e.item_type.is_none() {
                for a in &before.affixes {
                    if let Some(d) = catalog.affixes.get(&a.id)
                        && compatibility(catalog, before, d, Some(a), None, false).allowed
                        && !compatibility(catalog, &after, d, Some(a), None, false).allowed
                    {
                        return Err(EditError::Incompatible(CompatibilityReason::EquipmentType));
                    }
                }
            }
        }
        ItemEdit::UniqueIdentity(e) => {
            let Some(current) = before.identity.as_ref().filter(|_| backed(before.rarity)) else {
                return Err(EditError::InvalidUniqueIdentity(IdentityProblem::NotUniqueBacked));
            };
            if !byte(e.sub_type) || !(0..=65535).contains(&e.unique_id) {
                return Err(EditError::InvalidUniqueIdentity(IdentityProblem::Mismatch));
            }
            if i32::from(current.id) == e.unique_id {
                return Err(EditError::InvalidUniqueIdentity(IdentityProblem::Unchanged));
            }
            let d = catalog.uniques.get(&(e.unique_id as u16)).ok_or(EditError::MissingDefinition)?;
            let rarity = rarity_for(before, d.is_set_item);
            if d.item_type != before.item_type || !d.sub_types.contains(&(e.sub_type as u8)) || i32::from(rarity) != e.rarity {
                return Err(EditError::InvalidUniqueIdentity(IdentityProblem::Mismatch));
            }
            if d.is_set_item && !before.affixes.is_empty() {
                return Err(EditError::InvalidUniqueIdentity(IdentityProblem::SetHoldsAffixes));
            }
            if !e.unrestricted && ((rarity == 9 && !d.can_drop_as_legendary) || !legendary_fits(before, d.is_set_item, d.legendary_type)) {
                return Err(EditError::InvalidUniqueIdentity(IdentityProblem::NeedsOverride { set: d.is_set_item }));
            }
            after.sub_type = e.sub_type as u16;
            after.rarity = rarity;
            after.identity.as_mut().ok_or(EditError::InvalidUniqueIdentity(IdentityProblem::NotUniqueBacked))?.id = e.unique_id as u16;
        }
    }
    let moves = ordered
        .iter()
        .enumerate()
        .filter(|(_, a)| usize::from(a.slot) < before.affixes.len())
        .map(|(slot, a)| (a.slot, slot as u8))
        .collect();
    for (slot, a) in ordered.iter_mut().enumerate() {
        a.slot = slot as u8
    }
    after.affixes = ordered;
    after.fingerprint = after.legacy_fingerprint();
    // Roll-only edits also rebuild serialization, which stores 12-bit IDs, and an
    // existing unique-capacity problem must not let the result exceed the reader's
    // hard 32 bound.
    if !matches!(
        edit,
        ItemEdit::Status(StatusEdit {
            field: StatusField::LevelRequirement,
            ..
        })
    ) && let Some(a) = after.affixes.iter().find(|a| i32::from(a.id) > MAX_WRITABLE_AFFIX_ID)
    {
        return Err(EditError::AffixIdNotSerializable(a.id));
    }
    if eg_trainer_wire::forge::encode(Some(&after)).is_err() {
        return Err(EditError::ProjectedSnapshotOutOfBounds);
    }
    let evidence = match &edit {
        ItemEdit::Roll(RollEdit {
            target: RollTarget::Unique | RollTarget::Implicit,
            ..
        }) => roll_evidence.cloned(),
        _ => None,
    };
    Ok(PreparedEdit {
        before: expected.clone(),
        edit,
        projected: after,
        moves,
        evidence,
    })
}

pub fn rarity_for(item: &ForgeSnapshot, set: bool) -> u8 {
    if set {
        8
    } else if !item.affixes.is_empty() || item.rarity == 9 {
        9
    } else {
        7
    }
}
pub fn legendary_fits(item: &ForgeSnapshot, set: bool, legendary_type: i32) -> bool {
    if set {
        item.status.legendary_potential == 0 && item.status.weavers_will == 0
    } else if legendary_type == 1 {
        item.status.legendary_potential == 0
    } else {
        item.status.weavers_will == 0
    }
}
fn validate_status(item: &ForgeSnapshot, catalog: &GameCatalog, e: &StatusEdit) -> Result<(), EditError> {
    let legendary_type = if e.field.requires_definition(item) {
        let id = item.identity.as_ref().ok_or(EditError::InvalidStatus)?.id;
        Some(catalog.uniques.get(&id).ok_or(EditError::MissingDefinition)?.legendary_type)
    } else {
        None
    };
    if e.field.allows(item, &catalog.limits, e.value, e.unrestricted, legendary_type) {
        Ok(())
    } else {
        Err(EditError::InvalidStatus)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadbackPolicy {
    /// Direct cache store: no rebuild, rename or other derived change is expected.
    LevelRequirement,
    Strict,
    Override,
    Roll,
    Status,
    Base,
    Identity,
    IdentityOverride,
    Add,
    AddOverride,
    Remove,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadbackDifference {
    pub field: String,
    pub expected: String,
    pub actual: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadbackDiagnostics {
    pub policy: ReadbackPolicy,
    pub observed: Option<ForgeSnapshot>,
    pub differences: Vec<ReadbackDifference>,
    pub matching_slots: Vec<u8>,
}
impl ReadbackDiagnostics {
    pub fn confirmed(&self) -> bool {
        self.differences.is_empty()
    }
}
fn policy(edit: &ItemEdit) -> ReadbackPolicy {
    match edit {
        ItemEdit::Affix(e) => {
            if e.allow_incompatible {
                ReadbackPolicy::Override
            } else {
                ReadbackPolicy::Strict
            }
        }
        ItemEdit::Roll(_) => ReadbackPolicy::Roll,
        ItemEdit::Status(StatusEdit {
            field: StatusField::LevelRequirement,
            ..
        }) => ReadbackPolicy::LevelRequirement,
        ItemEdit::Status(_) => ReadbackPolicy::Status,
        ItemEdit::BaseType(_) => ReadbackPolicy::Base,
        ItemEdit::UniqueIdentity(e) => {
            if e.unrestricted {
                ReadbackPolicy::IdentityOverride
            } else {
                ReadbackPolicy::Identity
            }
        }
        ItemEdit::AddAffix(e) => {
            if e.allow_incompatible {
                ReadbackPolicy::AddOverride
            } else {
                ReadbackPolicy::Add
            }
        }
        ItemEdit::RemoveAffix(_) => ReadbackPolicy::Remove,
    }
}
fn compare<T: fmt::Debug + PartialEq>(differences: &mut Vec<ReadbackDifference>, field: impl Into<String>, expected: &T, actual: &T) {
    if expected != actual {
        differences.push(ReadbackDifference {
            field: field.into(),
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        })
    }
}
fn inspect_readback(before: &ForgeSnapshot, edit: &ItemEdit, expected: &ForgeSnapshot, after: Option<&ForgeSnapshot>) -> ReadbackDiagnostics {
    let mut diagnostics = ReadbackDiagnostics {
        policy: policy(edit),
        observed: after.cloned(),
        differences: Vec::new(),
        matching_slots: Vec::new(),
    };
    let Some(after) = after else {
        diagnostics.differences.push(ReadbackDifference {
            field: "item".into(),
            expected: "Present".into(),
            actual: "Missing".into(),
        });
        return diagnostics;
    };
    let differences = &mut diagnostics.differences;
    if diagnostics.policy == ReadbackPolicy::LevelRequirement {
        compare(differences, "item", expected, after);
        return diagnostics;
    }
    compare(differences, "token.item", &before.item, &after.item);
    compare(differences, "token.individual_id", &before.individual_id, &after.individual_id);
    if let ItemEdit::Affix(e) = edit {
        diagnostics.matching_slots = after
            .affixes
            .iter()
            .filter(|a| (i32::from(a.id), i32::from(a.tier), i32::from(a.roll), a.seal) == (e.id, e.tier, e.roll, e.seal as i32))
            .map(|a| a.slot)
            .collect();
        if e.allow_incompatible {
            // Locate by the submitted original slot through stable layout; duplicate
            // matching values elsewhere are only diagnostic.
            let mut labels = before.affixes.clone();
            labels[e.slot as usize].seal = e.seal as i32;
            order(&mut labels, before.rarity);
            let slot = labels.iter().position(|a| i32::from(a.slot) == e.slot).unwrap_or(e.slot as usize) as u8;
            let matches = after.affixes.iter().filter(|a| a.slot == slot).collect::<Vec<_>>();
            compare(differences, format!("affixes[slot={slot}].count"), &1_usize, &matches.len());
            if let [a] = matches.as_slice() {
                let field = format!("affixes[slot={slot}]");
                compare(differences, format!("{field}.id"), &(e.id as u16), &a.id);
                compare(differences, format!("{field}.tier"), &(e.tier as u16), &a.tier);
                compare(differences, format!("{field}.roll"), &(e.roll as u8), &a.roll);
                compare(differences, format!("{field}.seal"), &(e.seal as i32), &a.seal);
            }
            return diagnostics;
        }
    }
    compare(differences, "itemType", &expected.item_type, &after.item_type);
    compare(differences, "subType", &expected.sub_type, &after.sub_type);
    if matches!(edit, ItemEdit::Affix(_) | ItemEdit::Roll(_) | ItemEdit::Status(_)) {
        compare(differences, "classRequirement", &expected.class_requirement, &after.class_requirement)
    }
    compare(differences, "rarity", &expected.rarity, &after.rarity);
    // Readback compares set identity too.
    compare(differences, "identity", &expected.identity, &after.identity);
    if !matches!(edit, ItemEdit::BaseType(BaseTypeEdit { item_type: Some(_), .. })) {
        compare(differences, "implicitRolls", &expected.implicit_rolls, &after.implicit_rolls)
    }
    let mut expected_status = expected.status.clone();
    let mut actual_status = after.status.clone();
    if !matches!(
        edit,
        ItemEdit::Status(StatusEdit {
            field: StatusField::LevelRequirement,
            ..
        })
    ) {
        expected_status.level_requirement = 0;
        actual_status.level_requirement = 0
    }
    compare(differences, "status", &expected_status, &actual_status);
    compare(differences, "affixes.length", &expected.affixes.len(), &after.affixes.len());
    for (index, (wanted, actual)) in expected.affixes.iter().zip(&after.affixes).enumerate() {
        let mut wanted = wanted.clone();
        let mut actual = actual.clone();
        wanted.name.clear();
        actual.name.clear();
        if let ItemEdit::AddAffix(e) = edit
            && e.category.is_none()
        {
            let mut labels = before.affixes.clone();
            labels.push(AffixSnapshot {
                slot: labels.len() as u8,
                id: e.id as u16,
                tier: e.tier as u16,
                roll: e.roll as u8,
                kind: e.kind as i32,
                seal: e.seal as i32,
                name: String::new(),
                special: 0,
            });
            order(&mut labels, before.rarity);
            if labels.get(index).is_some_and(|a| usize::from(a.slot) == before.affixes.len()) {
                wanted.special = actual.special
            }
        }
        if let ItemEdit::Affix(e) = edit {
            let mut labels = before.affixes.clone();
            labels[e.slot as usize].seal = e.seal as i32;
            order(&mut labels, before.rarity);
            if labels.get(index).is_some_and(|a| i32::from(a.slot) == e.slot) && accepts_category(before, wanted.special, actual.special) {
                wanted.special = actual.special
            }
        }
        compare(differences, format!("affixes[index={index}]"), &wanted, &actual);
    }
    diagnostics
}
