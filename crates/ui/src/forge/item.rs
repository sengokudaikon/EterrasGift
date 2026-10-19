//! The item in the forge as the presentation sees it, and the game's own rules the editor applies before it writes
//! (spec/item-forge.md §15). TODO(forge): these move to the trainer's domain once it is ported; the editor keeps only
//! what it shows.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffixKind {
    Prefix,
    Suffix,
}

impl AffixKind {
    pub fn name(self) -> &'static str {
        match self {
            AffixKind::Prefix => "Prefix",
            AffixKind::Suffix => "Suffix",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Seal {
    Normal,
    Sealed,
    Primordial,
    Corrupted,
}

impl Seal {
    /// The seal menu's entries, in the game's order.
    pub const ALL: [Seal; 4] = [Seal::Normal, Seal::Sealed, Seal::Primordial, Seal::Corrupted];

    pub fn name(self) -> &'static str {
        match self {
            Seal::Normal => "Normal",
            Seal::Sealed => "Sealed",
            Seal::Primordial => "Primordial",
            Seal::Corrupted => "Corrupted",
        }
    }

    pub fn index(self) -> usize {
        Seal::ALL.iter().position(|seal| *seal == self).unwrap_or(0)
    }
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
    pub formatting_error: String,
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

    pub fn with(mut self, field: StatusField, value: u8) -> ItemStatus {
        match field {
            StatusField::ForgingPotential => self.forging_potential = value,
            StatusField::LegendaryPotential => self.legendary_potential = value,
            StatusField::WeaversWill => self.weavers_will = value,
            StatusField::WeaversTouch => self.weavers_touch = value,
            StatusField::LevelRequirement => self.level_requirement = value,
            StatusField::Corrupted => self.corrupted = value != 0,
            StatusField::Ruined => self.ruined = value != 0,
        }
        self
    }
}

/// What the forge holds, as one read of the game.
#[derive(Clone, Debug, PartialEq)]
pub struct ForgeItem {
    /// Another token is another item; the same token changed by the game is the same item.
    pub token: String,
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
        let each = match self.rarity {
            UNIQUE | SET => 0,
            _ if self.idol => 1,
            _ => 2,
        };
        let open = |kind| self.affixes.iter().filter(|affix| affix.seal == Seal::Normal && affix.kind == kind).count() as i32;
        (each - open(AffixKind::Prefix), each - open(AffixKind::Suffix))
    }
}

/// The affixes in the order the game loads them, keeping their slot numbers (spec §15.1.1).
pub fn load_order(affixes: &[AffixState], rarity: u8) -> Vec<AffixState> {
    let pick = |test: &dyn Fn(&AffixState) -> bool| affixes.iter().filter(|affix| test(affix)).cloned().collect::<Vec<_>>();
    if is_unique_backed(rarity) {
        [pick(&|a| a.seal != Seal::Corrupted), pick(&|a| a.seal == Seal::Corrupted)].concat()
    } else {
        [
            pick(&|a| a.seal == Seal::Sealed),
            pick(&|a| a.seal == Seal::Corrupted),
            pick(&|a| !matches!(a.seal, Seal::Sealed | Seal::Corrupted)),
        ]
        .concat()
    }
}

fn layout_problems(affixes: &[AffixState], rarity: u8, corrupted: bool) -> Vec<String> {
    let mut problems = Vec::new();
    let capacity = if is_unique_backed(rarity) { UNIQUE_CAPACITY } else { CAPACITY };
    let count = |seal| affixes.iter().filter(|affix| affix.seal == seal).count();
    if affixes.len() > capacity {
        problems.push(format!("This item holds at most {capacity} affixes."));
    }
    if count(Seal::Corrupted) > 0 && matches!(rarity, UNIQUE | SET) {
        problems.push("Unique and set items do not keep a corrupted affix.".into());
    }
    if count(Seal::Corrupted) > 0 && rarity == LEGENDARY && !corrupted {
        problems.push("A legendary keeps a corrupted affix only while it is corrupted.".into());
    }
    if count(Seal::Sealed) > 0 && is_unique_backed(rarity) {
        problems.push("Unique, set and legendary items have no sealed affix slot.".into());
    }
    if count(Seal::Sealed) > 1 {
        problems.push("An item holds one sealed affix.".into());
    }
    if count(Seal::Corrupted) > 1 {
        problems.push("An item holds one corrupted affix.".into());
    }
    if affixes.iter().any(|affix| affix.seal == Seal::Primordial && affix.tier != 8) {
        problems.push("Only tier 8 affixes are primordial.".into());
    }
    if affixes.iter().any(|affix| affix.seal == Seal::Normal && affix.tier == 8) {
        problems.push("A tier 8 affix is primordial unless it is sealed or corrupted. Choose Primordial.".into());
    }
    problems
}

/// A problem these affixes bring that the item did not already have (spec §15.1.6).
pub fn new_problem(before: &ForgeItem, after: &[AffixState]) -> Option<String> {
    let corrupted = before.status.is_none_or(|status| status.corrupted);
    let existing = layout_problems(&before.affixes, before.rarity, corrupted);
    layout_problems(after, before.rarity, corrupted).into_iter().find(|problem| !existing.contains(problem))
}

/// The rarity byte the game sets after adding or removing, below unique rarity (spec §15.1.7).
pub fn rarity_after(affixes: &[AffixState], rarity: u8) -> u8 {
    if is_unique_backed(rarity) || rarity > 6 {
        return rarity;
    }
    let mut seals: Vec<Seal> = affixes.iter().map(|affix| affix.seal).filter(|seal| *seal != Seal::Normal).collect();
    seals.sort();
    seals.dedup();
    (affixes.len() as i64 - seals.len() as i64).clamp(0, 4) as u8
}

/// Where each affix went: old slot to new slot.
pub type Moves = BTreeMap<usize, usize>;

/// Two moves one after the other.
pub fn then(first: &Moves, next: &Moves) -> Moves {
    first.iter().map(|(from, to)| (*from, *next.get(to).unwrap_or(to))).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollTarget {
    Affix,
    Unique,
    Implicit,
}

/// One write to the game (spec §15.4).
#[derive(Clone, Debug, PartialEq)]
pub enum ItemEdit {
    Affix {
        slot: usize,
        id: u16,
        tier: u8,
        roll: u8,
        seal: Seal,
        allow_incompatible: bool,
    },
    Rolls {
        target: RollTarget,
        changes: Vec<(usize, u8)>,
    },
    Status {
        field: StatusField,
        value: u8,
        unrestricted: bool,
    },
    Base {
        sub_type: u16,
        allow_incompatible: bool,
    },
    Add {
        id: u16,
        tier: u8,
        roll: u8,
        seal: Seal,
        kind: AffixKind,
        category: Category,
        allow_incompatible: bool,
    },
    Remove {
        slot: usize,
        id: u16,
    },
    /// Makes a unique-backed item another unique or set item, with the rarity the game gives it (spec §15.4.5).
    Identity {
        unique_id: u16,
        sub_type: u16,
        rarity: u8,
        unrestricted: bool,
    },
}

/// The rarity the game gives an item becoming a unique or set item: a set item is a set; with affixes, or already
/// legendary, a legendary; otherwise a unique.
pub fn identity_rarity(item: &ForgeItem, set_item: bool) -> u8 {
    if set_item {
        SET
    } else if !item.affixes.is_empty() || item.rarity == LEGENDARY {
        LEGENDARY
    } else {
        UNIQUE
    }
}

impl ItemEdit {
    /// Goes past what the game would make, so it needs "I know what I'm doing".
    pub fn needs_override(&self) -> bool {
        match self {
            ItemEdit::Affix { allow_incompatible, .. } | ItemEdit::Add { allow_incompatible, .. } => *allow_incompatible,
            ItemEdit::Status { unrestricted, .. } => *unrestricted,
            ItemEdit::Base { allow_incompatible, .. } => *allow_incompatible,
            ItemEdit::Identity { unrestricted, .. } => *unrestricted,
            _ => false,
        }
    }

    fn replaced(&self, before: &ForgeItem) -> Vec<AffixState> {
        match self {
            ItemEdit::Affix { slot, id, tier, roll, seal, .. } => load_order(
                &before
                    .affixes
                    .iter()
                    .map(|affix| {
                        if affix.slot == *slot {
                            AffixState {
                                id: *id,
                                tier: *tier,
                                roll: *roll,
                                seal: *seal,
                                ..affix.clone()
                            }
                        } else {
                            affix.clone()
                        }
                    })
                    .collect::<Vec<_>>(),
                before.rarity,
            ),
            ItemEdit::Add {
                id,
                tier,
                roll,
                seal,
                kind,
                category,
                ..
            } => {
                let mut affixes = before.affixes.clone();
                affixes.push(AffixState {
                    slot: before.affixes.len(),
                    id: *id,
                    tier: *tier,
                    roll: *roll,
                    kind: *kind,
                    seal: *seal,
                    category: *category,
                });
                load_order(&affixes, before.rarity)
            }
            ItemEdit::Remove { slot, .. } => load_order(&before.affixes.iter().filter(|affix| affix.slot != *slot).cloned().collect::<Vec<_>>(), before.rarity),
            _ => before.affixes.clone(),
        }
    }

    pub fn is_valid(&self, item: &ForgeItem) -> bool {
        match self {
            ItemEdit::Affix { slot, tier, .. } => {
                item.affixes.iter().filter(|affix| affix.slot == *slot).count() == 1 && (1..=8).contains(tier) && new_problem(item, &self.replaced(item)).is_none()
            }
            ItemEdit::Add { tier, .. } => (1..=8).contains(tier) && new_problem(item, &self.project(item).affixes).is_none(),
            ItemEdit::Remove { slot, id } => item.affixes.iter().any(|affix| affix.slot == *slot && affix.id == *id),
            ItemEdit::Rolls { target, changes } => {
                !changes.is_empty()
                    && match target {
                        RollTarget::Affix => changes.iter().all(|(slot, _)| item.affix(*slot).is_some()),
                        RollTarget::Unique => item.unique.as_ref().is_some_and(|unique| {
                            matches!(item.rarity, UNIQUE | LEGENDARY)
                                && changes
                                    .iter()
                                    .all(|(index, _)| *index < unique.rolls.len() && unique.modifiers.iter().any(|m| m.editable && m.roll_ids.contains(index)))
                        }),
                        RollTarget::Implicit => item.implicits.as_ref().is_some_and(|implicits| {
                            changes
                                .iter()
                                .all(|(index, _)| *index < item.implicit_rolls.len() && implicits.iter().any(|m| m.editable && m.roll_ids == [*index]))
                        }),
                    }
            }
            ItemEdit::Status { field, .. } => match field {
                StatusField::ForgingPotential | StatusField::LevelRequirement => item.rarity < UNIQUE,
                StatusField::LegendaryPotential => item.rarity == UNIQUE && item.status.is_some_and(|status| status.weavers_will == 0),
                StatusField::WeaversWill => matches!(item.rarity, UNIQUE | LEGENDARY),
                StatusField::WeaversTouch => item.idol,
                StatusField::Corrupted | StatusField::Ruined => true,
            },
            ItemEdit::Base { sub_type, .. } => !is_unique_backed(item.rarity) && *sub_type != item.sub_type,
            ItemEdit::Identity { unique_id, rarity, .. } => {
                let set = *rarity == SET;
                is_unique_backed(item.rarity) && item.identity_id().is_some_and(|id| id != *unique_id) && *rarity == identity_rarity(item, set) && (!set || item.affixes.is_empty())
            }
        }
    }

    /// The item as the game holds it after this edit, with the affixes in load order.
    pub fn project(&self, before: &ForgeItem) -> ForgeItem {
        let mut after = before.clone();
        match self {
            ItemEdit::Affix { .. } => {
                after.affixes = self
                    .replaced(before)
                    .into_iter()
                    .enumerate()
                    .map(|(index, affix)| AffixState {
                        slot: before.affixes[index].slot,
                        ..affix
                    })
                    .collect();
            }
            ItemEdit::Add { .. } | ItemEdit::Remove { .. } => {
                after.affixes = self.replaced(before).into_iter().enumerate().map(|(slot, affix)| AffixState { slot, ..affix }).collect();
                after.rarity = rarity_after(&after.affixes, before.rarity);
            }
            ItemEdit::Rolls { target, changes } => {
                for (index, value) in changes {
                    match target {
                        RollTarget::Affix => {
                            if let Some(affix) = after.affixes.iter_mut().find(|affix| affix.slot == *index) {
                                affix.roll = *value;
                            }
                        }
                        RollTarget::Unique => {
                            if let Some(roll) = after.unique.as_mut().and_then(|unique| unique.rolls.get_mut(*index)) {
                                *roll = *value;
                            }
                        }
                        RollTarget::Implicit => {
                            if let Some(roll) = after.implicit_rolls.get_mut(*index) {
                                *roll = *value;
                            }
                        }
                    }
                }
            }
            ItemEdit::Status { field, value, .. } => after.status = before.status.map(|status| status.with(*field, *value)),
            ItemEdit::Base { sub_type, .. } => after.sub_type = *sub_type,
            ItemEdit::Identity { unique_id, sub_type, rarity, .. } => {
                // The rolls move with the item, between the unique and the set side.
                let rolls = before
                    .unique
                    .as_ref()
                    .or(before.set_item.as_ref())
                    .map(|identity| identity.rolls.clone())
                    .unwrap_or_default();
                let identity = UniqueState {
                    id: *unique_id,
                    rolls,
                    modifiers: Vec::new(),
                    formatting_error: String::new(),
                };
                after.sub_type = *sub_type;
                after.rarity = *rarity;
                if *rarity == SET {
                    after.unique = None;
                    after.set_item = Some(identity);
                } else {
                    after.set_item = None;
                    after.unique = Some(UniqueState {
                        modifiers: before.unique.as_ref().map(|unique| unique.modifiers.clone()).unwrap_or_default(),
                        ..identity
                    });
                }
            }
        }
        after
    }

    /// Where the affixes go: sealing or corrupting moves an affix; adding and removing renumber.
    pub fn moves(&self, before: &ForgeItem) -> Moves {
        match self {
            ItemEdit::Affix { .. } => self
                .replaced(before)
                .iter()
                .enumerate()
                .map(|(index, affix)| (affix.slot, before.affixes[index].slot))
                .collect(),
            ItemEdit::Add { .. } | ItemEdit::Remove { .. } => self.replaced(before).iter().enumerate().map(|(index, affix)| (affix.slot, index)).collect(),
            _ => before.affixes.iter().map(|affix| (affix.slot, affix.slot)).collect(),
        }
    }

    /// The same edit naming the slots where earlier writes moved its affix.
    pub fn follow(&self, moves: &Moves) -> ItemEdit {
        let at = |slot: &usize| *moves.get(slot).unwrap_or(slot);
        match self {
            ItemEdit::Affix {
                slot,
                id,
                tier,
                roll,
                seal,
                allow_incompatible,
            } => ItemEdit::Affix {
                slot: at(slot),
                id: *id,
                tier: *tier,
                roll: *roll,
                seal: *seal,
                allow_incompatible: *allow_incompatible,
            },
            ItemEdit::Remove { slot, id } => ItemEdit::Remove { slot: at(slot), id: *id },
            ItemEdit::Rolls {
                target: RollTarget::Affix,
                changes,
            } => ItemEdit::Rolls {
                target: RollTarget::Affix,
                changes: changes.iter().map(|(slot, value)| (at(slot), *value)).collect(),
            },
            other => other.clone(),
        }
    }

    /// The edit that writes the previous values back, or None for one that cannot be reversed.
    pub fn reverse(&self, before: &ForgeItem) -> Option<ItemEdit> {
        match self {
            ItemEdit::Affix { slot, allow_incompatible, .. } => {
                let previous = before.affix(*slot)?;
                Some(ItemEdit::Affix {
                    slot: *self.moves(before).get(slot)?,
                    id: previous.id,
                    tier: previous.tier,
                    roll: previous.roll,
                    seal: previous.seal,
                    allow_incompatible: *allow_incompatible,
                })
            }
            ItemEdit::Rolls { target, changes } => Some(ItemEdit::Rolls {
                target: *target,
                changes: changes
                    .iter()
                    .filter_map(|(index, _)| {
                        let previous = match target {
                            RollTarget::Affix => before.affix(*index).map(|affix| affix.roll),
                            RollTarget::Unique => before.unique.as_ref().and_then(|unique| unique.rolls.get(*index).copied()),
                            RollTarget::Implicit => before.implicit_rolls.get(*index).copied(),
                        };
                        previous.map(|value| (*index, value))
                    })
                    .collect(),
            }),
            ItemEdit::Status { field, unrestricted, .. } => before.status.map(|status| ItemEdit::Status {
                field: *field,
                value: status.read(*field),
                unrestricted: *unrestricted,
            }),
            ItemEdit::Base { allow_incompatible, .. } => Some(ItemEdit::Base {
                sub_type: before.sub_type,
                allow_incompatible: *allow_incompatible,
            }),
            // A reverse the game would refuse (it gives another rarity back) makes the change one-way.
            ItemEdit::Identity { unrestricted, .. } => {
                let back = ItemEdit::Identity {
                    unique_id: before.identity_id()?,
                    sub_type: before.sub_type,
                    rarity: before.rarity,
                    unrestricted: *unrestricted,
                };
                back.is_valid(&self.project(before)).then_some(back)
            }
            ItemEdit::Add { .. } | ItemEdit::Remove { .. } => None,
        }
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

/// Whether an affix fits the item, or a slot of it (spec §15.2). TODO(forge): class requirements and the idol rules
/// (omen and weaver idols) come with the trainer's domain; the fixture has neither.
pub fn compatibility(item: &ForgeItem, definition: &AffixDefinition, slot: Option<&AffixState>, unrestricted: bool) -> Compatibility {
    if !definition.verified {
        return Compatibility::Unknown;
    }
    let strict = if slot.is_some_and(|slot| slot.kind != definition.kind) {
        Some(Rejection::SlotType)
    } else if !definition.item_types.contains(&item.item_type) {
        Some(Rejection::EquipmentType)
    } else if slot.map_or(Category::Standard, |slot| slot.category) != definition.category {
        Some(Rejection::SpecialCategory)
    } else if item.affixes.iter().any(|affix| affix.id == definition.id && Some(affix.slot) != slot.map(|slot| slot.slot)) {
        Some(Rejection::AlreadyPresent)
    } else {
        None
    };
    match strict {
        None => Compatibility::Allowed,
        Some(Rejection::SpecialCategory) if unrestricted => Compatibility::Override,
        Some(reason) => Compatibility::Rejected(reason),
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
    fn uncorrupting_moves_an_affix_where_the_game_loads_it() {
        // The fixture item is not in load order; an affix write puts the sealed and corrupted affixes first.
        let boots = item(vec![
            affix(0, 25, Seal::Normal),
            affix(1, 1148, Seal::Normal),
            affix(2, 4000, Seal::Sealed),
            affix(3, 4001, Seal::Corrupted),
        ]);
        let uncorrupt = ItemEdit::Affix {
            slot: 3,
            id: 4001,
            tier: 4,
            roll: 100,
            seal: Seal::Normal,
            allow_incompatible: false,
        };
        let after = uncorrupt.project(&boots);
        assert_eq!(after.affixes.iter().map(|a| a.id).collect::<Vec<_>>(), [4000, 25, 1148, 4001]);
        assert_eq!(uncorrupt.moves(&boots), Moves::from([(2, 0), (0, 1), (1, 2), (3, 3)]));
        // Its reverse writes the corrupted seal back where the affix went, and the game loads it second.
        let back = uncorrupt.reverse(&boots).unwrap();
        assert_eq!(
            back,
            ItemEdit::Affix {
                slot: 3,
                id: 4001,
                tier: 4,
                roll: 100,
                seal: Seal::Corrupted,
                allow_incompatible: false
            }
        );
        assert_eq!(back.project(&after).affixes.iter().map(|a| a.id).collect::<Vec<_>>(), [4000, 4001, 25, 1148]);
    }

    #[test]
    fn a_second_sealed_affix_is_a_new_problem() {
        let boots = item(vec![affix(0, 4000, Seal::Sealed), affix(1, 25, Seal::Normal)]);
        let seal = ItemEdit::Affix {
            slot: 1,
            id: 25,
            tier: 4,
            roll: 100,
            seal: Seal::Sealed,
            allow_incompatible: false,
        };
        assert!(!seal.is_valid(&boots));
        assert_eq!(new_problem(&boots, &seal.replaced(&boots)).as_deref(), Some("An item holds one sealed affix."));
    }

    #[test]
    fn free_slots_count_unsealed_affixes() {
        let boots = item(vec![affix(0, 25, Seal::Normal), affix(1, 4001, Seal::Corrupted)]);
        assert_eq!(boots.free_slots(), (2, 1));
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
