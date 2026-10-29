//! How Home and the Mods page start the game in its three ways: one start at a time, each said in one sentence. The
//! game process itself is never touched; only Steam, the executable and MelonLoader's proxy in the game folder are.

use crate::game::install::GameInstall;
use crate::game::process::RunningGame;
use crate::launch::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use crate::launch::loader::{self, ModLoaderState};
use eg_core::game_log::GameLogState;
use eg_core::operations::Cancellation;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// What a start came to, in one sentence; `started` is false when the game was not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchReport {
    pub started: bool,
    pub message: String,
}

/// Said while a start with mods waits for MelonLoader, which can take minutes after a game update.
pub const STARTING_WITH_MODS: &str = "Starting offline with mods. After a game update MelonLoader can take a few minutes.";

pub struct GameLauncher {
    host: Arc<dyn GameHost>,
    // Cancelled when the app closes, so a start with mods still waiting for MelonLoader sets its proxy aside first.
    lifetime: Cancellation,
    starting: Mutex<bool>,
    finished: Condvar,
}

impl GameLauncher {
    /// How often a page that follows the game looks again: a process lookup by name and the new lines of Player.log.
    pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);

    pub fn new(host: Arc<dyn GameHost>) -> GameLauncher {
        GameLauncher {
            host,
            lifetime: Cancellation::default(),
            starting: Mutex::new(false),
            finished: Condvar::new(),
        }
    }

    pub fn is_starting(&self) -> bool {
        *self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn find_install(&self) -> Option<GameInstall> {
        self.host.find_install()
    }

    pub fn find_running(&self) -> Option<RunningGame> {
        self.host.find_running()
    }

    pub fn loader_root(&self, install: &GameInstall) -> PathBuf {
        self.host.loader_root(install)
    }

    /// A best-effort logged version for display, or None when unavailable.
    pub fn read_game_version(&self, install: &GameInstall) -> Option<String> {
        self.host.read_game_version(install)
    }

    pub fn read_log(&self, game: &RunningGame) -> GameLogState {
        self.host.read_log(game)
    }

    pub fn read_mod_loader(&self) -> ModLoaderState {
        self.host.read_mod_loader(self.host.find_install().as_ref())
    }

    /// At startup: sets aside a MelonLoader proxy an earlier start left in place while no game runs. Takes the install
    /// the caller found (`find_install`), so startup looks for the game once.
    pub fn settle_mod_loader(&self, install: Option<&GameInstall>) -> bool {
        self.host.settle_mod_loader(install)
    }

    /// Starts the game and says how it went. Blocks while a start with mods waits for MelonLoader, so call it off the
    /// UI thread.
    pub fn play(&self, mode: LaunchMode) -> LaunchReport {
        {
            let mut starting = self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.lifetime.is_cancelled() {
                return refused("The app is closing.");
            }
            if *starting {
                return refused("Last Epoch is already starting.");
            }
            *starting = true;
        }
        // Reset and wake shutdown even when the host panics; the operation owner reports the fault.
        struct Finished<'a>(&'a GameLauncher);
        impl Drop for Finished<'_> {
            fn drop(&mut self) {
                *self.0.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
                self.0.finished.notify_all();
            }
        }
        let _finished = Finished(self);
        let outcome = self.host.launch(mode, self.host.find_install().as_ref(), &self.lifetime);
        match outcome {
            Ok(outcome) => LaunchReport {
                started: true,
                message: started(mode, outcome),
            },
            Err(LaunchError::Refused(reason)) => refused(&reason),
            // Said like any other refusal: a start that fails must not take the page down with it.
            Err(LaunchError::Failed(reason)) => refused(&format!("Last Epoch didn't start: {reason}")),
            Err(LaunchError::Cancelled) => refused("The app is closing."),
        }
    }

    /// Stops waiting for MelonLoader and waits for a start under way to set the proxy aside; for the app's close.
    pub fn stop(&self) {
        self.cancel();
        let starting = self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(self.finished.wait_while(starting, |starting| *starting));
    }

    pub fn cancel(&self) {
        self.lifetime.cancel();
    }
}

fn refused(message: &str) -> LaunchReport {
    LaunchReport {
        started: false,
        message: message.to_owned(),
    }
}

fn started(mode: LaunchMode, outcome: LaunchOutcome) -> String {
    match (mode, outcome) {
        (LaunchMode::Online, _) => "Started through Steam, without mods.".into(),
        (LaunchMode::Offline, _) => "Started offline, without mods.".into(),
        (_, LaunchOutcome::LoaderStarted) => "Started offline with mods.".into(),
        _ => format!(
            "Started offline with mods, but MelonLoader wrote no log in {} minutes. See MelonLoader/Latest.log.",
            loader::LOADER_TIMEOUT.as_secs() / 60
        ),
    }
}
