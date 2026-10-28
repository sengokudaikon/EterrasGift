//! The game on this machine: its install (found through Steam's files), the folders it uses, its running process and the
//! version it reports.

#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod folders;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod install;
pub mod key_values;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod process;
pub mod version;
