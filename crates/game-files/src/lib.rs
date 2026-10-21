//! Last Epoch's game data, read from the files the game ships: no process, no injection, no game running. See
//! `research/game-files.md` for where each part lives.
//!
//! Layers, bottom up: [`bundle`] (UnityFS archives and their LZ4 blocks), [`serialized`] (Unity serialized files),
//! [`typetree`] (layouts and the decoder, which must consume every byte of an object), then the game's own data:
//! [`Install::definitions`] (affixes and item bases) and [`Install::strings`] (the text of one locale).
//!
//! Only data that ships with its type trees is read here. Classes stored without them (uniques, properties, set bonuses
//! in `resources.assets`) need their layout from the IL2CPP build and are not read yet.

#![forbid(unsafe_code)]

mod archive;
pub mod bundle;
mod common_strings;
pub mod definitions;
pub mod error;
mod install;
mod reader;
pub mod serialized;
pub mod strings;
#[cfg(test)]
mod testing;
pub mod typetree;
pub mod value;

pub use archive::Script;
pub use definitions::Definitions;
pub use error::{Error, Result};
pub use install::{DATA_FOLDER, Install};
pub use strings::Strings;
