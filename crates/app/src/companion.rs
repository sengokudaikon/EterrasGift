//! Companion service construction and discovery; neither opens the game process.

use eg_companion::launch::host::FileGameHost;
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::mods::{
    library::ModLibrary,
    sources::{ModSources, SystemProcessRunner},
    worker::ModWorker,
};
use eg_companion::{game::folders, mods::dotnet};
use eg_core::settings::SettingsStore;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub struct Companion {
    pub launcher: Arc<GameLauncher>,
    pub mods: ModWorker,
    pub sources: Arc<ModSources>,
}

impl Companion {
    pub fn new(store: &SettingsStore) -> Self {
        let mods = ModWorker::new(ModLibrary::new(store.paths().mod_library_directory.clone()));
        let sources = Arc::new(ModSources::new(Arc::new(SystemProcessRunner)));
        // Locations are read once: new choices wait for a restart.
        let host = FileGameHost::new(store.active_game_executable(), &folders::player_log(), store.active_melon_loader_directory()).with_mod_worker(mods.clone());
        let launcher = Arc::new(GameLauncher::new(Arc::new(host)));
        Self { launcher, mods, sources }
    }

    /// Look for the game and .NET beside window construction. Join before starting the trainer or running the window:
    /// a proxy left by an interrupted start with mods must be set aside before anything can start the game.
    pub fn discover(&self) -> JoinHandle<(Option<PathBuf>, Option<PathBuf>)> {
        let launcher = self.launcher.clone();
        thread::spawn(move || {
            let install = launcher.find_install();
            launcher.settle_mod_loader(install.as_ref());
            (install.map(|install| install.executable), dotnet::found_directory())
        })
    }
}
