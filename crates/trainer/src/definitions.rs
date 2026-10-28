//! The game's definitions: the catalog type (`catalog`), built from the shipped files (`files`), saved between runs
//! (`cache`) and kept by a background service (`service`).

pub mod cache;
pub mod catalog;
pub mod files;
pub mod service;
