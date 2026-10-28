//! The desktop's native boundary to the game process: process identity, artifact leases, PE layouts, the helper
//! bootstrap and the pipe. Nothing here discovers, opens or injects into a game automatically. The code that runs in
//! the game is the helper's (`eg-trainer-helper`).

#[cfg(windows)]
pub mod artifact;
pub mod bootstrap;
pub mod numeric_locale;
pub mod pe;

#[cfg(windows)]
pub mod pipe;
pub mod process;
#[cfg(windows)]
pub mod version;
