//! Shared owned messages and bounded codecs. No process, loader or IL2CPP access.
#![forbid(unsafe_code)]

pub mod catalog;
pub mod definitions;
pub mod dispatch;
pub mod fields;
pub mod forge;
pub mod locale;
pub mod mapping;
pub mod process;
pub mod protocol;
pub mod rate_write;
pub mod transport;
pub mod write;
