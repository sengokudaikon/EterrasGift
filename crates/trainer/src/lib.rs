//! Trainer contracts. The initial dispatcher uses owned commands and results with no process access.

#![forbid(unsafe_code)]

pub mod backend;
pub mod catalog;
pub mod catalog_cache;
pub mod connection;
pub mod dispatch;

pub mod gate;
pub mod profile;
pub mod session;
