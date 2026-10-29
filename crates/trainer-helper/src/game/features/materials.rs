//! Common contracts and a small emitter for the two before-original material replenishment leaves.
//! Both targets are i32(instance, MethodInfo): RAX/R8-R11 are scratch; RCX/RDX and the stack stay untouched.
use crate::{
    game::*,
    hooks::leaf::{FRAME, GeneratedEffect, LeafEffect},
};

pub(super) const FLOOR: u8 = 99;
pub(super) const MAX_ENTRIES: u32 = 65536;

pub(super) unsafe fn online_storage(access: &RuntimeAccess, images: &Images) -> Result<NonNull<c_void>, GameError> {
    // SAFETY: admitted attached thread and retained metadata; initialize the process-owned Boolean static.
    unsafe {
        let environment = access.class(images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
        let online = access.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
        access.hook_static_storage(environment, online, 1)
    }
}

pub(super) unsafe fn field_offset(access: &RuntimeAccess, owner: Class, name: &CStr, expected: Class, alignment: usize) -> Result<u32, GameError> {
    // SAFETY: exact non-byref field class and instance storage are checked before returning the aligned offset.
    unsafe {
        let field = access.instance_field_in(owner, name, expected, "materials")?;
        let offset = (access.field_offset)(field);
        if (access.class_value)(owner) || !(16..=16384).contains(&offset) || !offset.is_multiple_of(alignment) {
            return Err(unsupported("material field layout"));
        }
        Ok(offset as u32)
    }
}

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

pub(super) struct Code {
    pub bytes: Vec<u8>,
    exits: Vec<usize>,
    online: NonNull<c_void>,
}
impl Code {
    pub fn new(online: NonNull<c_void>) -> Self {
        let f = FRAME;
        let mut code = Self {
            bytes: Vec::new(),
            exits: Vec::new(),
            online,
        };
        code.emit(&[0x4c, 0x8b, 0x58, f.switch, 0x49, 0x83, 0x3b, 0]);
        code.exit(0x84); // closed
        code.emit(&[0x4c, 0x8b, 0x58, f.effect[0], 0x41, 0x80, 0x3b, 0]);
        code.exit(0x85); // online
        code.emit(&[0x48, 0x85, 0xc9]);
        code.exit(0x84); // null receiver: let the original decide its behavior
        code
    }
    pub fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    pub fn offset(&mut self, instruction: &[u8], offset: u32) {
        self.emit(instruction);
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
    pub fn bind(&mut self, at: usize) {
        self.bind_at(at, self.bytes.len());
    }
    pub fn bind_at(&mut self, at: usize, target: usize) {
        let displacement = i32::try_from(target as isize - (at + 4) as isize).expect("bounded material leaf");
        self.bytes[at..at + 4].copy_from_slice(&displacement.to_le_bytes());
    }
    pub fn finish(mut self) -> LeafEffect {
        for at in std::mem::take(&mut self.exits) {
            self.bind(at);
        }
        LeafEffect::Generated(GeneratedEffect {
            code: self.bytes,
            slots: [self.online.as_ptr() as u64, 0, 0, 0],
        })
    }
}

#[cfg(test)]
mod tests;
