//! Finds, watches and starts Last Epoch without touching the running process (`spec/companion.md` §2 and §4). The
//! `GameHost` trait is what Home and the Mods page use; tests pass a fake so no test ever starts the real game.

use crate::Lifetime;
use crate::install::{self, GameInstall};
use crate::loader::{self, ModLoaderState, ModLoaderSwitch};
use crate::mods::assembly::LoaderVersion;
use crate::mods::library::preferences_path;
use crate::mods::loader_state::{self, LoaderInstall};
use crate::mods::worker::ModWorker;
use crate::process::{self, RunningGame};
use eg_core::game_log::{GameLogReader, GameLogState};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    /// Through Steam, so the player's Steam launch options apply. MelonLoader is set aside first.
    Online,
    /// The executable with --offline: offline characters only. MelonLoader is set aside first.
    Offline,
    /// The executable with --offline and MelonLoader's proxy back in place, so the mods that are on load.
    OfflineWithMods,
}

/// How a start went once it was under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// Started; no mods were asked for.
    Started,
    /// Started with mods: MelonLoader wrote its log, and its proxy is set aside again.
    LoaderStarted,
    /// Started with mods, but MelonLoader wrote no log in time; its proxy is set aside again all the same.
    LoaderNotSeen,
}

/// Why a start or its cleanup failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The start was refused or could not run; the sentence says why.
    Refused(String),
    /// Something on disk went wrong; the text is the system's reason.
    Failed(String),
    /// The app is closing.
    Cancelled,
}

/// Finds, watches and starts Last Epoch without touching the running process.
pub trait GameHost: Send + Sync {
    fn find_install(&self) -> Option<GameInstall>;
    /// The oldest running game, or None. A second instance exits by itself, so the oldest is the one that stays.
    fn find_running(&self) -> Option<RunningGame>;
    /// What the game's own Player.log says about this run.
    fn read_log(&self, game: &RunningGame) -> GameLogState;
    /// Whether MelonLoader is installed where it runs from, and whether the next start would load it.
    fn read_mod_loader(&self, install: Option<&GameInstall>) -> ModLoaderState;
    /// Where a game with mods runs MelonLoader from: the game folder, unless another is chosen in Settings.
    fn loader_root(&self, install: &GameInstall) -> PathBuf;
    /// The installed game's version ("1.5.2"), as its last run wrote it.
    fn read_game_version(&self, install: &GameInstall) -> Option<String>;
    /// Starts the game. Without mods, MelonLoader's proxy is set aside first so nothing of it loads, online or offline.
    /// With mods, the proxy goes back in place for this start and is set aside again once MelonLoader has started.
    /// Blocks until then, so call it off the UI thread.
    fn launch(&self, mode: LaunchMode, install: Option<&GameInstall>, lifetime: &Lifetime) -> Result<LaunchOutcome, LaunchError>;
    /// Sets MelonLoader's proxy aside when it was left in place and no game is running (a start with mods the app did
    /// not see through), so a start from Steam stays without mods. True when it set it aside.
    fn settle_mod_loader(&self, install: Option<&GameInstall>) -> bool;
}

/// The real host: processes are found by name and start time, the mode comes from Player.log, and starts go through
/// Steam or the executable.
pub struct FileGameHost {
    configured_executable: Option<PathBuf>,
    player_log: PathBuf,
    log: Mutex<GameLogReader>,
    loader_directory: Option<PathBuf>,
    mods: Option<ModWorker>,
}

impl FileGameHost {
    /// `loader_directory` is where a game with mods runs MelonLoader from when not the game folder (chosen in Settings):
    /// the mods are placed there and the game is told with --melonloader.basedir. The proxy stays in the game folder.
    pub fn new(configured_executable: Option<&Path>, player_log: &Path, loader_directory: Option<&Path>) -> FileGameHost {
        FileGameHost {
            configured_executable: configured_executable.map(Path::to_path_buf),
            player_log: player_log.to_path_buf(),
            log: Mutex::new(GameLogReader::new(player_log)),
            loader_directory: loader_directory.map(Path::to_path_buf),
            mods: None,
        }
    }

    /// Shares the Mods page's admission queue so accepted edits finish before preparation reads the library.
    pub fn with_mod_worker(mut self, mods: ModWorker) -> Self {
        self.mods = Some(mods);
        self
    }

    fn process_name(&self) -> String {
        match &self.configured_executable {
            Some(executable) => executable.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            None => install::PROCESS_NAME.to_owned(),
        }
    }

    fn switch(&self, install: &GameInstall) -> ModLoaderSwitch {
        ModLoaderSwitch::new(install.directory(), Some(&self.loader_root(install)))
    }

    /// MelonLoader runs from a folder of its own, outside the game.
    fn staged(&self, install: &GameInstall) -> bool {
        !eg_core::paths::same_folder(&self.loader_root(install), install.directory())
    }

    /// How the game is started: through Steam online; else its executable with --offline, and for a game with mods whose
    /// MelonLoader runs from another folder, that folder as --melonloader.basedir (the form with "=" every version reads).
    pub fn command(&self, mode: LaunchMode, install: &GameInstall) -> Command {
        let mut command = Command::new(&install.executable);
        command.current_dir(install.directory()).arg("--offline");
        if mode == LaunchMode::OfflineWithMods && self.staged(install) {
            command.arg(format!(
                "--melonloader.basedir={}",
                eg_core::paths::trim_end_separator(&self.loader_root(install)).display()
            ));
        }
        command
    }

    fn start(&self, mode: LaunchMode, install: &GameInstall) -> Result<(), LaunchError> {
        if mode == LaunchMode::Online {
            return crate::shell::open(&format!("steam://rungameid/{}", install::STEAM_APP_ID))
                .map_err(|error| LaunchError::Refused(format!("Steam could not start the game: {error}")));
        }
        self.command(mode, install)
            .spawn()
            .map(drop)
            .map_err(|error| LaunchError::Refused(format!("The game could not be started: {error}")))
    }

    /// Makes Mods\ match the library and writes waiting setting changes, before MelonLoader can read either, and
    /// repairs the UnityEngine.CoreModule.dll MelonLoader generated for this game build if it would stop every mod.
    fn prepare_mods(&self, install: &GameInstall) -> Result<(), LaunchError> {
        let Some(mods) = &self.mods else {
            return Err(LaunchError::Refused("The mod library is not available, so the game cannot be started with mods.".into()));
        };
        let root = self.loader_root(install);
        if self.staged(install) {
            let staged = LoaderInstall::find(&root);
            let minimum = LoaderVersion::new(0, 6, 6);
            if staged.as_ref().is_none_or(|loader| loader.version < minimum) {
                let version = staged.map_or_else(|| "version unknown".into(), |loader| loader.version.to_string());
                return Err(LaunchError::Refused(format!(
                    "The MelonLoader in {} ({version}) can't run from a folder outside the game; that needs MelonLoader {minimum} or later. Update it there, or clear the MelonLoader folder in Settings.",
                    root.display()
                )));
            }
        }
        mods.call(move |library| {
            library.prepare(&root, &preferences_path(&root))?;
            if loader_state::needs_repair(&root) {
                loader_state::repair_core_module(&root)?;
            }
            Ok(())
        })
        .map_err(|error| LaunchError::Refused(format!("The mods could not be made ready, so the game was not started: {error}")))
    }

    /// A note that cannot be written does not undo a successful game start.
    fn launched(&self, install: &GameInstall, started: SystemTime) {
        if let Some(mods) = &self.mods
            && let Ok(when) = jiff::Timestamp::try_from(started)
        {
            let root = self.loader_root(install);
            let _ = mods.call(move |library| library.launched(&root, when));
        }
    }
}

impl GameHost for FileGameHost {
    fn find_install(&self) -> Option<GameInstall> {
        install::find(self.configured_executable.as_deref(), None)
    }

    fn find_running(&self) -> Option<RunningGame> {
        process::oldest_running(&self.process_name())
    }

    fn read_log(&self, game: &RunningGame) -> GameLogState {
        self.log.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).read(game.start_time)
    }

    fn read_mod_loader(&self, install: Option<&GameInstall>) -> ModLoaderState {
        install.map_or(ModLoaderState::NotInstalled, |install| self.switch(install).state())
    }

    fn loader_root(&self, install: &GameInstall) -> PathBuf {
        self.loader_directory.clone().unwrap_or_else(|| install.directory().to_path_buf())
    }

    fn read_game_version(&self, install: &GameInstall) -> Option<String> {
        crate::game_version::read(&self.player_log, &install.game_assembly())
    }

    fn launch(&self, mode: LaunchMode, install: Option<&GameInstall>, lifetime: &Lifetime) -> Result<LaunchOutcome, LaunchError> {
        if self.find_running().is_some() {
            return Err(LaunchError::Refused("Last Epoch is running already. Close it first to start it another way.".into()));
        }
        // Without the game folder, a proxy there could not be set aside, and Steam would start the game with mods.
        let Some(install) = install else {
            return Err(LaunchError::Refused(
                if mode == LaunchMode::Online {
                    "Last Epoch was not found, so the app can't make sure mods stay off online. Choose the game executable in Settings."
                } else {
                    "Last Epoch was not found, so it cannot be started offline. Choose the game executable in Settings."
                }
                .into(),
            ));
        };
        let loader = self.switch(install);
        let with_mods = mode == LaunchMode::OfflineWithMods;
        if with_mods {
            // A start MelonLoader can't join changes nothing: Mods\ and the waiting settings stay as they are.
            loader.check_restore().map_err(LaunchError::Refused)?;
            self.prepare_mods(install)?;
            if lifetime.is_cancelled() {
                return Err(LaunchError::Cancelled);
            }
            loader.restore().map_err(LaunchError::Refused)?;
        } else {
            loader.set_aside().map_err(LaunchError::Refused)?;
        }
        let started = SystemTime::now();
        let result = self.start(mode, install).map(|()| {
            if !with_mods {
                return LaunchOutcome::Started;
            }
            self.launched(install, started);
            if loader.wait_for_loader(started, loader::LOADER_TIMEOUT, Duration::from_millis(500), lifetime) {
                LaunchOutcome::LoaderStarted
            } else {
                LaunchOutcome::LoaderNotSeen
            }
        });
        let result = if lifetime.is_cancelled() { Err(LaunchError::Cancelled) } else { result };
        // Whatever happened after the restore, the proxy goes back aside: a start from Steam must never pick it up.
        if with_mods { finish_mod_launch(&loader, result) } else { result }
    }

    fn settle_mod_loader(&self, install: Option<&GameInstall>) -> bool {
        let Some(install) = install else {
            return false;
        };
        if self.find_running().is_some() {
            return false;
        }
        let loader = self.switch(install);
        // A rename that failed leaves it for the next start, which sets it aside before anything else.
        loader.state() == ModLoaderState::Active && loader.set_aside().is_ok()
    }
}

fn finish_mod_launch(loader: &ModLoaderSwitch, result: Result<LaunchOutcome, LaunchError>) -> Result<LaunchOutcome, LaunchError> {
    loader
        .set_aside()
        .map_err(|error| LaunchError::Failed(format!("MelonLoader's proxy could not be set aside. A start from Steam may still load mods: {error}")))?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::library::ModLibrary;
    use std::fs;

    fn game(temp: &Path) -> GameInstall {
        let install = GameInstall::new(temp.join("Game").join(install::EXECUTABLE_NAME));
        fs::create_dir_all(install.directory().join("MelonLoader")).unwrap();
        install
    }

    fn host(temp: &Path, install: &GameInstall) -> FileGameHost {
        FileGameHost::new(Some(&install.executable), &temp.join("Player.log"), None)
    }

    #[test]
    fn a_start_without_the_game_found_is_refused_even_online() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        let host = host(temp.path(), &install);
        let refused = host.launch(LaunchMode::Online, None, &Lifetime::default()).unwrap_err();
        assert_eq!(
            refused,
            LaunchError::Refused("Last Epoch was not found, so the app can't make sure mods stay off online. Choose the game executable in Settings.".into())
        );
    }

    #[test]
    fn a_conflict_refuses_every_start_and_renames_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        fs::write(install.directory().join(loader::PROXY_NAME), b"").unwrap();
        fs::write(install.directory().join(loader::SET_ASIDE_NAME), b"").unwrap();
        let host = host(temp.path(), &install);
        for mode in [LaunchMode::Offline, LaunchMode::OfflineWithMods] {
            assert!(matches!(host.launch(mode, Some(&install), &Lifetime::default()), Err(LaunchError::Refused(text)) if text.starts_with(loader::CONFLICT)));
        }
        assert!(install.directory().join(loader::PROXY_NAME).exists());
        assert!(install.directory().join(loader::SET_ASIDE_NAME).exists());
    }

    #[test]
    fn a_start_with_mods_waits_for_the_mod_library_and_leaves_the_proxy_aside() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        fs::write(install.directory().join(loader::SET_ASIDE_NAME), b"").unwrap();
        let host = host(temp.path(), &install);
        assert!(matches!(
            host.launch(LaunchMode::OfflineWithMods, Some(&install), &Lifetime::default()),
            Err(LaunchError::Refused(_))
        ));
        assert_eq!(host.read_mod_loader(Some(&install)), ModLoaderState::SetAside);
    }

    #[test]
    fn a_proxy_left_in_place_is_set_aside_at_startup() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        fs::write(install.directory().join(loader::PROXY_NAME), b"").unwrap();
        let host = host(temp.path(), &install);
        assert!(host.settle_mod_loader(Some(&install)));
        assert_eq!(host.read_mod_loader(Some(&install)), ModLoaderState::SetAside);
        assert!(!host.settle_mod_loader(Some(&install)));
    }

    #[test]
    fn cleanup_failure_is_reported_for_every_start_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        let loader = ModLoaderSwitch::new(install.directory(), None);
        fs::write(install.directory().join(loader::PROXY_NAME), b"proxy").unwrap();
        // A directory at the destination forces a real rename failure.
        fs::create_dir(install.directory().join(loader::SET_ASIDE_NAME)).unwrap();
        for outcome in [
            Ok(LaunchOutcome::LoaderStarted),
            Ok(LaunchOutcome::LoaderNotSeen),
            Err(LaunchError::Refused("start failed".into())),
        ] {
            assert!(matches!(finish_mod_launch(&loader, outcome), Err(LaunchError::Failed(message))
                if message.contains("could not be set aside") && message.contains("Steam may still load mods")));
            assert_eq!(loader.state(), ModLoaderState::Active);
        }
        fs::remove_dir(install.directory().join(loader::SET_ASIDE_NAME)).unwrap();
        assert_eq!(finish_mod_launch(&loader, Ok(LaunchOutcome::LoaderStarted)), Ok(LaunchOutcome::LoaderStarted));
        assert_eq!(loader.state(), ModLoaderState::SetAside);
    }

    #[test]
    fn an_offline_start_runs_the_executable_with_offline_in_its_folder() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        let staged = temp.path().join("Staged");
        let host = FileGameHost::new(Some(&install.executable), &temp.path().join("Player.log"), Some(&staged));
        let command = host.command(LaunchMode::OfflineWithMods, &install);
        assert_eq!(command.get_program(), install.executable.as_os_str());
        assert_eq!(command.get_current_dir(), Some(install.directory()));
        let arguments: Vec<_> = command.get_args().map(|argument| argument.to_string_lossy().into_owned()).collect();
        assert_eq!(arguments, ["--offline".to_owned(), format!("--melonloader.basedir={}", staged.display())]);
        assert_eq!(host.command(LaunchMode::Offline, &install).get_args().count(), 1);
    }

    #[test]
    fn preparation_waits_for_accepted_edits_and_launch_timestamp_is_best_effort() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        let library = ModLibrary::new(temp.path().join("Library"));
        let dll = temp.path().join("Fixture.dll");
        fs::write(&dll, include_bytes!("../tests/fixtures/melon/clean.bin")).unwrap();
        library.add(&dll, false).unwrap();
        let mods = ModWorker::new(library);
        let edited = mods.enqueue(|library| library.set_enabled("Fixture.dll", false)).unwrap();
        let host = host(temp.path(), &install).with_mod_worker(mods.clone());
        host.prepare_mods(&install).unwrap();
        edited.recv().unwrap().unwrap();
        assert!(!install.directory().join("Mods/Fixture.dll").exists());
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        host.launched(&install, started);
        let root = install.directory().to_path_buf();
        assert_eq!(
            mods.call(move |library| Ok(library.placed(&root)?.last_launch)).unwrap(),
            Some(jiff::Timestamp::try_from(started).unwrap())
        );
        mods.close();
        // Closing admission cannot turn a successful game start into a failure to record its timestamp.
        host.launched(&install, started);
    }

    #[test]
    fn staged_loader_is_checked_before_library_or_preferences_change() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        let staged = temp.path().join("Staged");
        let library = ModLibrary::new(temp.path().join("Library"));
        let mods = ModWorker::new(library);
        let host = FileGameHost::new(Some(&install.executable), &temp.path().join("Player.log"), Some(&staged)).with_mod_worker(mods.clone());
        assert!(matches!(host.prepare_mods(&install), Err(LaunchError::Refused(reason)) if reason.contains("needs MelonLoader 0.6.6 or later")));
        assert!(!staged.exists());
        assert!(!temp.path().join("Library").exists());
        fs::create_dir_all(staged.join("MelonLoader/net6")).unwrap();
        fs::write(staged.join("MelonLoader/net6/MelonLoader.dll"), include_bytes!("../tests/fixtures/melon/loader.bin")).unwrap();
        host.prepare_mods(&install).unwrap();
        assert!(staged.join("Mods").is_dir());
        assert!(temp.path().join("Library/placed.json").is_file());
        mods.close();
    }

    #[test]
    fn preparation_failure_keeps_proxy_aside_and_does_not_record_a_launch() {
        let temp = tempfile::tempdir().unwrap();
        let install = game(temp.path());
        fs::write(install.directory().join(loader::SET_ASIDE_NAME), b"proxy").unwrap();
        let directory = temp.path().join("Library");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("profile.json"), "damaged").unwrap();
        let mods = ModWorker::new(ModLibrary::new(directory.clone()));
        let host = host(temp.path(), &install).with_mod_worker(mods.clone());
        let result = host.launch(LaunchMode::OfflineWithMods, Some(&install), &Lifetime::default());
        assert!(matches!(result, Err(LaunchError::Refused(reason)) if reason.starts_with("The mods could not be made ready, so the game was not started:")));
        assert_eq!(host.read_mod_loader(Some(&install)), ModLoaderState::SetAside);
        assert!(!directory.join("placed.json").exists());
        mods.close();
    }
}
