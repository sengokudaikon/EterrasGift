//! Changing the game: the one store boundary every feature writes through, and the checks a destination passes first.
//! A write is checked completely before its store. Once a store has started, every failure is [`WriteError::Failed`]:
//! the store may have happened, and nothing is retried or rolled back.

use super::{Class, GameError, RuntimeAccess};
use eg_il2cpp_runtime::raw;
use eg_trainer_wire::protocol::ResponseCode;
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
impl From<eg_il2cpp_runtime::MetadataError> for WriteError {
    fn from(error: eg_il2cpp_runtime::MetadataError) -> Self {
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
