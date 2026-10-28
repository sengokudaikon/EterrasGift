//! Supported-game native bootstrap. Unsafe code is confined to the modules that call Windows directly.
//! Construction is inert; explicit preparation enforces offline and process identity checks.

#![deny(unsafe_code)]

mod lifecycle;

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod windows;

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod desktop;
