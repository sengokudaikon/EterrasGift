//! The trainer on the desktop. `definitions` holds the game's definitions, read from its shipped files; `item` plans,
//! checks and undoes item edits and samples roll values; `features` is the feature registry and the cheat profiles;
//! `connect` admits a game and prepares the helper; `session` talks to the helper; `backend` is the serialized worker
//! the UI commands; `write` is a forge write's plan and outcome; `screens` is what the trainer's pages show, without a
//! window.

#![forbid(unsafe_code)]

pub mod backend;
pub mod connect;
pub mod definitions;
pub mod features;
pub mod item;
pub mod screens;
pub mod session;
#[cfg(feature = "testing")]
pub mod testing;
pub mod write;
