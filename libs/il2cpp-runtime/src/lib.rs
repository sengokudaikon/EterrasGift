//! A live IL2CPP runtime, reached only through its public exports. `raw` declares the exports and the opaque types they
//! pass; `thread` owns attachment and strong GC roots; `metadata` looks up images, classes, fields and methods, checks
//! each against the contract its caller states and reads objects through them; `invoke` calls a method whose signature
//! was checked first; `unity` asks what Unity adds to it; `module` finds the loaded module those exports come from.
//! Everything here is game-neutral: what a game's classes mean belongs to the code that names them.

mod invoke;
pub mod metadata;
#[cfg(windows)]
pub mod module;
pub mod raw;
#[cfg(any(test, feature = "test-stubs"))]
pub mod test_fakes;
mod thread;
mod unity;

pub use invoke::{Argument, CheckedMethod, InvocationContext, InvocationOutcome, InvokeError, ReturnValue, Scalar, SignaturePosition, ValueTypeContract};
pub use metadata::{MetadataApi, MetadataError};
pub use thread::{AttachedThread, RuntimeError, StrongRoot, ThreadContext};
