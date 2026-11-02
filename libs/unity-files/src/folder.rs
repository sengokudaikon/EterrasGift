//! Where a Unity player keeps its files, as paths only.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The standard layout of a built Unity IL2CPP player: `<name>.exe` and `GameAssembly.dll` in a folder, the data in
/// `<name>_Data` beside them. Nothing here looks at the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerFolder {
    folder: PathBuf,
    data: PathBuf,
}

impl PlayerFolder {
    /// The player called `name` (its executable's name, without `.exe`) in `folder`. Unity finds its data by that name.
    pub fn new(folder: impl Into<PathBuf>, name: impl AsRef<OsStr>) -> Self {
        let folder = folder.into();
        let mut data_name = name.as_ref().to_os_string();
        data_name.push("_Data");
        let data = folder.join(data_name);
        Self { folder, data }
    }

    /// The player that `executable` starts.
    pub fn of(executable: &Path) -> Self {
        Self::new(executable.parent().unwrap_or(Path::new("")), executable.file_stem().unwrap_or_default())
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// `<name>_Data`.
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// `GameAssembly.dll`, the IL2CPP build's native code and the type records its fields point at.
    pub fn game_assembly(&self) -> PathBuf {
        self.folder.join("GameAssembly.dll")
    }

    /// `global-metadata.dat`, the IL2CPP build's names, type and field definitions.
    pub fn metadata(&self) -> PathBuf {
        self.data.join("il2cpp_data").join("Metadata").join("global-metadata.dat")
    }

    /// The player data Unity stores without type trees.
    pub fn resources(&self) -> PathBuf {
        self.data.join("resources.assets")
    }

    /// The player's managers, which hold the `MonoScript`s that `resources.assets`' behaviours run.
    pub fn managers(&self) -> PathBuf {
        self.data.join("globalgamemanagers.assets")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_follows_the_players_name() {
        let player = PlayerFolder::new("Games/Tower", "Tower Run");
        let games = Path::new("Games/Tower");
        assert_eq!(player.folder(), games);
        assert_eq!(player.data(), games.join("Tower Run_Data"));
        assert_eq!(player.game_assembly(), games.join("GameAssembly.dll"));
        let data = games.join("Tower Run_Data");
        assert_eq!(player.metadata(), data.join("il2cpp_data").join("Metadata").join("global-metadata.dat"));
        assert_eq!(player.resources(), data.join("resources.assets"));
        assert_eq!(player.managers(), data.join("globalgamemanagers.assets"));
        assert_eq!(PlayerFolder::of(&games.join("Tower Run.exe")), player);
    }
}
