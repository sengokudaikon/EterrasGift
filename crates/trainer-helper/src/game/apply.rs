//! Checked one-time object edits: destination validation and the shared store/readback boundary. Persistent scalar
//! effects reuse destination checks and own restoration through their gateway lifecycle. Once a requested edit's store
//! starts, every failure is [`WriteError::Failed`]: the store may have happened, and nothing is retried or rolled back.

use super::{Class, GameError, RuntimeAccess};
use eg_trainer_wire::protocol::ResponseCode;
use il2cpp_runtime::raw;
use std::{ffi::CStr, fmt, ptr::NonNull};

/// Why a write was not confirmed. Only `Failed` may describe a write whose store was attempted; no variant permits a
/// retry.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    NotReady(String),
    Stale(String),
    Refused(String),
    Offline,
    Failed(String),
}

impl WriteError {
    /// The reply code the helper sends for it.
    pub fn code(&self) -> ResponseCode {
        match self {
            Self::NotReady(_) => ResponseCode::NotReady,
            Self::Stale(_) => ResponseCode::StaleItem,
            Self::Refused(_) => ResponseCode::Refused,
            Self::Offline => ResponseCode::OfflineRequired,
            Self::Failed(_) => ResponseCode::Failed,
        }
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady(text) | Self::Stale(text) | Self::Refused(text) | Self::Failed(text) => f.write_str(text),
            Self::Offline => f.write_str("An offline character with a live item manager is required"),
        }
    }
}
impl std::error::Error for WriteError {}

impl From<GameError> for WriteError {
    fn from(error: GameError) -> Self {
        match error {
            GameError::OfflineRequired => Self::Offline,
            GameError::NotReady(reason) => Self::NotReady(reason.into()),
            GameError::Runtime(error) => Self::NotReady(error.to_string()),
            error => Self::Refused(error.to_string()),
        }
    }
}
impl From<il2cpp_runtime::MetadataError> for WriteError {
    fn from(error: il2cpp_runtime::MetadataError) -> Self {
        GameError::from(error).into()
    }
}

/// A failure after the store boundary: the store may have happened.
pub(crate) fn uncertain(error: impl fmt::Display) -> WriteError {
    WriteError::Failed(format!("Store attempted; readback unconfirmed: {error}"))
}

/// The store boundary. `store` changes the game and `readback` reads the result back; once `store` has started, every
/// failure is uncertain. The caller has checked every destination and keeps its objects rooted throughout.
pub(crate) fn stored<T>(store: impl FnOnce() -> Result<(), WriteError>, readback: impl FnOnce() -> Result<T, WriteError>) -> Result<T, WriteError> {
    store().and_then(|()| readback()).map_err(uncertain)
}

impl RuntimeAccess {
    /// `owner.name` as a destination: an instance field declared on `owner`, of exactly the primitive `primitive`
    /// (type code `code`), neither static, read-only nor literal, inside the object.
    ///
    /// # Safety
    /// `owner` is class metadata of this live runtime.
    pub(crate) unsafe fn mutable_field(&self, owner: Class, name: &CStr, primitive: &CStr, code: i32) -> Result<NonNull<raw::FieldInfo>, WriteError> {
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
                || !(16..=16384).contains(&(self.field_offset)(field))
            {
                return Err(WriteError::Refused("Not the exact mutable instance field".into()));
            }
            NonNull::new(field.cast()).ok_or_else(|| WriteError::Refused("Missing mutable field".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Ptr, Type, test_fakes::token};
    use std::{
        cell::Cell,
        ffi::{c_char, c_void},
    };

    const OWNER: usize = 0x1000;
    const FIELD: usize = 0x2000;
    const INT32: usize = 0x3000;
    thread_local! {
        static MISSING: Cell<bool> = const { Cell::new(false) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static CODE: Cell<i32> = const { Cell::new(0x08) };
        static TYPE_CLASS: Cell<usize> = const { Cell::new(INT32) };
        static DECLARING: Cell<usize> = const { Cell::new(OWNER) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static OFFSET: Cell<usize> = const { Cell::new(144) };
    }
    unsafe extern "C-unwind" fn corlib() -> *const c_void {
        token::<c_void>(0x4000).cast_const()
    }
    unsafe extern "C-unwind" fn class_from_name(_: *const c_void, namespace: *const c_char, name: *const c_char) -> Class {
        // SAFETY: system_class supplies static nul-terminated names.
        assert_eq!(unsafe { (CStr::from_ptr(namespace), CStr::from_ptr(name)) }, (c"System", c"Int32"));
        token(INT32)
    }
    unsafe extern "C-unwind" fn class_field(owner: Class, _: *const c_char) -> Ptr {
        assert_eq!(owner as usize, OWNER);
        if MISSING.get() { std::ptr::null_mut() } else { token(FIELD) }
    }
    unsafe extern "C-unwind" fn class_parent(_: Class) -> Class {
        std::ptr::null_mut()
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> Type {
        token(0x5000)
    }
    unsafe extern "C-unwind" fn type_byref(_: Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn type_code(_: Type) -> i32 {
        CODE.get()
    }
    unsafe extern "C-unwind" fn type_class(_: Type) -> Class {
        token(TYPE_CLASS.get())
    }
    unsafe extern "C-unwind" fn field_parent(_: Ptr) -> Class {
        token(DECLARING.get())
    }
    unsafe extern "C-unwind" fn field_flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn field_offset(_: Ptr) -> usize {
        OFFSET.get()
    }
    fn reset() {
        MISSING.set(false);
        BYREF.set(false);
        CODE.set(0x08);
        TYPE_CLASS.set(INT32);
        DECLARING.set(OWNER);
        FLAGS.set(0);
        OFFSET.set(144);
    }

    #[test]
    fn mutable_field_is_exactly_the_declared_writable_instance_primitive_inside_the_object() {
        let mut api = RuntimeAccess::test_stubs();
        api.corlib = corlib;
        api.class_from_name = class_from_name;
        api.class_field = class_field;
        api.class_parent = class_parent;
        api.field_type = field_type;
        api.type_byref = type_byref;
        api.type_code = type_code;
        api.type_class = type_class;
        api.field_parent = field_parent;
        api.field_flags = field_flags;
        api.field_offset = field_offset;
        // SAFETY: authored metadata tokens only; no object is read or written.
        let field = || unsafe { api.mutable_field(token(OWNER), c"value", c"Int32", 0x08) };
        assert_eq!(field().unwrap().as_ptr() as usize, FIELD);
        for offset in [16, 16384] {
            OFFSET.set(offset);
            assert!(field().is_ok(), "offset {offset}");
        }
        let faults: &[fn()] = &[
            || MISSING.set(true),
            || BYREF.set(true),
            || CODE.set(0x0a),
            || TYPE_CLASS.set(INT32 + 1),
            || DECLARING.set(OWNER + 1),
            || FLAGS.set(0x10),
            || FLAGS.set(0x20),
            || FLAGS.set(0x40),
            || FLAGS.set(0x70),
            || OFFSET.set(0),
            || OFFSET.set(15),
            || OFFSET.set(16385),
            || OFFSET.set(usize::MAX),
        ];
        for (case, fault) in faults.iter().enumerate() {
            reset();
            fault();
            assert!(matches!(field(), Err(WriteError::Refused(_))), "case {case}");
        }
    }
}
