//! The trainer's game boundary. Nothing here discovers, opens or injects into a game automatically.
//!
//! Host side: process identity, artifact leases, the helper bootstrap and the pipe. In the game, behind
//! `il2cpp`: IL2CPP attachment and roots, metadata discovery, checked invocation and entry hooks.

#[cfg(windows)]
pub mod artifact;
pub mod bootstrap;
#[cfg(feature = "il2cpp")]
pub mod il2cpp;
pub mod numeric_locale;
pub mod pe;

#[cfg(all(feature = "il2cpp", windows, target_arch = "x86_64"))]
pub mod hooks;

#[cfg(windows)]
pub mod pipe;
pub mod process;
