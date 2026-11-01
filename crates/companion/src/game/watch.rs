//! One watch on the game for every page: whether it runs, what its log says about it and what the launcher is doing. Pages
//! say they are following with a [`Following`] guard and read [`GameSeen`] from a watch channel, so looks never overlap, a
//! look that finds what the last one did wakes nobody, and with nobody following it costs nothing.

use crate::game::process::RunningGame;
use crate::home::HomeModel;
use crate::launch::launcher::GameLauncher;
use crate::launch::loader::ModLoaderState;
use eg_core::bloom::BloomPhase;
use eg_core::game_log::GameReadiness;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use task_runtime::{CancellationToken, DropGuard, Tasks};
use tokio::sync::{Notify, watch};

/// What the latest look found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameSeen {
    pub running: Option<RunningGame>,
    /// What Player.log says about this run, while one is running.
    pub readiness: Option<GameReadiness>,
    /// As last read by [`GameWatch::refresh`]: it costs more than the rest, so ordinary looks leave it.
    pub loader: Option<ModLoaderState>,
    /// One launcher starts the game for every page, so a start begun elsewhere shows here too.
    pub starting: bool,
    /// The latest look panicked, so the rest may be out of date. The panic hook has reported it.
    pub failed: bool,
}

impl GameSeen {
    /// Where Eterra's tree goes for this.
    pub fn phase(&self) -> BloomPhase {
        HomeModel::phase_of(self.running.as_ref(), |_| self.readiness.unwrap_or_default())
    }
}

struct Shared {
    launcher: Arc<GameLauncher>,
    seen: watch::Sender<GameSeen>,
    followers: AtomicUsize,
    /// A look that must also read the loader and tell subscribers even if it finds nothing new.
    refresh: AtomicBool,
    wake: Notify,
}

impl Shared {
    fn publish(&self, found: Result<GameSeen, Panicked>, refresh: bool) {
        self.seen.send_if_modified(|seen| {
            let next = match found {
                Ok(found) => GameSeen {
                    loader: found.loader.or(seen.loader),
                    ..found
                },
                Err(Panicked) => GameSeen { failed: true, ..*seen },
            };
            let changed = refresh || *seen != next;
            *seen = next;
            changed
        });
    }
}

struct Panicked;

/// Finds the install, reads Steam's files, takes a process snapshot and reads Player.log, so it runs off the UI thread.
fn look(launcher: &GameLauncher, refresh: bool) -> GameSeen {
    let loader = refresh.then(|| launcher.read_mod_loader());
    let running = launcher.host().find_running();
    let readiness = running.as_ref().map(|game| launcher.host().read_log(game).readiness);
    GameSeen {
        running,
        readiness,
        loader,
        starting: launcher.is_starting(),
        failed: false,
    }
}

/// Keeps the watch looking while it lives.
#[must_use = "the watch stops looking for this follower when this is dropped"]
pub struct Following(Arc<Shared>);

impl Drop for Following {
    fn drop(&mut self) {
        self.0.followers.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Cheap to clone. The watch ends with its last clone, or when the runtime closes.
#[derive(Clone)]
pub struct GameWatch {
    shared: Arc<Shared>,
    /// Ends the task with the last clone; none when every look happens in [`GameWatch::refresh`].
    task: Option<Arc<DropGuard>>,
}

impl GameWatch {
    /// Looks every [`GameLauncher::WATCH_INTERVAL`] while anyone follows.
    pub fn start(launcher: Arc<GameLauncher>, tasks: &Tasks) -> Self {
        Self::every(GameLauncher::WATCH_INTERVAL, launcher, tasks)
    }

    pub fn every(interval: Duration, launcher: Arc<GameLauncher>, tasks: &Tasks) -> Self {
        let shared = Self::shared(launcher);
        let closing = tasks.token();
        // Refused only on a runtime already closing, and then there is nothing left to look for.
        let _ = tasks.spawn(watch_game(shared.clone(), interval, tasks.clone(), closing.clone()));
        Self {
            shared,
            task: Some(Arc::new(closing.drop_guard())),
        }
    }

    /// Deterministic fixture mode: nothing runs in the background, and [`GameWatch::refresh`] looks before it returns.
    pub fn inline(launcher: Arc<GameLauncher>) -> Self {
        Self {
            shared: Self::shared(launcher),
            task: None,
        }
    }

    fn shared(launcher: Arc<GameLauncher>) -> Arc<Shared> {
        Arc::new(Shared {
            launcher,
            seen: watch::channel(GameSeen::default()).0,
            followers: AtomicUsize::new(0),
            refresh: AtomicBool::new(false),
            wake: Notify::new(),
        })
    }

    /// Keeps it looking until the guard is dropped. The first look comes at once.
    pub fn follow(&self) -> Following {
        self.shared.followers.fetch_add(1, Ordering::AcqRel);
        self.shared.wake.notify_one();
        Following(self.shared.clone())
    }

    /// Each change to what is seen. It needs no runtime to be awaited, so a UI can await it on its own thread. Subscribing
    /// alone starts no looks: see [`GameWatch::follow`].
    pub fn subscribe(&self) -> watch::Receiver<GameSeen> {
        self.shared.seen.subscribe()
    }

    pub fn current(&self) -> GameSeen {
        *self.shared.seen.borrow()
    }

    /// Looks as soon as it can, whether or not anyone follows: reads the loader too, and tells subscribers even if nothing
    /// changed, so a page coming into view hears something. Asking again during a look makes one more, not several.
    pub fn refresh(&self) {
        if self.task.is_some() {
            self.shared.refresh.store(true, Ordering::Release);
            self.shared.wake.notify_one();
        } else {
            self.shared.publish(Ok(look(&self.shared.launcher, true)), true);
        }
    }
}

async fn watch_game(shared: Arc<Shared>, interval: Duration, tasks: Tasks, closing: CancellationToken) {
    loop {
        let wanted = shared.followers.load(Ordering::Acquire) > 0 || shared.refresh.load(Ordering::Acquire);
        if !wanted {
            if closing.run_until_cancelled(shared.wake.notified()).await.is_none() {
                return;
            }
            continue;
        }
        let refresh = shared.refresh.swap(false, Ordering::AcqRel);
        let launcher = shared.launcher.clone();
        let Ok(looking) = tasks.spawn_blocking(move || look(&launcher, refresh)) else {
            return;
        };
        shared.publish(looking.await.map_err(|_| Panicked), refresh);
        tokio::select! {
            () = closing.cancelled() => return,
            () = shared.wake.notified() => {}
            () = tokio::time::sleep(interval) => {}
        }
    }
}
