//! Trainer contracts. The initial dispatcher uses owned commands and results with no process access.

#![forbid(unsafe_code)]

pub mod backend;
pub mod catalog;
pub mod catalog_cache;
pub mod connection;
pub mod dispatch;
pub mod edits;

pub mod gate;
pub mod mapping;
pub mod profile;
pub mod session;
pub mod sources;
