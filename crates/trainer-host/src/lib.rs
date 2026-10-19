//! Native trainer composition. The domain/session crate remains free of unsafe code.
//! Constructing this layer performs no process access. A runtime authority must supply
//! an explicit per-process unsafe attestation before the first load; no generic game
//! admission policy, automatic injection, or live-game approval is implied.

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod windows;
