//! Narrow IL2CPP boundary. Raw declarations are separate from thread and root ownership.

pub mod discovery;
mod invoke;
pub mod raw;
mod thread;

pub use invoke::{Argument, CheckedMethod, InvocationContext, InvocationOutcome, InvokeError, ReturnValue, Scalar, SignaturePosition};
pub use thread::{AttachedThread, RuntimeError, StrongRoot, ThreadContext};
