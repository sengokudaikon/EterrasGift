//! Common contracts and a small emitter for the two before-original material replenishment leaves.
//! Both targets are i32(instance, MethodInfo): RAX/R8-R11 are scratch; RCX/RDX and the stack stay untouched.
use crate::game::*;
use native_hooks::{
    emit,
    leaf::{FRAME, GeneratedEffect, LeafEffect},
};
use std::ops::{Deref, DerefMut};

pub(super) const FLOOR: u8 = 99;
pub(super) const MAX_ENTRIES: u32 = 65536;

pub(super) unsafe fn quantity_offset(access: &RuntimeAccess, owner: Class, name: &CStr) -> Result<u32, GameError> {
    // SAFETY: reuse the generic write contract; only an aligned writable declared Int32 is accepted.
    unsafe {
        let field = access.mutable_field(owner, name, c"Int32", 0x08).map_err(|error| unsupported(error.to_string()))?;
        let offset = (access.field_offset)(field.as_ptr().cast());
        if (access.class_value)(owner) || !offset.is_multiple_of(4) {
            return Err(unsupported("material quantity layout"));
        }
        Ok(offset as u32)
    }
}

/// A material leaf's code: closed, online and a null receiver each leave the original to run unchanged.
pub(super) struct Code {
    code: emit::Code,
    online: NonNull<c_void>,
}
impl Code {
    pub fn new(online: NonNull<c_void>) -> Self {
        let f = FRAME;
        let mut code = emit::Code::default();
        code.emit(&[0x4c, 0x8b, 0x58, f.switch, 0x49, 0x83, 0x3b, 0]);
        code.exit(0x84); // closed
        code.emit(&[0x4c, 0x8b, 0x58, f.effect[0], 0x41, 0x80, 0x3b, 0]);
        code.exit(0x85); // online
        code.emit(&[0x48, 0x85, 0xc9]);
        code.exit(0x84); // null receiver: let the original decide its behavior
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
    fn deref(&self) -> &emit::Code {
        &self.code
    }
}
impl DerefMut for Code {
    fn deref_mut(&mut self) -> &mut emit::Code {
        &mut self.code
    }
}

#[cfg(test)]
mod tests;
