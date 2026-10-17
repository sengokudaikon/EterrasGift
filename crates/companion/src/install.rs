//! Finding the game (`spec/companion.md` §1): the executable chosen in Settings, else the Steam library that holds it.

use crate::key_values::KeyValues;
use regex::bytes::Regex;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const STEAM_APP_ID: u32 = 899770;
/// Last Epoch's process name, which its executable is named after.
pub const PROCESS_NAME: &str = "Last Epoch";
pub const EXECUTABLE_NAME: &str = "Last Epoch.exe";

/// Who made the game and what it is called, as Unity records them; MelonLoader matches mods against these. Unity also
/// names the game's folder under LocalLow after them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameIdentity {
    pub developer: String,
    pub product: String,
}

impl GameIdentity {
    pub const LAST_EPOCH_DEVELOPER: &str = "Eleventh Hour Games";
    pub const LAST_EPOCH_PRODUCT: &str = "Last Epoch";
}

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

    /// Unity's data folder beside the executable, named after it: "Last Epoch_Data".
    pub fn data_directory(&self) -> PathBuf {
        let stem = self.executable.file_stem().unwrap_or_default().to_string_lossy();
        self.directory().join(format!("{stem}_Data"))
    }

    /// The compiled game code MelonLoader prepares its assemblies from.
    pub fn game_assembly(&self) -> PathBuf {
        self.directory().join("GameAssembly.dll")
    }

    /// The Unity version the game was built with, without its release type ("6000.4.8" for 6000.4.8f1), as MelonLoader
    /// reads it: from the header of the data folder's globalgamemanagers. None when it can't be read.
    pub fn read_unity_version(&self) -> Option<String> {
        static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)(\d+\.\d+\.\d+)[abfpx]\d+\x00").unwrap());
        let mut header = Vec::new();
        fs::File::open(self.data_directory().join("globalgamemanagers"))
            .ok()?
            .take(256)
            .read_to_end(&mut header)
            .ok()?;
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
        None => from_steam(&find_steam()?),
    }
}

/// Finds the library that lists the game in libraryfolders.vdf, then its folder in the app manifest.
pub fn from_steam(steam_directory: &Path) -> Option<GameInstall> {
    let app = STEAM_APP_ID.to_string();
    let mut libraries: Vec<PathBuf> = Vec::new();
    if let Some(folders) = read_key_values(&steam_directory.join("steamapps").join("libraryfolders.vdf"))
        .as_ref()
        .and_then(|root| root.get("libraryfolders"))
    {
        // Libraries that list the game first; the rest still get a look, since the list can lag behind a move.
        for (_, folder) in &folders.children {
            if let Some(path) = folder.get("path").and_then(|path| path.value.as_deref()) {
                if folder.get("apps").and_then(|apps| apps.get(&app)).is_some() {
                    libraries.insert(0, PathBuf::from(path));
                } else {
                    libraries.push(PathBuf::from(path));
                }
            }
        }
    }
    libraries.push(steam_directory.to_path_buf());
    libraries.into_iter().find_map(|library| {
        let steamapps = library.join("steamapps");
        let manifest = read_key_values(&steamapps.join(format!("appmanifest_{app}.acf")))?;
        let folder = manifest.get("AppState")?.get("installdir")?.value.clone().filter(|folder| !folder.is_empty())?;
        let executable = eg_core::paths::normalized(&steamapps.join("common").join(folder).join(EXECUTABLE_NAME));
        executable.is_file().then(|| GameInstall::new(executable))
    })
}

fn read_key_values(path: &Path) -> Option<KeyValues> {
    KeyValues::parse(&fs::read_to_string(path).ok()?)
}

/// Steam's folder: as the registry has it, else its default place when that exists.
pub fn find_steam() -> Option<PathBuf> {
    if let Some(path) = steam_path_from_registry().filter(|path| !path.is_empty()) {
        return Some(eg_core::paths::normalized(Path::new(&path)));
    }
    let fallback = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Steam");
    fallback.is_dir().then_some(fallback)
}

#[cfg(windows)]
fn steam_path_from_registry() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = "Software\\Valve\\Steam\0".encode_utf16().collect();
    let name: Vec<u16> = "SteamPath\0".encode_utf16().collect();
    let mut size = 0u32;
    // SAFETY: the key and value names are NUL-terminated; the first call only asks for the size.
    if unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } != 0
    {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer holds `size` bytes, as the registry asked for.
    if unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    } != 0
    {
        return None;
    }
    let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}

#[cfg(not(windows))]
fn steam_path_from_registry() -> Option<String> {
    None
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
        assert_eq!(install.read_identity().unwrap().developer, GameIdentity::LAST_EPOCH_DEVELOPER);
    }
}
