//! Last Epoch's game data, read from the files the game ships: no process, no injection, no game running. See
//! `research/game-files.md` for where each part lives.
//!
//! Layers, bottom up: [`bundle`] (UnityFS archives and their LZ4 blocks), [`serialized`] (Unity serialized files),
//! [`typetree`] (layouts and the decoder, which must consume every byte of an object), then the game's own data:
//! [`Install::definitions`] (affixes and item bases) and [`Install::strings`] (the text of one locale).
//!
//! Classes stored without type trees (uniques, properties and set bonuses in `resources.assets`) are read by
//! [`Install::player_definitions`] with layouts rebuilt from the IL2CPP build (`il2cpp`, `layout`): the metadata
//! (format 39) names the fields, `GameAssembly.dll` types them, and Unity's serialization rules lay them out. The
//! rules are checked against every class the bundles ship a tree for, and every object must decode exactly.

#![forbid(unsafe_code)]

mod archive;
pub mod bundle;
mod common_strings;
pub mod definitions;
pub mod error;
mod il2cpp;
mod install;
mod layout;
mod player;
mod reader;
pub mod serialized;
pub mod strings;
#[cfg(test)]
mod testing;
pub mod typetree;
pub mod uniques;
pub mod value;

pub use archive::Script;
pub use definitions::Definitions;
pub use error::{Error, Result};
pub use install::{DATA_FOLDER, Install};

pub use strings::Strings;
pub use uniques::PlayerDefinitions;
