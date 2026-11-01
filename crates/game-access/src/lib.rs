//! The desktop's native boundary to the game process: process identity, the helper bootstrap and the pipe.
//! Nothing here discovers, opens or injects into a game automatically. The code that runs in
//! the game is the helper's (`eg-trainer-helper`).

pub mod bootstrap;

#[cfg(windows)]
pub mod pipe;
pub mod process;
#[cfg(windows)]
pub mod version;
