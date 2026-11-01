//! A single admission queue shared by Mods and game preparation. Enqueue on the UI thread; wait only on workers.
//! A Play requested after an edit cannot overtake that edit, including when requested from Home.
//!
//! A task that panics stops the library and nothing else. What it left on disk is unknown, so no later task runs on it until
//! the app starts again: a task already queued gets no reply, and a new one is refused. The panic hook has reported the fault,
//! and the app's exit code follows [`ModWorker::faulted`].

use super::library::ModLibrary;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use task_runtime::{CancellationToken, JoinHandle, Tasks};
use tokio::sync::{mpsc, oneshot};

use spiderland_phrases::phrases;

phrases! {
    Closing: "The mod library is closing."
    Stopped: "The mod library stopped. Restart the app to use it again."
}

type Job = Box<dyn FnOnce(&ModLibrary) + Send>;
struct Owner {
    sender: Mutex<Option<mpsc::UnboundedSender<Job>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    tasks: Tasks,
    cancel: CancellationToken,
    fault: Arc<AtomicBool>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.sender.get_mut().unwrap().take();
    }
}

#[derive(Clone)]
pub struct ModWorker(Arc<Owner>);
impl ModWorker {
    pub fn new(library: ModLibrary, tasks: &Tasks) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        // Refused only on a runtime already closing: with the receiver gone, every task is then refused too.
        let worker = tasks.spawn(serve(library, receiver, tasks.token())).ok();
        Self(Arc::new(Owner {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(worker),
            tasks: tasks.clone(),
            cancel: tasks.token(),
            fault: Arc::new(AtomicBool::new(false)),
        }))
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.0.cancel.clone()
    }
    pub fn faulted(&self) -> bool {
        self.0.fault.load(Ordering::Acquire)
    }
    /// A dropped reply means the library stopped: a caller must not take that for a failure it can ask again after.
    pub fn enqueue<T: Send + 'static>(&self, task: impl FnOnce(&ModLibrary) -> io::Result<T> + Send + 'static) -> io::Result<oneshot::Receiver<io::Result<T>>> {
        let (reply, receive) = oneshot::channel();
        if self.faulted() {
            return Err(io::Error::other(Stopped.get()));
        }
        let fault = self.0.fault.clone();
        let sender = self.0.sender.lock().unwrap();
        let sender = sender.as_ref().ok_or_else(|| io::Error::other(Closing.get()))?;
        sender
            .send(Box::new(move |library| {
                if fault.load(Ordering::Acquire) {
                    return;
                }
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(library))) {
                    Ok(outcome) => {
                        let _ = reply.send(outcome);
                    }
                    Err(_) => fault.store(true, Ordering::Release),
                }
            }))
            .map_err(|_| io::Error::other(Stopped.get()))?;
        Ok(receive)
    }
    /// For the game host and fixtures, off the reactor. Never wait in an interactive UI callback.
    pub fn call<T: Send + 'static>(&self, task: impl FnOnce(&ModLibrary) -> io::Result<T> + Send + 'static) -> io::Result<T> {
        self.enqueue(task)?.blocking_recv().map_err(|_| io::Error::other(Stopped.get()))?
    }
    /// Cancels source builds, rejects new tasks and waits for accepted mutations to drain.
    pub fn close(&self) {
        self.0.cancel.cancel();
        self.0.sender.lock().unwrap().take();
        let worker = self.0.worker.lock().unwrap().take();
        if let Some(worker) = worker {
            let _ = self.0.tasks.block_on(worker);
        }
    }
}

/// Runs tasks one at a time, in the order accepted, each on the blocking pool. Everything accepted runs before it ends. It
/// ends when every sender is gone, or when the runtime closes around a worker nobody closed.
async fn serve(library: ModLibrary, mut jobs: mpsc::UnboundedReceiver<Job>, closing: CancellationToken) {
    let library = Arc::new(library);
    while let Some(job) = next(&mut jobs, &closing).await {
        let library = library.clone();
        // A job catches its task's panic, so this can only fail with the runtime gone.
        let _ = tokio::task::spawn_blocking(move || job(&library)).await;
    }
}

async fn next(jobs: &mut mpsc::UnboundedReceiver<Job>, closing: &CancellationToken) -> Option<Job> {
    if !jobs.is_closed() {
        if let Some(next) = closing.run_until_cancelled(jobs.recv()).await {
            return next;
        }
        jobs.close();
    }
    jobs.recv().await
}
