//! MelonLoader's files, read and edited without running it or any mod: what a DLL declares ([`assembly`]), what the loader
//! recorded and how to repair what it generated ([`loader_state`]), its Latest.log ([`log`]), MelonPreferences.cfg
//! ([`preferences`]), where its files live ([`paths`]) and its proxy in the game folder ([`switch`]). Names no app or game.
//! It has no words to show: a reason is a type for the caller to put in words, and an `io::Error` says what is wrong with
//! the file in plain English.

#![forbid(unsafe_code)]

pub mod assembly;
mod cil;
pub mod loader_state;
pub mod log;
pub mod paths;
pub mod preferences;
pub mod switch;

use std::fmt;
use std::io;

/// A file whose content can't be used, with the sentence that says why.
fn invalid(message: impl fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
