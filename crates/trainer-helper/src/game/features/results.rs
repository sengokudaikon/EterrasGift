//! Shared original-first result emission. Features own their signatures, predicates and transformations.
use super::registry::MethodSpec;
use crate::{
    game::*,
    hooks::{
        emit,
        postfix::{FRAME, PostfixEffect},
    },
};
use std::ops::{Deref, DerefMut};

/// A result's code: closed or online leave the original's result as it is.
pub(super) struct Code {
    code: emit::Code,
    online: NonNull<c_void>,
}
impl Code {
    pub fn new(online: NonNull<c_void>) -> Self {
        let mut code = emit::Code::default();
        code.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
        code.exit(0x84);
        code.emit(&[0x4c, 0x8b, 0x58, FRAME.slots[0], 0x41, 0x80, 0x3b, 0]);
        code.exit(0x85);
        Self { code, online }
    }
    pub fn finish(self, spec: &MethodSpec, extra: u64) -> PostfixEffect {
        PostfixEffect {
            code: self.code.finish(),
            slots: [self.online.as_ptr() as u64, extra],
            positions: spec.positions(),
        }
    }
}
impl Deref for Code {
    type Target = emit::Code;
    fn deref(&self) -> &emit::Code {
        &self.code
    }
}
impl DerefMut for Code {
    fn deref_mut(&mut self) -> &mut emit::Code {
        &mut self.code
    }
}

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
