//! The trainer's game boundary. Nothing here discovers, opens or injects into a game automatically.
//!
//! The first layer models the pointer-handle IL2CPP ABI and tests ownership against a fake runtime.
//! It does not establish compatibility with any live Unity build. See `research/trainer-interop.md`.

#[cfg(windows)]
pub mod artifact;
pub mod bootstrap;
#[cfg(feature = "il2cpp")]
pub mod il2cpp;
pub mod numeric_locale;
pub mod pe;

#[cfg(all(feature = "il2cpp", windows, target_arch = "x86_64"))]
pub mod hooks;

#[cfg(windows)]
pub mod pipe;
pub mod process;
