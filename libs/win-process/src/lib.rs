//! The processes running on Windows: a read-only census by ID and executable name, and the file times their identity
//! carries. Opens no process. Names no app or game.

#![deny(unsafe_code)]

#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
mod process;
mod time;

pub use process::{each, is_named};
pub use time::from_filetime;
