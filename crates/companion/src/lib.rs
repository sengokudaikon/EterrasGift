//! The companion tier (`spec/companion.md`): Home's starts and, later, the mod library. It touches only files the game,
//! Steam and MelonLoader own, plus process lookups by name and start time. It never opens or reads the game process;
//! that is the trainer's tier, which this crate never depends on.

#![deny(unsafe_code)]

pub mod dotnet;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod folders;
pub mod game_version;
pub mod home;
pub mod host;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod install;
pub mod key_values;
pub mod launcher;
pub mod loader;
pub mod mods;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod process;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod shell;

/// Cancels waiting companion work when the app closes.
pub use eg_core::operations::Cancellation as Lifetime;
