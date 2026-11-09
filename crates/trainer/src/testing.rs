//! Stand-ins for what the worker looks at, for tests and captures that run it with no game: the game's processes, its log,
//! and the preparation of a connection. Behind the `testing` feature.

use crate::backend::{BackendConnection, BackendError, BackendHandle, BackendSnapshot, Connector, ProcessIdentity, ProcessLocator};
use crate::connect::gate::LogSource;
use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness, LoggedCharacter};
use jiff::Timestamp;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// A handle on snapshots the test publishes through the sender, at the moments it chooses, with no worker behind it: what
/// a screen reads of a worker, without one.
pub fn published(first: BackendSnapshot) -> (BackendHandle, watch::Sender<Arc<BackendSnapshot>>) {
    let (publish, published) = watch::channel(Arc::new(first));
    (BackendHandle::watching(published), publish)
}

/// Copies of one game, as a locator finds them. How many can change while the worker runs.
#[derive(Clone, Debug)]
pub struct Games {
    target: ProcessIdentity,
    count: Arc<AtomicUsize>,
}

impl Games {
    /// The one game `target`.
    pub fn one(target: ProcessIdentity) -> Games {
        Games::counted(target, Arc::new(AtomicUsize::new(1)))
    }

    /// As many copies of `target` as `count` says when the worker looks.
    pub fn counted(target: ProcessIdentity, count: Arc<AtomicUsize>) -> Games {
        Games { target, count }
    }
}

impl ProcessLocator for Games {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        Ok(vec![self.target.clone(); self.count.load(Ordering::SeqCst)])
    }
}

/// A game's log as the gate reads it, saying what a test sets.
#[derive(Clone, Debug)]
pub struct Log(Arc<Mutex<GameLogState>>);

impl Log {
    pub fn new(state: GameLogState) -> Log {
        Log(Arc::new(Mutex::new(state)))
    }

    /// What every read says from now on.
    pub fn set(&self, state: GameLogState) {
        *self.0.lock().unwrap() = state;
    }
}

impl LogSource for Log {
    fn read(&mut self, _: Timestamp) -> GameLogState {
        self.0.lock().unwrap().clone()
    }
}

/// A current log with a character in play: "Owned fixture", a level 12 Primalist in Legacy, offline unless `online`.
pub fn character(online: bool) -> GameLogState {
    GameLogState {
        status: GameLogStatus::Current,
        readiness: if online { GameReadiness::Online } else { GameReadiness::Offline },
        shell_state: Some("None".into()),
        offline_mode: Some(!online),
        character: Some(LoggedCharacter {
            name: "Owned fixture".into(),
            level: Some(12),
            online,
            cycle: "Legacy".into(),
            class: "Primalist".into(),
        }),
        problem: None,
    }
}

/// A current log of a game with no character loaded yet.
pub fn no_character() -> GameLogState {
    GameLogState {
        status: GameLogStatus::Current,
        readiness: GameReadiness::NoCharacter,
        shell_state: Some("None".into()),
        offline_mode: None,
        character: None,
        problem: None,
    }
}

/// A log the gate doesn't recognise, with no character in it.
pub fn unrecognized() -> GameLogState {
    GameLogState {
        status: GameLogStatus::Unrecognized,
        readiness: GameReadiness::NoCharacter,
        shell_state: Some("None".into()),
        offline_mode: Some(false),
        character: None,
        problem: None,
    }
}

/// A connector the worker must never reach, because its gate stops first: a connect panics and fails the test.
pub struct Unreached<C>(PhantomData<fn() -> C>);

impl<C> Unreached<C> {
    pub fn new() -> Self {
        Unreached(PhantomData)
    }
}

impl<C> Default for Unreached<C> {
    fn default() -> Self {
        Unreached::new()
    }
}

impl<C> std::fmt::Debug for Unreached<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Unreached")
    }
}

impl<C: BackendConnection> Connector for Unreached<C> {
    type Connection = C;

    async fn connect(&mut self, target: ProcessIdentity, _: Timestamp) -> Result<C, BackendError> {
        panic!("the worker connected to {target:?}, where the gate should have stopped it")
    }
}

/// A connector that hands the worker a connection `make` makes, after `delay`.
pub struct Connects<F> {
    make: F,
    delay: Duration,
}

impl<F> Connects<F> {
    pub fn new(make: F) -> Self {
        Connects::after(Duration::ZERO, make)
    }

    /// Each connect takes `delay`, as preparing the game does, so what shows while it waits can be seen.
    pub fn after(delay: Duration, make: F) -> Self {
        Connects { make, delay }
    }
}

impl<F> std::fmt::Debug for Connects<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connects").field("delay", &self.delay).finish_non_exhaustive()
    }
}

impl<C: BackendConnection, F: FnMut(ProcessIdentity) -> C + Send + 'static> Connector for Connects<F> {
    type Connection = C;

    async fn connect(&mut self, target: ProcessIdentity, _: Timestamp) -> Result<C, BackendError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok((self.make)(target))
    }
}
