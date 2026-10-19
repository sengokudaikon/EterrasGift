//! Supported-game native bootstrap. The domain/session crate remains free of unsafe code.
//! Construction is inert; explicit preparation enforces offline and process identity checks.

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod windows;

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod desktop;
