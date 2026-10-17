//! How Home and the Mods page start the game in its three ways: one start at a time, each said in one sentence
//! (`spec/companion.md` §4). The game process itself is never touched; only Steam, the executable and MelonLoader's
//! proxy in the game folder are.

use crate::Lifetime;
use crate::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use crate::install::GameInstall;
use crate::loader::{self, ModLoaderState};
use crate::process::RunningGame;
use eg_core::game_log::GameLogState;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// What a start came to, in one sentence; `started` is false when the game was not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchReport {
    pub started: bool,
    pub message: String,
}

/// Said while a start with mods waits for MelonLoader, which can take minutes after a game update.
pub const STARTING_WITH_MODS: &str = "Starting Last Epoch offline with mods. MelonLoader takes a moment, and a few minutes after a game update.";

pub struct GameLauncher {
    host: Arc<dyn GameHost>,
    // Cancelled when the app closes, so a start with mods still waiting for MelonLoader sets its proxy aside first.
    lifetime: Lifetime,
    starting: Mutex<bool>,
    finished: Condvar,
}

impl GameLauncher {
    /// How often a page that follows the game looks again: a process lookup by name and the new lines of Player.log.
    pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);

    pub fn new(host: Arc<dyn GameHost>) -> GameLauncher {
        GameLauncher {
            host,
            lifetime: Lifetime::default(),
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

    pub fn read_log(&self, game: &RunningGame) -> GameLogState {
        self.host.read_log(game)
    }

    pub fn read_mod_loader(&self) -> ModLoaderState {
        self.host.read_mod_loader(self.host.find_install().as_ref())
    }

    /// At startup: sets aside a MelonLoader proxy an earlier start left in place while no game runs.
    pub fn settle_mod_loader(&self) -> bool {
        self.host.settle_mod_loader(self.host.find_install().as_ref())
    }

    /// Starts the game and says how it went. Blocks while a start with mods waits for MelonLoader, so call it off the
    /// UI thread.
    pub fn play(&self, mode: LaunchMode) -> LaunchReport {
        {
            let mut starting = self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if *starting {
                return refused("Last Epoch is starting already.");
            }
            *starting = true;
        }
        let outcome = self.host.launch(mode, self.host.find_install().as_ref(), &self.lifetime);
        *self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        self.finished.notify_all();
        match outcome {
            Ok(outcome) => LaunchReport {
                started: true,
                message: started(mode, outcome),
            },
            Err(LaunchError::Refused(reason)) => refused(&reason),
            // Said like any other refusal: a start that fails must not take the page down with it.
            Err(LaunchError::Failed(reason)) => refused(&format!("Starting Last Epoch went wrong: {reason}")),
            Err(LaunchError::Cancelled) => refused("The app is closing."),
        }
    }

    /// Stops waiting for MelonLoader and waits for a start under way to set the proxy aside; for the app's close.
    pub fn stop(&self) {
        self.lifetime.cancel();
        let starting = self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(self.finished.wait_while(starting, |starting| *starting));
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
        (LaunchMode::Online, _) => "Started Last Epoch through Steam, without mods.".into(),
        (LaunchMode::Offline, _) => "Started Last Epoch offline, without mods.".into(),
        (_, LaunchOutcome::LoaderStarted) => format!(
            "Started Last Epoch offline with mods. MelonLoader is running, and its {} is set aside again so a start from Steam stays without mods.",
            loader::PROXY_NAME
        ),
        _ => format!(
            "Started Last Epoch offline with mods, but MelonLoader wrote no log within {} minutes; see MelonLoader/Latest.log. Its {} is set aside again.",
            loader::LOADER_TIMEOUT.as_secs() / 60,
            loader::PROXY_NAME
        ),
    }
}
