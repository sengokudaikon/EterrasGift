//! Admitting a game and preparing the helper: the Player.log gate (`gate`), offline admission around the helper's
//! preparation (`admission`), the build's identity and artifact leases (`build`) and the composition's inputs
//! (`sources`), and on Windows the
//! desktop's own preparation of the helper it embeds (`host`).

pub mod admission;
pub mod build;
pub mod gate;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod host;
pub mod sources;
