//! The game's rules for an item that both processes enforce: the desktop when it plans an edit, the helper before it
//! stores one. They decide nothing the game decides itself.

#![forbid(unsafe_code)]

use crate::{
    definitions::{ItemLimits, ValueRange},
    forge::{AffixSnapshot, ForgeSnapshot},
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
/// Host expectation only: the native writer invokes the game's rarity method.
pub fn affix_list_rarity(item: &ForgeSnapshot, affixes: &[AffixSnapshot]) -> u8 {
    if item.rarity > 6 {
        return item.rarity;
    }
    let seals = affixes.iter().map(|a| a.seal).filter(|s| *s != 0).collect::<std::collections::BTreeSet<_>>();
    affixes.len().saturating_sub(seals.len()).min(4) as u8
}
