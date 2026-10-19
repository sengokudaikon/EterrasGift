//! The trainer's game boundary. Nothing here discovers, opens or injects into a game automatically.
//!
//! The first layer models the pointer-handle IL2CPP ABI and tests ownership against a fake runtime.
//! It does not establish compatibility with any live Unity build. See `research/trainer-rust.md`.

#[cfg(windows)]
pub mod artifact;
pub mod bootstrap;
pub mod catalog;
pub mod definitions;
pub mod forge;
pub mod il2cpp;
pub mod locale;
pub mod mapping;

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod scheduler;

#[cfg(windows)]
pub mod pipe;
pub mod process;
pub mod protocol;
pub mod transport;
pub mod write;
