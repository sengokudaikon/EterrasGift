//! Where MelonLoader keeps its files under the folder it runs from (the game folder, unless it is told another).

use std::path::{Path, PathBuf};

/// MelonLoader's own folder; its being there is what makes a game folder one MelonLoader runs in.
pub fn loader(root: &Path) -> PathBuf {
    root.join("MelonLoader")
}

/// The game's Mods folder, which MelonLoader loads mods from.
pub fn mods(root: &Path) -> PathBuf {
    root.join("Mods")
}

pub fn user_data(root: &Path) -> PathBuf {
    root.join("UserData")
}

/// The mods' settings, which MelonLoader saves when the game quits.
pub fn preferences(root: &Path) -> PathBuf {
    user_data(root).join("MelonPreferences.cfg")
}

/// The log MelonLoader starts writing as soon as it runs.
pub fn latest_log(root: &Path) -> PathBuf {
    loader(root).join("Latest.log")
}
