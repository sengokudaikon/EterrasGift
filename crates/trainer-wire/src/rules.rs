//! The game's rules for an item that both processes enforce: the desktop when it plans an edit, the helper before it
//! stores one. They decide nothing the game decides itself.

#![forbid(unsafe_code)]

use crate::{
    definitions::{ItemLimits, ValueRange},
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus},
    write::StatusField,
};

impl StatusField {
    pub fn requires_definition(self, item: &ForgeSnapshot) -> bool {
        matches!(self, Self::LegendaryPotential | Self::WeaversWill) && item.rarity == 7
    }
    /// Game status rules shared by planning and the native pre-store guard.
    /// `legendary_type` comes from the captured catalog on the host and getUnique
    /// inside the game. Neither an override nor an absent definition guesses it.
    pub fn allows(self, item: &ForgeSnapshot, limits: &ItemLimits, value: i32, unrestricted: bool, legendary_type: Option<i32>) -> bool {
        if !(0..=255).contains(&value) {
            return false;
        }
        let status = &item.status;
        let in_range = |range: ValueRange| (i32::from(range.minimum)..=i32::from(range.maximum)).contains(&value);
        match self {
            Self::ForgingPotential => !matches!(item.rarity, 7..=9) && in_range(limits.forging_potential),
            Self::LevelRequirement => !matches!(item.rarity, 7..=9) && in_range(limits.level_requirement),
            Self::Corrupted | Self::Ruined => matches!(value, 0 | 1),
            Self::WeaversTouch => unrestricted && (25..=33).contains(&item.item_type) && (value == 0 || in_range(limits.weavers_touch)),
            Self::LegendaryPotential | Self::WeaversWill => {
                let will = self == Self::WeaversWill;
                if will && unrestricted && item.rarity == 9 {
                    value <= i32::from(status.weavers_will.max(limits.weavers_will.maximum)) && (status.weavers_will > 0 || status.legendary_potential == 0 || value == 0)
                } else if item.rarity == 7 {
                    let forced = will && unrestricted && legendary_type == Some(0);
                    (legendary_type == Some(i32::from(will)) || forced)
                        && (if will { status.legendary_potential == 0 } else { status.weavers_will == 0 })
                        && (if will {
                            in_range(limits.weavers_will) || (forced && value == 0)
                        } else {
                            in_range(limits.legendary_potential)
                        })
                } else {
                    false
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutProblem {
    Capacity,
    UniqueCorrupted,
    LegendaryUncorrupted,
    UniqueSealed,
    MultipleSealed,
    MultipleCorrupted,
    PrimordialTier,
    NormalTierEight,
}
/// What makes an affix layout one the game cannot hold.
pub fn layout_problems(item: &ForgeSnapshot, affixes: &[AffixSnapshot]) -> Vec<LayoutProblem> {
    let mut problems = Vec::new();
    let sealed = affixes.iter().filter(|a| a.seal == 1).count();
    let corrupted = affixes.iter().filter(|a| a.seal == 3).count();
    if affixes.len() > if matches!(item.rarity, 7..=9) { 7 } else { 32 } {
        problems.push(LayoutProblem::Capacity)
    }
    if corrupted > 0 && matches!(item.rarity, 7 | 8) {
        problems.push(LayoutProblem::UniqueCorrupted)
    }
    if corrupted > 0 && item.rarity == 9 && !item.status.corrupted {
        problems.push(LayoutProblem::LegendaryUncorrupted)
    }
    if sealed > 0 && matches!(item.rarity, 7..=9) {
        problems.push(LayoutProblem::UniqueSealed)
    }
    if sealed > 1 {
        problems.push(LayoutProblem::MultipleSealed)
    }
    if corrupted > 1 {
        problems.push(LayoutProblem::MultipleCorrupted)
    }
    if affixes.iter().any(|a| a.seal == 2 && a.tier != 8) {
        problems.push(LayoutProblem::PrimordialTier)
    }
    if affixes.iter().any(|a| a.seal == 0 && a.tier == 8) {
        problems.push(LayoutProblem::NormalTierEight)
    }
    problems
}
/// Stable load grouping used by the game: regular seal, corrupted seal, remaining affixes.
pub fn affix_order_key(rarity: u8, seal: i32) -> u8 {
    if matches!(rarity, 7..=9) {
        u8::from(seal == 3)
    } else {
        match seal {
            1 => 0,
            3 => 1,
            _ => 2,
        }
    }
}

/// The affixes in the game's load order, numbered again.
pub(crate) fn reorder_affixes(item: &mut ForgeSnapshot) {
    item.affixes.sort_by_key(|a| affix_order_key(item.rarity, a.seal));
    for (slot, affix) in item.affixes.iter_mut().enumerate() {
        affix.slot = slot as u8;
    }
}
/// The rarity and status an affix list leaves the item: its rarity (`affix_list_rarity`), and what the game's own
/// crafting does beside it. A unique that becomes legendary spends its legendary potential (a legendary holds none;
/// Weaver's Will stays), and a legendary that holds a corrupted affix is corrupted, as corruption leaves it: the save
/// keeps that affix's corruption only on a corrupted legendary.
pub fn affix_list_outcome(item: &ForgeSnapshot, affixes: &[AffixSnapshot]) -> (u8, ItemStatus) {
    let rarity = affix_list_rarity(item, affixes);
    let mut status = item.status.clone();
    if item.rarity == 7 && rarity == 9 {
        status.legendary_potential = 0;
    }
    if rarity == 9 && affixes.iter().any(|affix| affix.seal == 3) {
        status.corrupted = true;
    }
    (rarity, status)
}
/// The rarity an affix list gives. Below unique it is the game's own rule (the native writer invokes the game's rarity
/// method). A unique that gains an affix is legendary, as the Temporal Sanctum and corruption make it, and a legendary
/// that loses its last is a unique again (the native writer stores it); a set item and a unique idol (there are no
/// legendary idols) keep their rarity.
pub fn affix_list_rarity(item: &ForgeSnapshot, affixes: &[AffixSnapshot]) -> u8 {
    match item.rarity {
        7 | 9 if !(25..=33).contains(&item.item_type) => return if affixes.is_empty() { 7 } else { 9 },
        rarity if rarity > 6 => return rarity,
        _ => {}
    }
    let seals = affixes.iter().map(|a| a.seal).filter(|s| *s != 0).collect::<std::collections::BTreeSet<_>>();
    affixes.len().saturating_sub(seals.len()).min(4) as u8
}
