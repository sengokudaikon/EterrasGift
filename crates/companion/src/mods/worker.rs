//! A single admission queue shared by Mods and game preparation. Enqueue on the UI thread; wait only on workers.
//! A Play requested after an edit cannot overtake that edit, including when requested from Home.
//!
//! A task that panics stops the library and nothing else. What it left on disk is unknown, so no later task runs on it until
//! the app starts again: a task already queued gets no reply, and a new one is refused. The panic hook has reported the fault,
//! and the app's exit code follows [`ModWorker::faulted`].

use super::library::ModLibrary;
use std::io;
use task_runtime::{CancellationToken, Refusal, Serial, Tasks};
use tokio::sync::oneshot;

use phrases::phrases;

phrases! {
    Closing: "The mod library is closing."
    Stopped: "The mod library stopped. Restart the app to use it again."
}

#[derive(Clone)]
pub struct ModWorker {
    library: Serial<ModLibrary>,
    /// Cancelled first at a close, so source builds stop before the queue drains.
    cancel: CancellationToken,
}

impl ModWorker {
    pub fn new(library: ModLibrary, tasks: &Tasks) -> Self {
        Self {
            library: Serial::start(library, tasks),
            cancel: tasks.token(),
        }
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn faulted(&self) -> bool {
        self.library.stopped()
    }
    /// A dropped reply means the library stopped: a caller must not take that for a failure it can ask again after.
    pub fn enqueue<T: Send + 'static>(&self, task: impl FnOnce(&ModLibrary) -> io::Result<T> + Send + 'static) -> io::Result<oneshot::Receiver<io::Result<T>>> {
        self.library.enqueue(move |library| task(library)).map_err(|refusal| {
            io::Error::other(match refusal {
                Refusal::Closed => Closing.get(),
                Refusal::Stopped => Stopped.get(),
            })
        })
    }
    /// For the game host and fixtures, off the reactor. Never wait in an interactive UI callback.
    pub fn call<T: Send + 'static>(&self, task: impl FnOnce(&ModLibrary) -> io::Result<T> + Send + 'static) -> io::Result<T> {
        self.enqueue(task)?.blocking_recv().map_err(|_| io::Error::other(Stopped.get()))?
    }
    /// Cancels source builds, rejects new tasks and waits for accepted mutations to drain.
    pub fn close(&self) {
        self.cancel.cancel();
        self.library.close();
    }
}
