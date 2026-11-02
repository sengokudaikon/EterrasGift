//! A small x64 assembler for generated effects: bytes, rel32 conditional branches bound later, and exits that all
//! land at the end of the code, where the caller's gateway continues (to the original, or past it). [`LeafCode`] and
//! [`PostfixCode`] open a gated effect in its family's frame and close it into that family's effect.
use super::{
    leaf::{self, GeneratedEffect, LeafEffect},
    postfix::{self, PostfixEffect},
};
use std::{
    ffi::c_void,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

#[derive(Default)]
pub struct Code {
    pub bytes: Vec<u8>,
    exits: Vec<usize>,
}

impl Code {
    pub fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    /// An instruction whose last operand is a 32-bit field offset.
    pub fn offset(&mut self, instruction: &[u8], offset: u32) {
        self.emit(instruction);
        self.emit(&offset.to_le_bytes());
    }
    /// A `0f cc rel32` branch, bound later by `bind` or `bind_at`; its displacement's place in the code.
    pub fn branch(&mut self, condition: u8) -> usize {
        self.emit(&[0x0f, condition]);
        let at = self.bytes.len();
        self.emit(&[0; 4]);
        at
    }
    /// A branch to the end of the code.
    pub fn exit(&mut self, condition: u8) {
        let at = self.branch(condition);
        self.exits.push(at);
    }
    /// An unconditional jump to the end of the code.
    pub fn done(&mut self) {
        let at = self.jump();
        self.exits.push(at);
    }
    /// An unconditional rel32 jump, bound later in the same way as a conditional branch.
    pub fn jump(&mut self) -> usize {
        self.emit(&[0xe9]);
        let at = self.bytes.len();
        self.emit(&[0; 4]);
        at
    }
    /// A test, then an exit when it sets ZF (a null register).
    pub fn nonnull(&mut self, test: &[u8]) {
        self.emit(test);
        self.exit(0x84);
    }
    /// A gated effect's admission, for a frame's displacements from RAX: exit when the word at the address `switch` holds
    /// is zero, or the byte at the address `veto` holds is not. Scratches R11 and the flags.
    pub fn gated(&mut self, switch: u8, veto: u8) {
        self.emit(&[0x4c, 0x8b, 0x58, switch, 0x49, 0x83, 0x3b, 0]); // mov r11,[rax+switch]; cmp qword [r11],0
        self.exit(0x84);
        self.emit(&[0x4c, 0x8b, 0x58, veto, 0x41, 0x80, 0x3b, 0]); // mov r11,[rax+veto]; cmp byte [r11],0
        self.exit(0x85);
    }
    /// R11 is the reference stored at the address `slot` holds, a displacement from RAX; exit when it is null.
    pub fn reference(&mut self, slot: u8) {
        self.emit(&[0x4c, 0x8b, 0x58, slot, 0x4d, 0x8b, 0x1b]); // mov r11,[rax+slot]; mov r11,[r11]
        self.nonnull(&[0x4d, 0x85, 0xdb]);
    }
    /// One attempt at a coherent two-word settings snapshot, in XMM4/XMM5 (low 64 bits each). RAX is the data page;
    /// scratch R11 and flags. On odd/changed version exit without applying any writes. x64 ordered atomic loads pair
    /// with the serialized writer's sequentially consistent atomic stores; no payload is accessed nonatomically.
    pub fn snapshot(&mut self, [version, low, high]: [u8; 3]) {
        self.emit(&[0x4c, 0x8b, 0x58, version, 0x41, 0xf6, 0xc3, 1]);
        self.exit(0x85);
        self.emit(&[0xf3, 0x0f, 0x7e, 0x60, low, 0xf3, 0x0f, 0x7e, 0x68, high]);
        self.emit(&[0x4c, 0x3b, 0x58, version]);
        self.exit(0x85);
    }
    /// The branch at `at` lands here.
    pub fn bind(&mut self, at: usize) {
        self.bind_at(at, self.bytes.len());
    }
    /// The branch at `at` lands at `target`, before or after it.
    pub fn bind_at(&mut self, at: usize, target: usize) {
        let displacement = i32::try_from(target as isize - (at + 4) as isize).expect("bounded generated effect");
        self.bytes[at..at + 4].copy_from_slice(&displacement.to_le_bytes());
    }
    /// The code, every exit bound to its end.
    pub fn finish(mut self) -> Vec<u8> {
        for at in std::mem::take(&mut self.exits) {
            self.bind(at);
        }
        self.bytes
    }
}

/// A before-original leaf's code, for argument adjustments and overrides: closed or vetoed, the original runs as it was
/// called. A tail jump keeps indirect aggregates and long signatures in their original argument locations; no generated
/// frame, callback or hidden-result adaptation is involved. `veto` is the address of a byte, kept in the effect's first
/// slot, that is nonzero when the effect must stand aside. It stays readable until the process exits.
pub struct LeafCode {
    code: Code,
    veto: NonNull<c_void>,
}
impl LeafCode {
    pub fn new(veto: NonNull<c_void>) -> Self {
        let mut code = Code::default();
        code.gated(leaf::FRAME.switch, leaf::FRAME.effect[0]);
        Self { code, veto }
    }
    /// As `new`, and a null receiver in RCX also leaves the original to decide what happens.
    pub fn for_receiver(veto: NonNull<c_void>) -> Self {
        let mut code = Self::new(veto);
        code.nonnull(&[0x48, 0x85, 0xc9]); // test rcx,rcx
        code
    }
    pub fn finish(self) -> LeafEffect {
        self.finish_with([0; 3])
    }
    /// The effect, with these initial values in its remaining slots.
    pub fn finish_with(self, extra: [u64; 3]) -> LeafEffect {
        LeafEffect::Generated(GeneratedEffect {
            code: self.code.finish(),
            slots: [self.veto.as_ptr() as u64, extra[0], extra[1], extra[2]],
        })
    }
}
impl Deref for LeafCode {
    type Target = Code;
    fn deref(&self) -> &Code {
        &self.code
    }
}
impl DerefMut for LeafCode {
    fn deref_mut(&mut self) -> &mut Code {
        &mut self.code
    }
}

/// An original-first effect's code: closed or vetoed, the original's result stands. `veto` is as for [`LeafCode`].
pub struct PostfixCode {
    code: Code,
    veto: NonNull<c_void>,
}
impl PostfixCode {
    pub fn new(veto: NonNull<c_void>) -> Self {
        let mut code = Code::default();
        code.gated(postfix::FRAME.switch, postfix::FRAME.slots[0]);
        Self { code, veto }
    }
    /// The effect for a target of `positions` native positions, with `extra` initially in its second slot.
    pub fn finish(self, positions: u8, extra: u64) -> PostfixEffect {
        PostfixEffect {
            before: Vec::new(),
            code: self.code.finish(),
            slots: [self.veto.as_ptr() as u64, extra],
            positions,
        }
    }
}
impl Deref for PostfixCode {
    type Target = Code;
    fn deref(&self) -> &Code {
        &self.code
    }
}
impl DerefMut for PostfixCode {
    fn deref_mut(&mut self) -> &mut Code {
        &mut self.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VETO: NonNull<c_void> = NonNull::without_provenance(std::num::NonZero::new(0x1000).unwrap());

    /// switch at `s`, veto at `v`: each check is eight bytes and a six-byte exit, the exits landing past `tail` bytes.
    fn admission(s: u8, v: u8, tail: u8) -> Vec<u8> {
        let mut bytes = vec![0x4c, 0x8b, 0x58, s, 0x49, 0x83, 0x3b, 0, 0x0f, 0x84, 14 + tail, 0, 0, 0];
        bytes.extend([0x4c, 0x8b, 0x58, v, 0x41, 0x80, 0x3b, 0, 0x0f, 0x85, tail, 0, 0, 0]);
        bytes
    }

    #[test]
    fn leaf_effects_are_admitted_in_the_leaf_frame_and_keep_their_slots() {
        let mut code = LeafCode::new(VETO);
        code.emit(&[0x90]);
        let LeafEffect::Generated(effect) = code.finish_with([7, 8, 9]) else { unreachable!() };
        let mut expected = admission(leaf::FRAME.switch, leaf::FRAME.effect[0], 1);
        expected.push(0x90);
        assert_eq!(effect.code, expected);
        assert_eq!(effect.slots, [0x1000, 7, 8, 9]);
    }

    #[test]
    fn a_leaf_for_a_receiver_also_exits_on_null_rcx() {
        let LeafEffect::Generated(effect) = LeafCode::for_receiver(VETO).finish() else {
            unreachable!()
        };
        let mut expected = admission(leaf::FRAME.switch, leaf::FRAME.effect[0], 9);
        expected.extend([0x48, 0x85, 0xc9, 0x0f, 0x84, 0, 0, 0, 0]);
        assert_eq!(effect.code, expected);
        assert_eq!(effect.slots, [0x1000, 0, 0, 0]);
    }

    #[test]
    fn postfix_effects_are_admitted_in_the_postfix_frame_and_keep_their_slots() {
        let effect = PostfixCode::new(VETO).finish(5, 42);
        assert_eq!(effect.code, admission(postfix::FRAME.switch, postfix::FRAME.slots[0], 0));
        assert_eq!(effect.slots, [0x1000, 42]);
        assert_eq!(effect.positions, 5);
        assert!(effect.before.is_empty());
    }

    #[test]
    fn a_reference_loads_through_its_slot_and_exits_when_null() {
        let mut code = Code::default();
        code.reference(0x28);
        assert_eq!(code.finish(), [0x4c, 0x8b, 0x58, 0x28, 0x4d, 0x8b, 0x1b, 0x4d, 0x85, 0xdb, 0x0f, 0x84, 0, 0, 0, 0]);
    }

    #[test]
    fn branches_bind_forward_and_back_and_exits_land_at_the_end() {
        let mut c = Code::default();
        let top = c.bytes.len();
        c.emit(&[0x90]);
        let forward = c.branch(0x84);
        c.exit(0x85);
        c.bind(forward);
        let back = c.branch(0x8e);
        c.bind_at(back, top);
        c.done();
        let code = c.finish();
        let rel = |at: usize| i32::from_le_bytes(code[at..at + 4].try_into().unwrap());
        assert_eq!(rel(3), 6, "forward lands after the exit");
        assert_eq!(rel(9), 24 - 13, "the exit lands at the end");
        assert_eq!(rel(15), -19, "back lands at the top");
        assert_eq!(rel(20), 0, "done lands at the end");
        assert_eq!(code.len(), 24);
    }
}
