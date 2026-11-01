//! The .NET SDK that builds a mod's project.

use std::path::{Path, PathBuf};

const FILE_NAME: &str = "dotnet.exe";

/// The dotnet.exe a build uses when no folder is chosen: the first in DOTNET_ROOT, `%USERPROFILE%\.dotnet`, then each
/// PATH entry; None when there is none.
pub fn found() -> Option<PathBuf> {
    let mut folders: Vec<PathBuf> = Vec::new();
    folders.extend(std::env::var_os("DOTNET_ROOT").map(PathBuf::from));
    folders.extend(std::env::var_os("USERPROFILE").map(|profile| PathBuf::from(profile).join(".dotnet")));
    if let Some(path) = std::env::var_os("PATH") {
        folders.extend(std::env::split_paths(&path));
    }
    folders.into_iter().find_map(|folder| {
        let trimmed = PathBuf::from(folder.to_string_lossy().trim());
        let candidate = trimmed.join(FILE_NAME);
        (trimmed.is_absolute() && candidate.is_file()).then_some(candidate)
    })
}

/// The folder of the dotnet.exe found.
pub fn found_directory() -> Option<PathBuf> {
    found().and_then(|executable| executable.parent().map(Path::to_path_buf))
}

/// The dotnet.exe in the folder chosen; without one, the one found; failing that, whatever the system runs by that name.
pub fn executable(sdk: Option<&Path>) -> PathBuf {
    sdk.map(|folder| folder.join(FILE_NAME)).or_else(found).unwrap_or_else(|| "dotnet".into())
}
