//! Finding the game: the executable chosen in Settings, else the Steam library that holds it.

use melon_loader::assembly::GameIdentity;
use regex::bytes::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use unity_files::folder::PlayerFolder;

pub const STEAM_APP_ID: u32 = 899770;
/// Last Epoch's process name, which its executable is named after.
pub const PROCESS_NAME: &str = "Last Epoch";
pub const EXECUTABLE_NAME: &str = "Last Epoch.exe";

/// Who made the game and what it is called, as Unity records them. Unity also names the game's folder under LocalLow after
/// them.
pub const LAST_EPOCH_DEVELOPER: &str = "Eleventh Hour Games";
pub const LAST_EPOCH_PRODUCT: &str = "Last Epoch";

/// Where Last Epoch is installed: its executable, and the folder it runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameInstall {
    pub executable: PathBuf,
}

impl GameInstall {
    pub fn new(executable: impl Into<PathBuf>) -> GameInstall {
        GameInstall { executable: executable.into() }
    }

    pub fn directory(&self) -> &Path {
        self.executable.parent().unwrap_or(Path::new(""))
    }

    fn player(&self) -> PlayerFolder {
        PlayerFolder::of(&self.executable)
    }

    /// Unity's data folder beside the executable, named after it: "Last Epoch_Data".
    pub fn data_directory(&self) -> PathBuf {
        self.player().data().to_path_buf()
    }

    /// The compiled game code MelonLoader prepares its assemblies from.
    pub fn game_assembly(&self) -> PathBuf {
        self.player().game_assembly()
    }

    /// The Unity version the game was built with, without its release type ("6000.4.8" for 6000.4.8f1), as MelonLoader
    /// reads it: from the header of the data folder's globalgamemanagers. None when it can't be read.
    pub fn read_unity_version(&self) -> Option<String> {
        static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)(\d+\.\d+\.\d+)[abfpx]\d+\x00").unwrap());
        let header = file_ops::read::head(&self.data_directory().join("globalgamemanagers"), 256).ok()?;
        let found = VERSION.captures(&header)?;
        Some(String::from_utf8_lossy(&found[1]).into_owned())
    }

    /// The developer and product name from Unity's app.info (two lines); None when it is missing or unreadable.
    pub fn read_identity(&self) -> Option<GameIdentity> {
        let text = fs::read_to_string(self.data_directory().join("app.info")).ok()?;
        let mut lines = text.lines();
        let (developer, product) = (lines.next()?, lines.next()?);
        (!developer.is_empty() && !product.is_empty()).then(|| GameIdentity {
            developer: developer.to_owned(),
            product: product.to_owned(),
        })
    }
}

/// The executable chosen in Settings while it is there, else the Steam library that holds the game (a chosen one that
/// moved or was uninstalled is no reason to stop looking). Reads Steam's own files only.
pub fn find(configured: Option<&Path>, steam_directory: Option<&Path>) -> Option<GameInstall> {
    if let Some(configured) = configured
        && configured.is_file()
    {
        return Some(GameInstall::new(configured));
    }
    match steam_directory {
        Some(steam) => from_steam(steam),
        None => from_steam(&file_ops::paths::normalized(&win_platform::steam::folder()?)),
    }
}

/// Finds the library that lists the game in libraryfolders.vdf, then its folder in the app manifest.
pub fn from_steam(steam_directory: &Path) -> Option<GameInstall> {
    let folder = win_platform::steam::install_folder(steam_directory, STEAM_APP_ID, EXECUTABLE_NAME)?;
    Some(GameInstall::new(file_ops::paths::normalized(&folder.join(EXECUTABLE_NAME))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn steam_finds_the_game_in_the_library_that_lists_it() {
        let temp = tempfile::tempdir().unwrap();
        let steam = temp.path().join("Steam");
        let other = temp.path().join("Library");
        let escaped = |path: &Path| path.to_string_lossy().replace('\\', "\\\\");
        write(
            &steam.join("steamapps").join("libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" \"apps\" {{ }} }} \"1\" {{ \"path\" \"{}\" \"apps\" {{ \"899770\" \"1\" }} }} }}",
                escaped(&steam),
                escaped(&other)
            ),
        );
        write(&other.join("steamapps").join("appmanifest_899770.acf"), "\"AppState\" { \"installdir\" \"Last Epoch\" }");
        let executable = other.join("steamapps").join("common").join("Last Epoch").join(EXECUTABLE_NAME);
        assert_eq!(from_steam(&steam), None);
        write(&executable, "");
        assert_eq!(from_steam(&steam), Some(GameInstall::new(&executable)));
        // A chosen executable that is gone is no reason to stop looking.
        assert_eq!(find(Some(&temp.path().join("gone.exe")), Some(&steam)), Some(GameInstall::new(&executable)));
    }

    #[test]
    fn the_unity_version_and_maker_come_from_the_data_folder() {
        let temp = tempfile::tempdir().unwrap();
        let install = GameInstall::new(temp.path().join(EXECUTABLE_NAME));
        assert_eq!(install.data_directory(), temp.path().join("Last Epoch_Data"));
        assert_eq!(install.read_unity_version(), None);
        fs::create_dir_all(install.data_directory()).unwrap();
        fs::write(install.data_directory().join("globalgamemanagers"), b"\x00\x00\x00\x166000.4.8f1\x00\x00junk").unwrap();
        assert_eq!(install.read_unity_version().as_deref(), Some("6000.4.8"));
        fs::write(install.data_directory().join("app.info"), "Eleventh Hour Games\nLast Epoch").unwrap();
        assert_eq!(install.read_identity().unwrap().developer, LAST_EPOCH_DEVELOPER);
    }
}
