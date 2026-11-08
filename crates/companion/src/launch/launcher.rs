//! How Home and the Mods page start the game in its three ways: one start at a time, each said in one sentence. The
//! game process itself is never touched; only Steam, the executable and MelonLoader's proxy in the game folder are.

use crate::launch::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use crate::launch::loader::{self, ModLoaderState};
use phrases::phrases;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use task_runtime::CancellationToken;

/// What a start came to, in one sentence; `started` is false when the game was not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchReport {
    pub started: bool,
    pub message: String,
}

phrases! {
    /// Said while a start with mods waits for MelonLoader, which can take minutes after a game update.
    pub StartingWithMods: "Starting offline with mods. After a game update MelonLoader can take a few minutes."
    Closing: "The app is closing."
    AlreadyStarting: "Last Epoch is already starting."
    DidNotStart: "Last Epoch didn't start: {reason}"
    StartedOnline: "Started through Steam, without mods."
    StartedOffline: "Started offline, without mods."
    StartedWithMods: "Started offline with mods."
    NoLoaderLog: "Started offline with mods, but MelonLoader wrote no log in {minutes} minutes. See MelonLoader/Latest.log."
}

pub struct GameLauncher {
    host: Arc<dyn GameHost>,
    // Cancelled when the app closes, so a start with mods still waiting for MelonLoader sets its proxy aside first.
    lifetime: CancellationToken,
    starting: Mutex<bool>,
    finished: Condvar,
}

impl GameLauncher {
    /// How often a page that follows the game looks again: a process lookup by name and the new lines of Player.log.
    pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);

    pub fn new(host: Arc<dyn GameHost>) -> GameLauncher {
        GameLauncher {
            host,
            lifetime: CancellationToken::default(),
            starting: Mutex::new(false),
            finished: Condvar::new(),
        }
    }

    pub fn is_starting(&self) -> bool {
        *self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What the launcher starts the game through: finding it, watching it and its mod loader.
    pub fn host(&self) -> &dyn GameHost {
        self.host.as_ref()
    }

    pub fn read_mod_loader(&self) -> ModLoaderState {
        self.host.read_mod_loader(self.host.find_install().as_ref())
    }

    /// Starts the game and says how it went. Blocks while a start with mods waits for MelonLoader, so call it off the
    /// UI thread.
    pub fn play(&self, mode: LaunchMode) -> LaunchReport {
        {
            let mut starting = self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.lifetime.is_cancelled() {
                return refused(Closing.get());
            }
            if *starting {
                return refused(AlreadyStarting.get());
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
            Err(LaunchError::Failed(reason)) => refused(&DidNotStart.with(&[("reason", &reason)])),
            Err(LaunchError::Cancelled) => refused(Closing.get()),
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
        (LaunchMode::Online, _) => StartedOnline.into(),
        (LaunchMode::Offline, _) => StartedOffline.into(),
        (_, LaunchOutcome::LoaderStarted) => StartedWithMods.into(),
        _ => NoLoaderLog.with(&[("minutes", &(loader::LOADER_TIMEOUT.as_secs() / 60))]),
    }
}
