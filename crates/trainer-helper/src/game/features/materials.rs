//! Common contracts for the two before-original material replenishment leaves.
//! Both targets are i32(instance, MethodInfo): RAX/R8-R11 are scratch; RCX/RDX and the stack stay untouched.
use crate::game::*;

pub(super) const FLOOR: u8 = 99;
pub(super) const MAX_ENTRIES: u32 = 65536;

pub(super) unsafe fn quantity_offset(access: &RuntimeAccess, owner: Class, name: &CStr) -> Result<u32, GameError> {
    // SAFETY: reuse the generic write contract; only an aligned writable declared Int32 is accepted.
    unsafe {
        let field = access
            .mutable_field(owner, name, c"Int32", raw::type_code::I4)
            .map_err(|error| unsupported(error.to_string()))?;
        let offset = (access.field_offset)(field.as_ptr());
        if (access.class_value)(owner) || !offset.is_multiple_of(4) {
            return Err(unsupported("material quantity layout"));
        }
        Ok(offset as u32)
    }
}

#[cfg(test)]
mod tests;
