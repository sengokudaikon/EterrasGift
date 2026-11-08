//! The six locations Settings saves with "Save locations", and what the companion checks a tool's folder holds before.

use eg_core::settings::{AppSettings, FolderChecks};
use phrases::{Phrase, phrases};
use std::path::{Path, PathBuf};

phrases! {
    ChooseDotnet: "Choose the folder that holds dotnet.exe, or leave it empty to find the .NET SDK automatically."
    ChooseMelonloader: "Choose a folder with MelonLoader in it (a copy of the game's MelonLoader folder), or leave it empty for the game folder."
}

/// What the .NET SDK and MelonLoader folders must hold to be saved, for the settings writer to check
/// (`SettingsStore::checking`): dotnet.exe, and a MelonLoader folder.
pub const FOLDER_CHECKS: FolderChecks = FolderChecks {
    dotnet: Some(holds_dotnet),
    melon_loader: Some(holds_melon_loader),
};

fn holds_dotnet(folder: &Path) -> Result<(), &'static str> {
    if folder.join("dotnet.exe").is_file() { Ok(()) } else { Err(ChooseDotnet.get()) }
}

fn holds_melon_loader(folder: &Path) -> Result<(), &'static str> {
    if folder.join("MelonLoader").is_dir() { Ok(()) } else { Err(ChooseMelonloader.get()) }
}

/// The six location boxes, in the page's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    GameExecutable,
    UserData,
    ModLibrary,
    ModSources,
    Dotnet,
    MelonLoader,
}

impl Location {
    pub const ALL: [Location; 6] = [
        Location::GameExecutable,
        Location::UserData,
        Location::ModLibrary,
        Location::ModSources,
        Location::Dotnet,
        Location::MelonLoader,
    ];

    /// What the settings file holds for it.
    fn saved(self, settings: &AppSettings) -> &Option<PathBuf> {
        match self {
            Location::GameExecutable => &settings.game_executable,
            Location::UserData => &settings.user_data_directory,
            Location::ModLibrary => &settings.mod_library_directory,
            Location::ModSources => &settings.mod_sources_directory,
            Location::Dotnet => &settings.dotnet_directory,
            Location::MelonLoader => &settings.melon_loader_directory,
        }
    }

    /// What the page says about it: its label, the box's accessible name, its caption, its Browse button's name and the
    /// picker's title.
    pub(super) const fn words(self) -> [Phrase; 5] {
        match self {
            Location::GameExecutable => [
                Phrase::new("Game executable"),
                Phrase::new("Game executable"),
                Phrase::new("Leave empty to find the game automatically."),
                Phrase::new("Browse for the game executable"),
                Phrase::new("Select the Last Epoch executable"),
            ],
            Location::UserData => [
                Phrase::new("User-data folder"),
                Phrase::new("User-data folder"),
                Phrase::new("Fault reports, trainer labels, game definitions and, unless it has its own folder, the mod library. A new folder takes effect after a restart."),
                Phrase::new("Browse for the user-data folder"),
                Phrase::new("Select user-data folder"),
            ],
            Location::ModLibrary => [
                Phrase::new("Mod library"),
                Phrase::new("Mod library folder"),
                Phrase::new("Every mod version you install, and which mods are on. A new folder takes effect after a restart."),
                Phrase::new("Browse for the mod library folder"),
                Phrase::new("Select the mod library folder"),
            ],
            Location::ModSources => [
                Phrase::new("Mod sources folder"),
                Phrase::new("Mod sources folder"),
                Phrase::new("Your own mod projects, for the Mods page to build and install. The .NET SDK reads every project here, so keep only projects you trust."),
                Phrase::new("Browse for the mod sources folder"),
                Phrase::new("Select the mod sources folder"),
            ],
            Location::Dotnet => [
                Phrase::new(".NET SDK folder"),
                Phrase::new(".NET SDK folder"),
                Phrase::new("The folder with dotnet.exe, which builds mod projects. Leave empty to find it automatically."),
                Phrase::new("Browse for the .NET SDK folder"),
                Phrase::new("Select the folder with dotnet.exe"),
            ],
            Location::MelonLoader => [
                Phrase::new("MelonLoader folder"),
                Phrase::new("MelonLoader folder"),
                Phrase::new(
                    "Where games with mods run MelonLoader from. Leave empty for the game folder. Another folder needs a copy of MelonLoader 0.6.6 or later. Takes effect after a restart.",
                ),
                Phrase::new("Browse for the MelonLoader folder"),
                Phrase::new("Select the folder MelonLoader runs from"),
            ],
        }
    }

    /// The picker's title.
    pub fn picker_title(self) -> &'static str {
        self.words()[4].get()
    }
}

pub(super) fn location_text(saved: &AppSettings) -> [String; 6] {
    Location::ALL.map(|location| location.saved(saved).as_ref().map(|path| path.display().to_string()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_tool_folders_must_hold_their_tools() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            holds_dotnet(temp.path()),
            Err("Choose the folder that holds dotnet.exe, or leave it empty to find the .NET SDK automatically.")
        );
        fs::write(temp.path().join("dotnet.exe"), b"").unwrap();
        assert_eq!(holds_dotnet(temp.path()), Ok(()));
        assert!(holds_melon_loader(temp.path()).is_err());
        fs::create_dir(temp.path().join("MelonLoader")).unwrap();
        assert_eq!(holds_melon_loader(temp.path()), Ok(()));
    }
}
