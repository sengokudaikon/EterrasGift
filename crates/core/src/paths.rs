//! Where the app keeps its files: the checkout or install it belongs to, the user data and what lies in it. `AppPaths`
//! is plain values; the functions below it look at the disk to find them.

use file_ops::paths::normalized;
use std::path::{Path, PathBuf};

/// The app's own folder under `%LOCALAPPDATA%` holds `settings.json` and the last-resort logs. While the rewrite runs
/// beside the C# app it keeps a folder of its own, so neither can disturb the other's settings; the file format is the
/// same, so sharing the C# app's folder later is this one name ("EterrasGift").
pub const LOCAL_FOLDER: &str = "EterrasGiftRS";

/// The app's executable. Its presence in a folder that is not a checkout marks an install.
pub const EXECUTABLE: &str = "eterras-gift.exe";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub root: PathBuf,
    /// Running from a source checkout: development mode.
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
        let mod_library_directory = mod_library.map_or_else(|| default_mod_library(&user_data_directory), normalized);
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

    pub fn log_directory(&self) -> PathBuf {
        self.user_data_directory.join("logs")
    }

    /// A module's own folder in the user data, by the module's id.
    pub fn module_directory(&self, id: &str) -> PathBuf {
        self.user_data_directory.join(id)
    }

    /// The mod library's folder when none of its own is chosen: mods\ in the user data.
    pub fn default_mod_library_directory(&self) -> PathBuf {
        default_mod_library(&self.user_data_directory)
    }

    /// Where the player's own mod projects are by default: mods\ in a source checkout; none in a release.
    pub fn mod_sources_directory(&self) -> Option<PathBuf> {
        self.is_source_checkout.then(|| self.root.join("mods"))
    }
}

fn default_mod_library(user_data: &Path) -> PathBuf {
    user_data.join("mods")
}

/// A source checkout: the workspace and its toolchain pin beside it.
fn is_checkout(root: &Path) -> bool {
    root.join("Cargo.toml").is_file() && root.join("rust-toolchain.toml").is_file()
}

/// The checkout or install a program in `base` belongs to: the nearest checkout above it, else `base` itself.
pub fn find_root(base: &Path) -> PathBuf {
    let base = normalized(base);
    match base.ancestors().find(|directory| is_checkout(directory)) {
        Some(checkout) => checkout.to_path_buf(),
        None => base,
    }
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_the_nearest_checkout_else_the_programs_own_folder() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        let install = checkout.join("artifacts").join("app");
        std::fs::create_dir_all(&install).unwrap();
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
