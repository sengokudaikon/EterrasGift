//! The companion tier: finding, watching and starting the game, Home's state, and the mods (the library, MelonLoader's
//! files and the player's own mod projects). It touches only files the game, Steam and MelonLoader own, plus process
//! lookups by name and start time. It never opens or reads the game process; that is the trainer's tier, which this
//! crate never depends on.

#![deny(unsafe_code)]

pub mod game;
pub mod home;
pub mod launch;
pub mod mods;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod shell;
