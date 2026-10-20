//! The trainer's game boundary. Nothing here discovers, opens or injects into a game automatically.
//!
//! The first layer models the pointer-handle IL2CPP ABI and tests ownership against a fake runtime.
//! It does not establish compatibility with any live Unity build. See `research/trainer-rust.md`.

#[cfg(windows)]
pub mod artifact;
pub mod bootstrap;
pub use eg_trainer_wire::catalog;
pub use eg_trainer_wire::definitions;
pub use eg_trainer_wire::fields;
pub use eg_trainer_wire::forge;
#[cfg(feature = "il2cpp")]
pub mod il2cpp;
pub use eg_trainer_wire::locale;
pub use eg_trainer_wire::mapping;
pub mod numeric_locale;

#[cfg(all(feature = "il2cpp", windows, target_arch = "x86_64"))]
pub mod scheduler;

#[cfg(windows)]
pub mod pipe;
pub mod process;
pub use eg_trainer_wire::protocol;
pub use eg_trainer_wire::rate_write;
pub use eg_trainer_wire::transport;
pub use eg_trainer_wire::write;
