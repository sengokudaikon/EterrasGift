//! Last Epoch's game data, read from the files the game ships with `unity-files`: no process, no injection, no game
//! running. [`Install::definitions`] reads affixes, item bases, ailments and the idol altar registry, [`Install::strings`]
//! the text of one locale, and [`Install::player_definitions`] the uniques, properties and set bonuses stored without
//! type trees in `resources.assets`.

#![forbid(unsafe_code)]

pub mod definitions;
mod install;
#[cfg(test)]
mod layout_check;
pub mod uniques;

pub use binary_reader::{Error, Result};
pub use definitions::Definitions;
pub use install::Install;
pub use uniques::PlayerDefinitions;
pub use unity_files::strings::Strings;
