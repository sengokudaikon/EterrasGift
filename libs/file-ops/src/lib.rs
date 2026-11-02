//! Files handled with care on Windows: writes that replace in one step, folder moves that resume, leases that hold a
//! file against change, one-at-a-time locks, SHA-256 in one spelling, stamps that tell whether a file moved, reads that
//! are bounded or find nothing, text and JSON that people and other programs write, and names compared as Windows
//! compares them. Names no app or game.

#![deny(unsafe_code)]

#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod atomic;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod case;
pub mod folder_move;
pub mod hashes;
pub mod instance;
pub mod json;
#[cfg(windows)]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod lease;
pub mod paths;
pub mod read;
pub mod stamp;
pub mod text;
