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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cancelled when the app closes, so work that waits (a start with mods waiting for MelonLoader) stops and cleans up.
#[derive(Clone, Debug, Default)]
pub struct Lifetime(Arc<AtomicBool>);

impl Lifetime {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
