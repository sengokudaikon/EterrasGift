//! Where Steam keeps an app, from Steam's own files: the registry names its folder, `libraryfolders.vdf` its libraries and
//! each `appmanifest_*.acf` an app's folder.

mod key_values;

use key_values::KeyValues;
use std::fs;
use std::path::{Path, PathBuf};

/// The folder of the app `app_id`, in the library that lists it in libraryfolders.vdf, by its app manifest. Only a folder
/// that holds `executable_name` counts. The path is built from what Steam's files say, unnormalized.
pub fn install_folder(steam_directory: &Path, app_id: u32, executable_name: &str) -> Option<PathBuf> {
    let app = app_id.to_string();
    let mut libraries: Vec<PathBuf> = Vec::new();
    if let Some(folders) = read_key_values(&steam_directory.join("steamapps").join("libraryfolders.vdf"))
        .as_ref()
        .and_then(|root| root.get("libraryfolders"))
    {
        // Libraries that list the app first; the rest still get a look, since the list can lag behind a move.
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
        let folder = steamapps.join("common").join(folder);
        folder.join(executable_name).is_file().then_some(folder)
    })
}

fn read_key_values(path: &Path) -> Option<KeyValues> {
    KeyValues::parse(&fs::read_to_string(path).ok()?)
}

/// Steam's folder: as the registry spells it, else its default place when that exists.
pub fn folder() -> Option<PathBuf> {
    if let Some(path) = path_from_registry().filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let fallback = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Steam");
    fallback.is_dir().then_some(fallback)
}

#[cfg(windows)]
fn path_from_registry() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let key = crate::wide("Software\\Valve\\Steam");
    let name = crate::wide("SteamPath");
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
fn path_from_registry() -> Option<String> {
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
    fn the_library_that_lists_the_app_is_looked_in_first_and_the_executable_decides() {
        let temp = tempfile::tempdir().unwrap();
        let steam = temp.path().join("Steam");
        let (other, listing) = (temp.path().join("Other"), temp.path().join("Listing"));
        let escaped = |path: &Path| path.to_string_lossy().replace('\\', "\\\\");
        write(
            &steam.join("steamapps").join("libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" \"apps\" {{ }} }} \"1\" {{ \"path\" \"{}\" \"apps\" {{ \"123\" \"1\" }} }} }}",
                escaped(&other),
                escaped(&listing)
            ),
        );
        for library in [&other, &listing] {
            write(&library.join("steamapps").join("appmanifest_123.acf"), "\"AppState\" { \"installdir\" \"Some App\" }");
        }
        let folder = |library: &Path| library.join("steamapps").join("common").join("Some App");
        assert_eq!(install_folder(&steam, 123, "app.exe"), None);
        write(&folder(&other).join("app.exe"), "");
        assert_eq!(install_folder(&steam, 123, "app.exe"), Some(folder(&other)), "the rest still get a look");
        write(&folder(&listing).join("app.exe"), "");
        assert_eq!(install_folder(&steam, 123, "app.exe"), Some(folder(&listing)));
        assert_eq!(install_folder(&steam, 456, "app.exe"), None);
    }
}
