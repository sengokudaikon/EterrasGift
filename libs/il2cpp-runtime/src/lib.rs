//! A live IL2CPP runtime, reached only through its public exports. `raw` declares the exports and the opaque types they
//! pass; `thread` owns attachment and strong GC roots; `metadata` looks up images, classes, fields and methods and
//! checks each against the contract its caller states; `invoke` calls a method whose signature was checked first.
//! Everything here is game-neutral: what a game's classes mean belongs to the code that names them.

mod invoke;
pub mod metadata;
pub mod raw;
mod thread;

pub use invoke::{Argument, CheckedMethod, InvocationContext, InvocationOutcome, InvokeError, ReturnValue, Scalar, SignaturePosition, ValueTypeContract};
pub use metadata::{MetadataApi, MetadataError};
pub use thread::{AttachedThread, RuntimeError, StrongRoot, ThreadContext};
