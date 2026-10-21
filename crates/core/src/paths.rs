//! Where the app keeps its files (`spec/data-and-persistence.md` §1): the checkout or install it belongs to, the user
//! data and what lies in it. `AppPaths` is plain values; the functions below it look at the disk to find them.

use std::path::{Component, Path, PathBuf};

/// The app's own folder under `%LOCALAPPDATA%` holds `settings.json` and the last-resort logs. While the rewrite runs
/// beside the C# app it keeps a folder of its own, so neither can disturb the other's settings; the file format is the
/// same, so sharing the C# app's folder later is this one name ("EterrasGift").
pub const LOCAL_FOLDER: &str = "EterrasGiftRS";

/// The app's executable, as an install holds it beside the launcher (`hosting::LAUNCHER_EXECUTABLE`): the C# app's
/// `EterrasGift.Desktop.exe` beside `EterrasGift.exe`.
pub const EXECUTABLE: &str = "eterras-gift-desktop.exe";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub root: PathBuf,
    /// Running from a source checkout: development mode, until a settings flag replaces this signal.
    pub is_source_checkout: bool,
    /// The user-data folder when none is chosen: data\ beside a checkout, or in the app's local folder for an install.
    pub default_user_data_directory: PathBuf,
    pub user_data_directory: PathBuf,
    /// The mod library: its own folder when one is chosen, else mods\ in the user data.
    pub mod_library_directory: PathBuf,
}

impl AppPaths {
    pub fn new(root: &Path, is_source_checkout: bool, default_user_data: &Path, user_data: Option<&Path>, mod_library: Option<&Path>) -> AppPaths {
        let default_user_data_directory = normalized(default_user_data);
        let user_data_directory = user_data.map_or_else(|| default_user_data_directory.clone(), normalized);
        let mod_library_directory = mod_library.map_or_else(|| user_data_directory.join("mods"), normalized);
        AppPaths {
            root: normalized(root),
            is_source_checkout,
            default_user_data_directory,
            user_data_directory,
            mod_library_directory,
        }
    }

    /// The same app with other folders chosen; `None` is each one's default.
    pub fn with(&self, user_data: Option<&Path>, mod_library: Option<&Path>) -> AppPaths {
        AppPaths::new(&self.root, self.is_source_checkout, &self.default_user_data_directory, user_data, mod_library)
    }

    /// The data that ships with the app (the bundled catalog), beside the checkout or install.
    pub fn catalog_directory(&self) -> PathBuf {
        self.root.join("data").join("catalog")
    }

    pub fn log_directory(&self) -> PathBuf {
        self.user_data_directory.join("logs")
    }

    /// A module's own folder in the user data, by the module's id.
    pub fn module_directory(&self, id: &str) -> PathBuf {
        self.user_data_directory.join(id)
    }

    /// The mod library's folder when none of its own is chosen: mods\ in the user data.
    pub fn default_mod_library_directory(&self) -> PathBuf {
        self.user_data_directory.join("mods")
    }

    /// Where the player's own mod projects are by default: mods\ in a source checkout; none in a release.
    pub fn mod_sources_directory(&self) -> Option<PathBuf> {
        self.is_source_checkout.then(|| self.root.join("mods"))
    }
}

/// A source checkout: the workspace and its toolchain pin beside it.
pub fn is_checkout(root: &Path) -> bool {
    root.join("Cargo.toml").is_file() && root.join("rust-toolchain.toml").is_file()
}

/// The checkout or install a program in `base` belongs to: the nearest checkout above it, else the nearest folder with
/// bundled catalogs (published checkout artifacts hold catalogs too, so source takes precedence), else `base` itself.
pub fn find_root(base: &Path) -> PathBuf {
    let base = normalized(base);
    let mut catalog = None;
    for directory in base.ancestors() {
        if is_checkout(directory) {
            return directory.to_path_buf();
        }
        if catalog.is_none() && directory.join("data").join("catalog").is_dir() {
            catalog = Some(directory.to_path_buf());
        }
    }
    catalog.unwrap_or(base)
}

/// The app's own folder in the user's local application data: settings and the last-resort logs.
pub fn local_directory() -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    local.join(LOCAL_FOLDER)
}

/// The app's folders for `root`: development mode in a source checkout, and user data beside it, or in the app's local
/// folder for an installed app (an install's own folder may not be writable).
pub fn app_paths(root: &Path, user_data: Option<&Path>, mod_library: Option<&Path>) -> AppPaths {
    let root = normalized(root);
    let checkout = is_checkout(&root);
    let default_data = if !checkout && root.join(EXECUTABLE).is_file() {
        local_directory().join("data")
    } else {
        root.join("data")
    };
    AppPaths::new(&root, checkout, &default_data, user_data, mod_library)
}

/// A path as the app stores it: full, with no trailing separator. A path the system can't make full stays as it is.
pub fn normalized(path: &Path) -> PathBuf {
    trim_end_separator(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// Drops a trailing separator, except the one that makes a root (`C:\`).
pub fn trim_end_separator(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.len() == text.len() || matches!(Path::new(trimmed).components().next_back(), None | Some(Component::Prefix(_))) {
        path.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

/// A path that names its drive or share and its root, so it never depends on the folder the app runs in.
pub fn is_fully_qualified(path: &Path) -> bool {
    path.is_absolute()
}

/// Two folders are the same place, however their ends and case are spelled.
pub fn same_folder(first: &Path, second: &Path) -> bool {
    same_text(&normalized(first).to_string_lossy(), &normalized(second).to_string_lossy())
}

/// Windows compares paths without case.
pub fn same_text(first: &str, second: &str) -> bool {
    first == second || first.to_uppercase() == second.to_uppercase()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn stored_paths_are_full_without_a_trailing_separator() {
        assert_eq!(normalized(Path::new(r"C:\Games\Last Epoch\")), PathBuf::from(r"C:\Games\Last Epoch"));
        assert_eq!(normalized(Path::new(r"C:\Games\.\Last Epoch")), PathBuf::from(r"C:\Games\Last Epoch"));
        assert_eq!(normalized(Path::new(r"C:\")), PathBuf::from(r"C:\"));
        assert!(same_folder(Path::new(r"C:\Games\"), Path::new(r"c:\games")));
    }

    #[test]
    fn only_a_path_with_its_drive_and_root_is_fully_qualified() {
        assert!(is_fully_qualified(Path::new(r"C:\Games")));
        assert!(is_fully_qualified(Path::new(r"\\server\share\Games")));
        assert!(!is_fully_qualified(Path::new(r"C:Games")));
        assert!(!is_fully_qualified(Path::new(r"\Games")));
        assert!(!is_fully_qualified(Path::new("Games")));
    }

    #[test]
    fn the_root_is_the_nearest_checkout_else_the_nearest_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        let install = checkout.join("artifacts").join("app");
        std::fs::create_dir_all(install.join("data").join("catalog")).unwrap();
        assert_eq!(find_root(&install), install);
        std::fs::write(checkout.join("Cargo.toml"), "").unwrap();
        std::fs::write(checkout.join("rust-toolchain.toml"), "").unwrap();
        assert_eq!(find_root(&install), checkout);
        let paths = app_paths(&checkout, None, None);
        assert!(paths.is_source_checkout);
        assert_eq!(paths.user_data_directory, checkout.join("data"));
        assert_eq!(paths.mod_library_directory, checkout.join("data").join("mods"));
        assert_eq!(paths.mod_sources_directory(), Some(checkout.join("mods")));
    }
}
