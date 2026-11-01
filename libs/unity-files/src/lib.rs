//! A Unity player's data files, read without running it. Names no game.
//!
//! Layers, bottom up: `bundle` (UnityFS archives and their LZ4 blocks), `serialized` (Unity serialized files),
//! `typetree` (layouts and the decoder, which must consume every byte of an object) and `archive` (bundles read
//! together, so references between them resolve). `strings` reads Unity Localization's string tables.
//!
//! Classes stored without type trees (`resources.assets`) are read by `player` with layouts rebuilt from the IL2CPP build
//! (`il2cpp`, `layout`): the metadata names the fields, the game assembly types them, and Unity's serialization
//! rules lay them out. Every object must decode exactly.

#![forbid(unsafe_code)]

pub mod archive;
pub mod bundle;
mod common_strings;
pub mod il2cpp;
pub mod layout;
pub mod player;
pub mod serialized;
pub mod strings;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod typetree;
pub mod value;

pub use binary_reader::{Error, Result};
