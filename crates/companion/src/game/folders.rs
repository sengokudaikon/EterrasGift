//! Where Last Epoch keeps the files it owns: Unity's persistent data folder under LocalLow.

use crate::game::install::{LAST_EPOCH_DEVELOPER, LAST_EPOCH_PRODUCT};
use std::path::PathBuf;

/// The game's own folder: `<LocalLow>\Eleventh Hour Games\Last Epoch`.
pub fn game_user_data() -> PathBuf {
    win_platform::folders::local_low().join(LAST_EPOCH_DEVELOPER).join(LAST_EPOCH_PRODUCT)
}

/// The running game's log. The previous run's log is rotated to Player-prev.log at launch.
pub fn player_log() -> PathBuf {
    game_user_data().join("Player.log")
}
