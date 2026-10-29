//! Shared original-first result emission. Features own their signatures, predicates and transformations.
use super::hooks::MethodSpec;
use crate::{
    game::*,
    hooks::postfix::{FRAME, PostfixEffect},
};

pub(super) struct Code {
    bytes: Vec<u8>,
    exits: Vec<usize>,
    online: NonNull<c_void>,
}
impl Code {
    pub fn new(online: NonNull<c_void>) -> Self {
        let mut c = Self {
            bytes: Vec::new(),
            exits: Vec::new(),
            online,
        };
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
        c.exit(0x84);
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.slots[0], 0x41, 0x80, 0x3b, 0]);
        c.exit(0x85);
        c
    }
    pub fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    pub fn offset(&mut self, bytes: &[u8], offset: u32) {
        self.emit(bytes);
        self.emit(&offset.to_le_bytes());
    }
    pub fn branch(&mut self, condition: u8) -> usize {
        self.emit(&[0x0f, condition]);
        let at = self.bytes.len();
        self.emit(&[0; 4]);
        at
    }
    pub fn exit(&mut self, condition: u8) {
        let at = self.branch(condition);
        self.exits.push(at);
    }
    pub fn done(&mut self) {
        self.emit(&[0xe9]);
        self.exits.push(self.bytes.len());
        self.emit(&[0; 4]);
    }
    pub fn bind(&mut self, at: usize) {
        let displacement = i32::try_from(self.bytes.len() as isize - (at + 4) as isize).expect("bounded result effect");
        self.bytes[at..at + 4].copy_from_slice(&displacement.to_le_bytes());
    }
    pub fn nonnull(&mut self, test: &[u8]) {
        self.emit(test);
        self.exit(0x84);
    }
    pub fn finish(mut self, spec: &MethodSpec, extra: u64) -> PostfixEffect {
        for at in std::mem::take(&mut self.exits) {
            self.bind(at);
        }
        PostfixEffect {
            code: self.bytes,
            slots: [self.online.as_ptr() as u64, extra],
            positions: u8::try_from(spec.params.len() + usize::from(spec.instance) + 1).expect("bounded explicit method contract"),
        }
    }
}

pub(crate) mod ability_speed;
pub(crate) mod boss_loot_chance;
pub(crate) mod max_bonus_stability;
pub(crate) mod max_random_rolls;
pub(crate) mod one_echo_type;
pub(crate) mod tier_7_affixes;

#[cfg(test)]
mod tests;
