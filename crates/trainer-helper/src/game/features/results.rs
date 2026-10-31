//! Scalar-result features over the shared original-first emitter.
use super::{postfix::Code, registry::MethodSpec};
use crate::{game::*, hooks::postfix::PostfixEffect};

pub(crate) mod ability_speed;
pub(crate) mod boss_loot_chance;
pub(crate) mod max_bonus_stability;
pub(crate) mod max_random_rolls;
pub(crate) mod movement_speed;
pub(crate) mod no_mana_cost;
pub(crate) mod one_echo_type;
pub(crate) mod tier_7_affixes;

#[cfg(test)]
mod tests;
