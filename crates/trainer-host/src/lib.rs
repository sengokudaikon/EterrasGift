//! Supported-game native bootstrap. The domain/session crate remains free of unsafe code.
//! Construction is inert; explicit preparation enforces offline and process identity checks.

#![deny(unsafe_code)]

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod windows;

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod desktop;
