//! What a caller states before using a method's code or a field's storage directly: a method's exact signature, a
//! field's offset, a destination's exact type, and the bounded window of code to read. Nothing here calls a managed
//! method or writes.

use super::{Class, Exports, MAX_FIELD_OFFSET, MetadataError, Method, MethodError, MethodQuery, Others, Params, Ptr, Returns, instance_offset, read_value, unsupported};
use crate::raw;
use std::{
    ffi::{CStr, c_void},
    ptr::NonNull,
};

/// One method's stated signature: its class and name, whether it takes a receiver, and its parameter and return types
/// by name (a trailing `&` is by reference).
pub struct MethodSpec {
    pub class: &'static CStr,
    pub method: &'static CStr,
    pub instance: bool,
    pub params: &'static [&'static str],
    pub returns: &'static str,
}
impl MethodSpec {
    /// Its native argument positions: the receiver when it has one, its parameters, and the MethodInfo.
    pub const fn positions(&self) -> u8 {
        let positions = self.params.len() + self.instance as usize + 1;
        assert!(positions <= u8::MAX as usize, "bounded explicit method contract");
        positions as u8
    }
}

impl Exports {
    /// The one method of `owner` with exactly `spec`'s signature: nongeneric, not P/Invoke or abstract, with its return
    /// type. The complete parameter signature is resolved before the selected overload's ABI and uniqueness are checked.
    ///
    /// # Safety
    /// Immutable attached metadata and a class descriptor retained by the caller.
    pub unsafe fn hook_method_contract(&self, owner: Class, spec: &MethodSpec) -> Result<Method, MetadataError> {
        let query = MethodQuery {
            params: Params::Named(spec.params),
            others: Others::Overloads,
            instance: Some(spec.instance),
            returns: Returns::Named(spec.returns),
            refused: raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT,
            refuse_generic: true,
            refuse_inflated: true,
            ..MethodQuery::named(spec.method, spec.params.len())
        };
        // SAFETY: immutable attached metadata and class descriptor retained by the caller.
        unsafe { self.find_method(owner, &query) }.map_err(|error| {
            let name = spec.method.to_string_lossy();
            match error {
                MethodError::Metadata(error) => error,
                MethodError::Unnamed => unsupported("null hook method name"),
                MethodError::Untyped => unsupported("null hook parameter type"),
                MethodError::Missing => unsupported(format!("missing hook: {name}")),
                MethodError::Twice | MethodError::Signature | MethodError::Parameters => unsupported(format!("hook contract: {name}")),
            }
        })
    }
    /// The offset of `owner`'s instance field `name`, whose type's class is exactly `expected`, aligned to `alignment`
    /// in a reference object's instance storage.
    /// # Safety
    /// Immutable attached metadata; `owner` and `expected` are retained class descriptors.
    pub unsafe fn hook_field_of(&self, owner: Class, name: &CStr, expected: Class, alignment: usize) -> Result<u32, MetadataError> {
        // SAFETY: exact non-byref field class and instance storage are checked before returning the aligned offset.
        unsafe {
            let field = self.instance_field_in(owner, name, expected, "hook")?;
            let offset = (self.field_offset)(field);
            if (self.class_value)(owner) || !instance_offset(offset) || !offset.is_multiple_of(alignment) {
                return Err(unsupported(format!("hook field layout: {}", name.to_string_lossy())));
            }
            Ok(offset as u32)
        }
    }
    /// The offset of `owner`'s instance field `name`, whose non-byref type is named `expected`, aligned to `alignment` in a
    /// reference object's instance storage.
    ///
    /// # Safety
    /// Immutable attached metadata; `owner` is a retained class descriptor.
    pub unsafe fn hook_field(&self, owner: Class, name: &CStr, expected: &str, alignment: usize) -> Result<u32, MetadataError> {
        // SAFETY: immutable attached metadata, instance fields on readable managed objects.
        unsafe {
            let field = self.field(owner, name)?;
            let offset = (self.field_offset)(field);
            let ty = (self.field_type)(field);
            if (self.class_value)(owner)
                || (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
                || ty.is_null()
                || (self.type_byref)(ty)
                || self.type_name(ty)? != expected
                || !instance_offset(offset)
                || !offset.is_multiple_of(alignment)
            {
                return Err(unsupported(format!("hook field layout: {}", name.to_string_lossy())));
            }
            Ok(offset as u32)
        }
    }
    /// The storage of `owner`'s static `field`, aligned to `alignment`, for code that reads it without calling the runtime.
    ///
    /// # Safety
    /// Attached thread of this initialized runtime; `field` is a static field of `owner` with validated type and flags, and
    /// its value fits in eight bytes.
    pub unsafe fn hook_static_storage(&self, owner: Class, field: Ptr, alignment: usize) -> Result<NonNull<c_void>, MetadataError> {
        // SAFETY: static field type/flags validated by caller; initialize through the public field API, then obtain
        // the owning storage through the public class API. The primitive fits in this eight-byte snapshot.
        unsafe {
            let mut snapshot = 0u64;
            (self.static_get)(field, (&raw mut snapshot).cast());
            let offset = (self.field_offset)(field);
            let storage = (self.class_static_data)(owner);
            if storage.is_null() || offset > MAX_FIELD_OFFSET || !offset.is_multiple_of(alignment) {
                return Err(unsupported("hook static storage layout"));
            }
            let pointer = storage.cast::<u8>().add(offset);
            if !(pointer as usize).is_multiple_of(alignment) {
                return Err(unsupported("hook static storage alignment"));
            }
            Ok(NonNull::new(pointer.cast()).unwrap())
        }
    }

    /// `owner.name` as a destination: an instance field declared on `owner`, of exactly the primitive `primitive`
    /// (type code `code`), neither static, read-only nor literal, inside the object; `None` when the field is not that.
    ///
    /// # Safety
    /// `owner` is class metadata of this live runtime.
    pub unsafe fn mutable_field(&self, owner: Class, name: &CStr, primitive: &CStr, code: i32) -> Result<Option<Ptr>, MetadataError> {
        // SAFETY: The owner is retained metadata; exact type, flags and declaring-class checks precede returning a writable field.
        unsafe {
            let field = self.field(owner, name)?;
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.type_byref)(ty)
                || (self.type_code)(ty) != code
                || (self.type_class)(ty) != self.system_class(primitive)?
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_INIT_ONLY | raw::FIELD_ATTRIBUTE_LITERAL) != 0
                || !instance_offset((self.field_offset)(field))
            {
                return Ok(None);
            }
            Ok(Some(field))
        }
    }

    /// Where a read of the code at `start` stops: `limit` bytes on, clipped at `module_end` and at the next method `owner`
    /// declares. A managed method may span several Windows unwind records (its EH funclets), so the window is a stated
    /// bound and not an unwind extent.
    ///
    /// # Safety
    /// The lookup contract ([`crate::metadata`]), with `MethodInfo.methodPointer` at offset 0.
    pub unsafe fn code_end(&self, owner: Class, start: usize, limit: usize, module_end: usize) -> Result<usize, MetadataError> {
        // SAFETY: the owner's method descriptors are retained metadata, each starting with its code pointer.
        unsafe {
            let mut end = start.saturating_add(limit).min(module_end);
            for next in self.methods(owner) {
                let next = read_value::<*mut u8>(next?.cast()) as usize;
                if next > start {
                    end = end.min(next);
                }
            }
            Ok(end)
        }
    }
}

#[cfg(test)]
mod tests;
