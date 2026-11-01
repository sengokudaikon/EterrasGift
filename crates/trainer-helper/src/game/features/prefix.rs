//! Before-original admission for argument adjustments and scalar overrides. Features own their signatures and
//! transformations. A tail jump keeps indirect aggregates and long signatures in their original argument locations;
//! no generated frame, callback, hidden-result adaptation or helper lifetime is involved.
use crate::game::*;
use native_hooks::{
    emit,
    leaf::{FRAME, GeneratedEffect, LeafEffect},
};
use std::ops::{Deref, DerefMut};

pub(super) struct Code {
    code: emit::Code,
    online: NonNull<c_void>,
}
impl Code {
    pub fn new(online: NonNull<c_void>) -> Self {
        let mut code = emit::Code::default();
        code.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
        code.exit(0x84);
        code.emit(&[0x4c, 0x8b, 0x58, FRAME.effect[0], 0x41, 0x80, 0x3b, 0]);
        code.exit(0x85);
        Self { code, online }
    }
    pub fn finish(self) -> LeafEffect {
        LeafEffect::Generated(GeneratedEffect {
            code: self.code.finish(),
            slots: [self.online.as_ptr() as u64, 0, 0, 0],
        })
    }
}
impl Deref for Code {
    type Target = emit::Code;
    fn deref(&self) -> &Self::Target {
        &self.code
    }
}
impl DerefMut for Code {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.code
    }
}

#[cfg(test)]
mod tests;
