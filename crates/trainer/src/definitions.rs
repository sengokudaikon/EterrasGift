//! The game's definitions: the catalog type (`catalog`) and its entries (`entries`), built from the shipped files
//! (`files`), saved between runs (`cache`) and kept by a background service (`service`).

pub mod cache;
pub mod catalog;
pub mod entries;
pub mod files;
pub mod service;
pub mod stat_text;
pub mod unique_mods;
