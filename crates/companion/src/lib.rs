//! The companion tier: finding, watching and starting the game, Home's state, and the mods (the library, MelonLoader's
//! files and the player's own mod projects). It touches only files the game, Steam and MelonLoader own, plus process
//! lookups by name and start time. It opens the game's process only to ask when it started, with the limited query
//! right, through `win_platform`. It never reads the process or writes to it; that is the trainer's tier, which this crate
//! never depends on.

#![forbid(unsafe_code)]

pub mod game;
pub mod home;
pub mod launch;
pub mod mods;
pub use win_platform::shell;
