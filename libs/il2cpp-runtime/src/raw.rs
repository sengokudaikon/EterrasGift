//! What the exports pass and return: opaque runtime types, the attribute bits and type codes their metadata reports, and
//! the capability to store into the managed graph. No runtime structure layout is assumed. GC handles are the modern
//! pointer-valued ABI; historical uint32 handles would need a different adapter.

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;

macro_rules! opaque {
    ($($name:ident),+ $(,)?) => { $(
        #[doc = "Opaque runtime-owned type. Never construct or dereference it in Rust."]
        #[repr(C)]
        pub struct $name { _private: [u8; 0] }
    )+ };
}

opaque!(Domain, Assembly, Image, Thread, Object, MethodInfo, Exception, Class, Type, FieldInfo);

// Attribute bits returned by il2cpp_field_get_flags and il2cpp_method_get_flags, named as in
// IL2CPP's il2cpp-tabledefs.h (the ECMA-335 FieldAttributes/MethodAttributes values).
pub const FIELD_ATTRIBUTE_STATIC: i32 = 0x0010;
pub const FIELD_ATTRIBUTE_INIT_ONLY: i32 = 0x0020;
pub const FIELD_ATTRIBUTE_LITERAL: i32 = 0x0040;
pub const METHOD_ATTRIBUTE_VIRTUAL: u32 = 0x0040;
pub const METHOD_ATTRIBUTE_ABSTRACT: u32 = 0x0400;
pub const METHOD_ATTRIBUTE_PINVOKE_IMPL: u32 = 0x2000;

/// What il2cpp_type_get_type returns, named as `IL2CPP_TYPE_*` in IL2CPP's Il2CppTypeEnum (the ECMA-335 element types).
pub mod type_code {
    pub const VOID: i32 = 0x01;
    pub const BOOLEAN: i32 = 0x02;
    pub const CHAR: i32 = 0x03;
    pub const I1: i32 = 0x04;
    pub const U1: i32 = 0x05;
    pub const I2: i32 = 0x06;
    pub const U2: i32 = 0x07;
    pub const I4: i32 = 0x08;
    pub const U4: i32 = 0x09;
    pub const I8: i32 = 0x0a;
    pub const U8: i32 = 0x0b;
    pub const R4: i32 = 0x0c;
    pub const R8: i32 = 0x0d;
    pub const STRING: i32 = 0x0e;
    pub const PTR: i32 = 0x0f;
    pub const BYREF: i32 = 0x10;
    pub const VALUETYPE: i32 = 0x11;
    pub const CLASS: i32 = 0x12;
    pub const VAR: i32 = 0x13;
    pub const ARRAY: i32 = 0x14;
    pub const GENERICINST: i32 = 0x15;
    pub const TYPEDBYREF: i32 = 0x16;
    pub const I: i32 = 0x18;
    pub const U: i32 = 0x19;
    pub const FNPTR: i32 = 0x1b;
    pub const OBJECT: i32 = 0x1c;
    pub const SZARRAY: i32 = 0x1d;
    pub const MVAR: i32 = 0x1e;
}

pub type FieldSetValue = unsafe extern "C-unwind" fn(*mut Object, *mut FieldInfo, *mut c_void);
pub type ElementBarrier = unsafe extern "C-unwind" fn(*mut Object, *mut *mut c_void, *mut c_void);

/// The exports that store into the managed graph, resolved with [`crate::Exports`] but kept apart from it: code given
/// only the table cannot write.
#[derive(Clone, Copy)]
pub struct FieldWriteApi {
    pub(crate) field_set_value: FieldSetValue,
    pub(crate) element_barrier: ElementBarrier,
}

impl FieldWriteApi {
    #[cfg(any(test, feature = "test-stubs"))]
    pub fn test_export(field_set_value: FieldSetValue) -> Self {
        unsafe extern "C-unwind" fn no_element_stores(_: *mut Object, _: *mut *mut c_void, _: *mut c_void) {
            panic!("this test stores no array elements");
        }
        Self {
            field_set_value,
            element_barrier: no_element_stores,
        }
    }

    /// Store one reference in an instance field.
    ///
    /// # Safety
    /// The set_i32 runtime, thread, lifetime and exclusive mutation obligations apply, with a mutable, nonstatic
    /// reference-typed `field` of the object's class (or an ancestor) instead of Int32, and a rooted `value` of a
    /// reference class the field's type accepts.
    pub unsafe fn set_object(&self, object: NonNull<Object>, field: NonNull<FieldInfo>, value: NonNull<Object>) {
        // SAFETY: caller validates the destination metadata and the value's class. SetValueRaw(deref_pointer=false) takes
        // the object pointer itself, not the address of a pointer slot as scalar stores do.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), value.as_ptr().cast()) };
    }
    /// Store null in an instance reference field.
    ///
    /// # Safety
    /// As [`Self::set_object`], without a value.
    pub unsafe fn clear_object(&self, object: NonNull<Object>, field: NonNull<FieldInfo>) {
        // SAFETY: caller validates the destination; SetValueRaw(deref_pointer=false) stores the pointer it is given.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), std::ptr::null_mut()) };
    }

    /// Store one reference in a reference array, through the runtime's write barrier.
    ///
    /// # Safety
    /// The set_i32 runtime, thread, lifetime and exclusive mutation obligations apply. `array` is a rooted
    /// single-dimension reference array whose elements start `header` bytes in, `index` is within its length, and `value`
    /// is rooted and of a class its element type accepts.
    pub unsafe fn set_element(&self, array: NonNull<Object>, header: usize, index: usize, value: NonNull<Object>) {
        // SAFETY: caller establishes the array's layout and the slot's bounds; references are pointer-sized.
        unsafe {
            let slot = array.as_ptr().cast::<u8>().add(header + index * std::mem::size_of::<usize>()).cast::<*mut c_void>();
            (self.element_barrier)(array.as_ptr(), slot, value.as_ptr().cast());
        }
    }

    /// Store one exact Int32 instance field. This grants no general managed-memory access.
    ///
    /// # Safety
    /// The caller must be attached to this runtime on the authorized engine thread, hold a
    /// strong root for the live object, and exclude shutdown, graph mutation and reentrancy.
    /// `field` must belong to the object's validated exact class and describe a mutable,
    /// nonstatic, nonliteral, nonreadonly, nonbyref core System.Int32 (IL2CPP_TYPE_I4) field.
    /// Its metadata and storage must be valid, and this particular mutation must be authorized.
    /// A void export provides no failure/SEH recovery guarantee; the write is attempted at entry.
    pub unsafe fn set_i32(&self, object: NonNull<Object>, field: NonNull<FieldInfo>, mut value: i32) {
        // SAFETY: this per-call contract proves the exact destination and four-byte source width.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), (&mut value as *mut i32).cast()) };
    }

    /// Store one exact Byte or canonical Boolean instance field.
    /// # Safety
    /// The set_i32 ownership/runtime contract applies, with an exact mutable
    /// System.Byte/U1 or System.Boolean/BOOLEAN field instead of Int32. For
    /// Boolean the caller must supply 0 or 1. Source and storage are one byte.
    pub unsafe fn set_u8(&self, object: NonNull<Object>, field: NonNull<FieldInfo>, mut value: u8) {
        // SAFETY: caller validates the destination metadata and one-byte width.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), std::ptr::from_mut(&mut value).cast()) };
    }

    /// Store one exact UInt16 instance field.
    /// # Safety
    /// The set_i32 ownership/runtime contract applies, with an exact mutable
    /// System.UInt16/U2 field instead of Int32. Source and storage are two bytes.
    pub unsafe fn set_u16(&self, object: NonNull<Object>, field: NonNull<FieldInfo>, mut value: u16) {
        // SAFETY: caller validates the destination metadata and two-byte width.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), std::ptr::from_mut(&mut value).cast()) };
    }

    /// One exact System.Single instance-field assignment, no conversion.
    ///
    /// # Safety
    /// This export/table must belong to the same live initialized runtime as the
    /// rooted object and immutable field metadata. Caller must be attached on the
    /// authorized engine thread and exclude shutdown, graph mutation and reentrancy.
    /// The field's validated exact declaring class must be the object's validated
    /// class or its known assignable ancestor; storage must be a valid mutable,
    /// nonstatic, nonliteral, nonreadonly, nonbyref core System.Single/R4 field.
    /// Caller owns exclusive graph write permission, native lifetime and this
    /// particular store. This passes a fully initialized f32 slot to the void setter;
    /// entering it is an attempted write even if control later does not return.
    pub unsafe fn set_f32(&self, object: NonNull<Object>, field: NonNull<FieldInfo>, mut value: f32) {
        // SAFETY: caller supplies the exact R4 field and exclusive write contract.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), std::ptr::from_mut(&mut value).cast()) };
    }
}

/// A required export was absent. Partial tables are never published.
#[derive(Debug, PartialEq, Eq)]
pub struct MissingExport(pub &'static CStr);

impl std::fmt::Display for MissingExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing IL2CPP export: {}", self.0.to_string_lossy())
    }
}

impl std::error::Error for MissingExport {}
