//! Shared owned messages and bounded codecs. No process, loader or IL2CPP access.
#![forbid(unsafe_code)]

pub mod catalog;
pub mod definitions;
pub mod fields;
pub mod forge;
pub mod hooks;
pub mod locale;
pub mod mapping;
pub mod process;
pub mod protocol;
pub mod rules;
pub mod transport;
pub mod write;
