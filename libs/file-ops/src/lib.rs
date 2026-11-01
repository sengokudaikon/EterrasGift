//! Files handled with care on Windows: writes that replace in one step, folder moves that resume, leases that hold a
//! file against change, one-at-a-time locks, SHA-256 in one spelling, and text and JSON that people and other programs
//! write. Names no app or game.

#![deny(unsafe_code)]

#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod atomic;
pub mod folder_move;
pub mod hashes;
pub mod instance;
pub mod json;
#[cfg(windows)]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod lease;
pub mod paths;
pub mod text;
