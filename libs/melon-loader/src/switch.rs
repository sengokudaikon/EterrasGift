//! MelonLoader in the game folder. It starts through a proxy DLL that Windows loads into every start of the game,
//! whatever starts it. Setting the proxy aside (renamed beside itself) is how a start
//! without mods keeps MelonLoader out entirely. Only a folder with MelonLoader in it is touched, so another tool's
//! version.dll is left alone.

use crate::paths as melon_paths;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The proxy DLL Windows loads into every start of the game, which starts MelonLoader.
pub const PROXY_NAME: &str = "version.dll";
/// The proxy set aside beside itself, so the game starts without MelonLoader.
pub const SET_ASIDE_NAME: &str = "version.dll.disabled";

/// Why the proxy could not be switched.
#[derive(Debug)]
pub enum SwitchError {
    /// Both the proxy and a set-aside copy are there.
    Conflict,
    /// MelonLoader or its proxy is missing, and it runs from the game folder.
    NoLoader,
    /// MelonLoader or its proxy is missing, and it runs from a folder of its own.
    NoLoaderInFolder,
    /// The proxy could not be renamed.
    NotRenamed(io::Error),
}

/// Whether MelonLoader will start with the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModLoaderState {
    /// No MelonLoader folder beside the game, or no proxy for it: no mod can load, and nothing is touched.
    NotInstalled,
    /// The proxy is in place: Windows loads it on every start, whatever starts it.
    Active,
    /// The proxy is set aside, so the game starts without MelonLoader.
    SetAside,
    /// Both the proxy and a set-aside copy are there, so which MelonLoader is current can't be told.
    Conflict,
}

/// The proxy in the game folder, and MelonLoader where it runs from.
#[derive(Clone, Debug)]
pub struct ModLoaderSwitch {
    game_directory: PathBuf,
    loader_root: PathBuf,
}

impl ModLoaderSwitch {
    /// `loader_root` is where MelonLoader runs from when that is not the game folder; the proxy stays in the game folder.
    pub fn new(game_directory: &Path, loader_root: Option<&Path>) -> ModLoaderSwitch {
        ModLoaderSwitch {
            game_directory: game_directory.to_path_buf(),
            loader_root: loader_root.unwrap_or(game_directory).to_path_buf(),
        }
    }

    fn proxy(&self) -> PathBuf {
        self.game_directory.join(PROXY_NAME)
    }

    fn set_aside_path(&self) -> PathBuf {
        self.game_directory.join(SET_ASIDE_NAME)
    }

    /// The log MelonLoader starts writing as soon as it runs.
    pub fn loader_log(&self) -> PathBuf {
        melon_paths::latest_log(&self.loader_root)
    }

    pub fn state(&self) -> ModLoaderState {
        if !melon_paths::loader(&self.loader_root).is_dir() {
            return ModLoaderState::NotInstalled;
        }
        match (self.proxy().is_file(), self.set_aside_path().is_file()) {
            (true, true) => ModLoaderState::Conflict,
            (true, false) => ModLoaderState::Active,
            (false, true) => ModLoaderState::SetAside,
            (false, false) => ModLoaderState::NotInstalled,
        }
    }

    /// Makes sure the next start loads no MelonLoader. Renaming works while a running game holds the DLL.
    pub fn set_aside(&self) -> Result<(), SwitchError> {
        match self.state() {
            ModLoaderState::Active => rename(&self.proxy(), &self.set_aside_path()),
            ModLoaderState::Conflict => Err(SwitchError::Conflict),
            _ => Ok(()),
        }
    }

    /// Puts the proxy back so the next start loads MelonLoader and the mods that are on.
    pub fn restore(&self) -> Result<(), SwitchError> {
        if self.check_restore()? == ModLoaderState::SetAside {
            rename(&self.set_aside_path(), &self.proxy())?;
        }
        Ok(())
    }

    /// Says why `restore` could not work, without renaming anything, so a start with mods that can't happen is refused
    /// before it changes Mods\ or the settings. Ok is the state the proxy is in: Active or SetAside.
    pub fn check_restore(&self) -> Result<ModLoaderState, SwitchError> {
        match self.state() {
            ModLoaderState::Conflict => Err(SwitchError::Conflict),
            ModLoaderState::NotInstalled if file_ops::paths::same_place(&self.loader_root, &self.game_directory) => Err(SwitchError::NoLoader),
            ModLoaderState::NotInstalled => Err(SwitchError::NoLoaderInFolder),
            state => Ok(state),
        }
    }

    /// Waits until MelonLoader writes its log at or after `since`, the sign it has started and no longer needs the
    /// proxy's name. False when `timeout` passes first or `cancelled` says so.
    pub fn wait_for_loader(&self, since: SystemTime, timeout: Duration, poll: Duration, cancelled: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if std::fs::metadata(self.loader_log())
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|written| written >= since)
            {
                return true;
            }
            if Instant::now() >= deadline || cancelled() {
                return false;
            }
            std::thread::sleep(poll);
        }
    }
}

fn rename(from: &Path, to: &Path) -> Result<(), SwitchError> {
    std::fs::rename(from, to).map_err(SwitchError::NotRenamed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_proxy_is_set_aside_and_restored_only_where_melonloader_is() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path();
        let switch = ModLoaderSwitch::new(game, None);
        fs::write(game.join(PROXY_NAME), b"another tool's").unwrap();
        assert_eq!(switch.state(), ModLoaderState::NotInstalled);
        switch.set_aside().unwrap();
        assert!(game.join(PROXY_NAME).exists(), "another tool's version.dll is left alone");
        assert!(matches!(switch.check_restore(), Err(SwitchError::NoLoader)));

        fs::create_dir(game.join("MelonLoader")).unwrap();
        assert_eq!(switch.state(), ModLoaderState::Active);
        switch.set_aside().unwrap();
        assert_eq!(switch.state(), ModLoaderState::SetAside);
        switch.restore().unwrap();
        assert_eq!(switch.state(), ModLoaderState::Active);

        fs::write(game.join(SET_ASIDE_NAME), b"older").unwrap();
        assert_eq!(switch.state(), ModLoaderState::Conflict);
        assert!(matches!(switch.set_aside(), Err(SwitchError::Conflict)));
        assert!(switch.restore().is_err());
    }

    #[test]
    fn a_loader_in_a_folder_of_its_own_is_told_apart() {
        let temp = tempfile::tempdir().unwrap();
        let switch = ModLoaderSwitch::new(temp.path(), Some(&temp.path().join("elsewhere")));
        assert!(matches!(switch.check_restore(), Err(SwitchError::NoLoaderInFolder)));
    }

    #[test]
    fn waiting_for_the_loader_ends_when_its_log_is_written_or_time_runs_out() {
        let temp = tempfile::tempdir().unwrap();
        let switch = ModLoaderSwitch::new(temp.path(), None);
        let since = SystemTime::now() - Duration::from_secs(1);
        assert!(!switch.wait_for_loader(since, Duration::from_millis(30), Duration::from_millis(10), || false));
        fs::create_dir_all(melon_paths::loader(temp.path())).unwrap();
        fs::write(switch.loader_log(), b"MelonLoader v0.7.3").unwrap();
        assert!(switch.wait_for_loader(since, Duration::from_millis(30), Duration::from_millis(10), || false));
    }

    #[test]
    fn waiting_for_the_loader_ends_when_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let switch = ModLoaderSwitch::new(temp.path(), None);
        assert!(!switch.wait_for_loader(SystemTime::now(), Duration::from_secs(60), Duration::from_millis(1), || true));
    }
}
