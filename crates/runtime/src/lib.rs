//! The desktop's background work on Tokio. [`Runtime`] owns one reactor thread and Tokio's blocking pool; [`Tasks`] is the
//! handle the rest of the app starts work with. Results come back as Tokio's own `JoinHandle`, and the channels between
//! owners are Tokio's own `mpsc`, `oneshot` and `watch`: this crate adds only what Tokio leaves to its user, a close barrier
//! and a record of panics.
//!
//! How work is meant to run on it:
//!
//! - Domain code stays synchronous and goes through [`Tasks::spawn_blocking`]. [`Tasks::spawn`] is for work that only
//!   waits; everything on it shares one thread, so nothing there may block.
//! - Cancellation is cooperative, through [`Tasks::token`]. Never `abort` a task: a write cut off partway leaves its files
//!   in a state nobody can name.
//! - A `JoinHandle` needs no runtime to be awaited, so the UI thread can await one inside `slint::spawn_local`.
//!
//! Code that runs in the game never uses this crate: the helper has a reactor and rules of its own.

#![forbid(unsafe_code)]

pub mod process;

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tokio::runtime::{Builder, Handle};
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

pub use tokio::task::{JoinError, JoinHandle};
pub use tokio_util::sync::CancellationToken;

/// How long the close waits for what no task accounts for: Tokio's own blocking reads of a child's pipes, which a stray
/// descendant can hold open for as long as it runs. All accepted work has ended by then.
const CLOSE_GRACE: Duration = Duration::from_secs(3);

/// The runtime has begun to close and accepts no new work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

/// Owns the reactor thread. Dropping it closes it as [`Runtime::shutdown`] does, so drop it from outside its own tasks.
pub struct Runtime {
    tasks: Tasks,
    reactor: Option<thread::JoinHandle<()>>,
}

impl Runtime {
    pub fn start() -> io::Result<Self> {
        let runtime = Builder::new_current_thread().enable_all().thread_name("background").build()?;
        let tasks = Tasks {
            handle: runtime.handle().clone(),
            tracker: TaskTracker::new(),
            token: CancellationToken::new(),
            ancestors: Vec::new(),
            faulted: Arc::default(),
        };
        let (root, tracker) = (tasks.token.clone(), tasks.tracker.clone());
        let reactor = thread::Builder::new().name("reactor".into()).spawn(move || {
            runtime.block_on(async {
                root.cancelled().await;
                tracker.close();
                tracker.wait().await;
            });
            runtime.shutdown_timeout(CLOSE_GRACE);
        })?;
        Ok(Self { tasks, reactor: Some(reactor) })
    }

    pub fn tasks(&self) -> Tasks {
        self.tasks.clone()
    }

    /// Cancels every token, refuses new work and waits for all accepted work to end, however long it takes: work that
    /// ignores its token holds this up. Says whether anything panicked over the runtime's life.
    pub fn shutdown(mut self) -> bool {
        self.close();
        self.tasks.faulted()
    }

    fn close(&mut self) {
        self.tasks.token.cancel();
        if let Some(reactor) = self.reactor.take()
            && reactor.join().is_err()
        {
            self.tasks.faulted.store(true, Ordering::Release);
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.close();
    }
}

/// Starts work on the [`Runtime`]. Cheap to clone, and usable from any thread, including the UI's.
///
/// A [`scope`](Tasks::scope) is the same handle for a part of the app that must end before the rest does: its owner
/// [`close`](Tasks::close)s it at its place in the app's shutdown, and the runtime still waits for anything in it.
#[derive(Clone)]
pub struct Tasks {
    handle: Handle,
    tracker: TaskTracker,
    token: CancellationToken,
    /// The scopes around this one, whose closes also wait for its work.
    ancestors: Vec<TaskTracker>,
    faulted: Arc<AtomicBool>,
}

impl Tasks {
    /// Runs synchronous work on the blocking pool.
    pub fn spawn_blocking<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> Result<JoinHandle<T>, Closed> {
        self.admit()?;
        let held = self.hold();
        Ok(self.tracker.spawn_blocking_on(
            move || {
                let _held = held;
                work()
            },
            &self.handle,
        ))
    }

    /// Runs a future on the reactor thread. A future that outlives the app's use of it must end when its [`Tasks::token`]
    /// is cancelled, or [`Runtime::shutdown`] waits for it forever.
    pub fn spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, Closed>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.admit()?;
        let held = self.hold();
        Ok(self.tracker.spawn_on(
            async move {
                let _held = held;
                future.await
            },
            &self.handle,
        ))
    }

    /// Waits on this thread for a future, such as a [`JoinHandle`], for code that is synchronous and already off the UI
    /// thread. Never call it inside a task on the reactor: Tokio panics rather than deadlock.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.handle.block_on(future)
    }

    /// Cancelled when this scope or the runtime closes. Cancelling it, or a child of it, stops only the work that holds
    /// that token.
    pub fn token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// A part of this scope that closes on its own. Panics in it count as this scope's.
    pub fn scope(&self) -> Tasks {
        let mut ancestors = self.ancestors.clone();
        ancestors.push(self.tracker.clone());
        Tasks {
            handle: self.handle.clone(),
            tracker: TaskTracker::new(),
            token: self.token.child_token(),
            ancestors,
            faulted: self.faulted.clone(),
        }
    }

    /// Cancels this scope's tokens, refuses new work in it and waits for its accepted work to end. Calling it again, or
    /// from several places at once, waits the same way. Other scopes carry on.
    pub async fn close(&self) {
        self.token.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }

    /// Whether any task in the runtime has panicked. A `JoinHandle` tells whoever awaits it; this also covers work nobody awaited, or
    /// that ended after the event loop stopped polling for it.
    pub fn faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }

    fn admit(&self) -> Result<(), Closed> {
        if self.token.is_cancelled() { Err(Closed) } else { Ok(()) }
    }

    fn hold(&self) -> Held {
        Held {
            faulted: self.faulted.clone(),
            _ancestors: self.ancestors.iter().map(TaskTracker::token).collect(),
        }
    }
}

/// Held for a task's whole run: it is dropped by the unwinding when the task panics, before the `JoinHandle` resolves.
struct Held {
    faulted: Arc<AtomicBool>,
    _ancestors: Vec<TaskTrackerToken>,
}

impl Drop for Held {
    fn drop(&mut self) {
        if thread::panicking() {
            self.faulted.store(true, Ordering::Release);
        }
    }
}
