//! No DllMain bootstrap. Start explicitly after loader initialization; exposed code is retained.
#![deny(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/identity.rs"));

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
mod engine;
#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod server;

#[cfg(all(windows, target_arch = "x86_64"))]
pub use engine::{eg_helper_bootstrap, eg_helper_build_id};

#[cfg(all(windows, target_arch = "x86_64"))]
mod lifecycle;

/// One diagnostic line on the game's stderr, for native harnesses. Never panics: `eprintln!` panics when the write
/// fails, and on a thread without a surrounding catch that aborts the game.
#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn report(line: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}
