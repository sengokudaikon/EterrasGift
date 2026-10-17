//! Narrow IL2CPP boundary. Raw declarations are separate from thread and root ownership.

pub mod raw;
mod thread;

pub use thread::{AttachedThread, RuntimeError, StrongRoot, ThreadContext};
