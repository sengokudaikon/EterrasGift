//! Calling the game's own methods: the body a receiver runs for one, and one call that binds a signature-checked method,
//! invokes it and reads what it returns.
use crate::game::{GameError, Image, Method, Root, RuntimeAccess, unsupported};
use il2cpp_runtime::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use std::ptr::NonNull;

/// What a call returns when it succeeds, read out of its outcome: none when it threw or returned anything else.
pub(crate) trait FromReturn<'a, 'r>: Sized {
    fn from_return(outcome: InvocationOutcome<'a, 'r>) -> Option<Self>;
}
impl FromReturn<'_, '_> for () {
    fn from_return(outcome: InvocationOutcome<'_, '_>) -> Option<Self> {
        matches!(outcome, InvocationOutcome::Returned(ReturnValue::Void)).then_some(())
    }
}
impl FromReturn<'_, '_> for Scalar {
    fn from_return(outcome: InvocationOutcome<'_, '_>) -> Option<Self> {
        match outcome {
            InvocationOutcome::Returned(ReturnValue::Scalar { value, .. }) => Some(value),
            _ => None,
        }
    }
}
/// A finite single.
impl FromReturn<'_, '_> for f32 {
    fn from_return(outcome: InvocationOutcome<'_, '_>) -> Option<Self> {
        match outcome {
            InvocationOutcome::Returned(ReturnValue::Scalar { value: Scalar::F32(value), .. }) if value.is_finite() => Some(value),
            _ => None,
        }
    }
}
/// A reference, rooted, or none.
impl<'a, 'r> FromReturn<'a, 'r> for Option<Root<'a, 'r>> {
    fn from_return(outcome: InvocationOutcome<'a, 'r>) -> Option<Self> {
        match outcome {
            InvocationOutcome::Returned(ReturnValue::Reference(value)) => Some(value),
            _ => None,
        }
    }
}

/// Calls `method` on `receiver` with `args` and reads what it returns. `unbound` is the refusal when the method cannot be
/// bound, and `failed` when the call threw or returned something else.
///
/// # Safety
/// `method` was found by a signature-checked lookup on this runtime, `receiver` and `args` are rooted, and the caller
/// permits running the game's method on this attached thread.
pub(crate) unsafe fn call<'a, 'r, T: FromReturn<'a, 'r>>(
    attached: &'a AttachedThread<'r>,
    method: Method,
    receiver: Option<&Root<'_, '_>>,
    args: &mut [Argument<'_, '_, '_>],
    unbound: &str,
    failed: &str,
) -> Result<T, GameError> {
    // SAFETY: as this function's; the context roots the receiver, arguments and a returned reference across the call.
    unsafe {
        let context = InvocationContext::new(attached);
        let method = context
            .bind(NonNull::new(method.cast_mut()).ok_or_else(|| unsupported(unbound))?)
            .map_err(|e| unsupported(e.to_string()))?;
        T::from_return(method.invoke(receiver, args).map_err(|e| unsupported(e.to_string()))?).ok_or_else(|| unsupported(failed))
    }
}

impl RuntimeAccess {
    /// The body `receiver` runs for `method` (`Exports::dispatched`), when it is the game's own: an override from
    /// another image than `game`, a mod's, is none.
    ///
    /// # Safety
    /// An attached thread of this runtime, `receiver` live, and `method` a method of one of its classes.
    pub(crate) unsafe fn game_body(&self, game: Image, receiver: NonNull<raw::Object>, method: Method) -> Option<Method> {
        // SAFETY: as this function's; the override's descriptor is retained metadata.
        unsafe {
            let body = self.dispatched(receiver, method)?;
            (body == method || (self.class_image)((self.method_class)(body)) == game).then_some(body)
        }
    }
}
