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
    /// `owner.name` as a destination, as the runtime crate's check says, in the helper's words when it is not one.
    ///
    /// # Safety
    /// `owner` is class metadata of this live runtime.
    pub(crate) unsafe fn mutable_field(&self, owner: Class, name: &CStr, primitive: &CStr, code: i32) -> Result<NonNull<raw::FieldInfo>, WriteError> {
        // SAFETY: the caller's contract is the runtime crate's.
        let field = unsafe { self.api.mutable_field(owner, name, primitive, code) }?.ok_or_else(|| WriteError::Refused("Not the exact mutable instance field".into()))?;
        NonNull::new(field).ok_or_else(|| WriteError::Refused("Missing mutable field".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Field, Type, test_fakes::token};
    use il2cpp_runtime::{MetadataError, metadata::unsupported};
    use std::ffi::c_char;

    #[test]
    fn a_field_that_is_not_the_destination_is_refused_in_the_helpers_words() {
        unsafe extern "C-unwind" fn class_field(_: Class, _: *const c_char) -> Field {
            token(0x2000)
        }
        unsafe extern "C-unwind" fn no_type(_: Field) -> Type {
            std::ptr::null()
        }
        let mut api = RuntimeAccess::test_stubs();
        api.class_field = class_field;
        api.field_type = no_type;
        // SAFETY: authored metadata tokens only; no object is read or written.
        let error = unsafe { api.mutable_field(token(0x1000), c"value", c"Int32", raw::type_code::I4) }.unwrap_err();
        assert_eq!(error, WriteError::Refused("Not the exact mutable instance field".into()));
    }

    #[test]
    fn a_metadata_failure_keeps_its_text_through_both_error_types() {
        let contract = || unsupported("x");
        assert_eq!(GameError::from(contract()).to_string(), contract().to_string());
        assert_eq!(GameError::from(MetadataError::NotReady("x")).to_string(), MetadataError::NotReady("x").to_string());
        assert_eq!(WriteError::from(contract()), WriteError::Refused("Unsupported IL2CPP runtime contract: x".into()));
        assert_eq!(WriteError::from(MetadataError::NotReady("x")), WriteError::NotReady("x".into()));
    }
}
