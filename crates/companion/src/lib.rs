//! The companion tier: finding, watching and starting the game, Home's state, the mods (the library, MelonLoader's files
//! and the player's own mod projects), and the other pages every run has: Settings (`settings`) and the log viewer
//! (`logs`). Beside the app's own files, it touches only files the game, Steam and MelonLoader own, plus process lookups by
//! name and start time. It opens the game's process only to ask when it started, with the limited query right, through
//! `win_platform`. It never reads the process or writes to it; that is the trainer's tier, which this crate never depends
//! on.

#![forbid(unsafe_code)]

pub mod game;
pub mod home;
pub mod launch;
pub mod logs;
pub mod mods;
pub mod settings;
pub use win_platform::shell;
