//! No DllMain bootstrap. Start explicitly after loader initialization; exposed code is retained.
include!(concat!(env!("OUT_DIR"), "/identity.rs"));

#[cfg(all(windows, target_arch = "x86_64"))]
mod engine;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod server;

#[cfg(all(windows, target_arch = "x86_64"))]
pub use engine::{eg_helper_bootstrap, eg_helper_build_id, eg_helper_metrics, eg_helper_shutdown, eg_helper_start};
