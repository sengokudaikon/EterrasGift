//! One owner of a state, and work on it one job at a time: [`Serial`].

use crate::{CancellationToken, JoinHandle, Tasks};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{mpsc, oneshot};

/// Runs one job on the state, and says whether it ended without a panic. It sets `stopped` before its reply ends, so a
/// caller whose reply ended sees [`Serial::stopped`].
type Job<S> = Box<dyn FnOnce(&mut S, &AtomicBool) -> bool + Send>;

/// Why [`Serial::enqueue`] took no job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// [`Serial::close`] was called, or the runtime is closing.
    Closed,
    /// A job panicked, and the state it left went with it.
    Stopped,
}

/// Owns a state and runs jobs on it one at a time, in the order they were accepted, each on the blocking pool. It never
/// drops a job it accepted: a close, or the runtime's, refuses new jobs and still runs the queued ones. A job that must not
/// run then checks a token of its own.
///
/// A job that panics stops it. No later job runs on the state it left: the state is dropped, the queued jobs are dropped
/// unrun, so their replies end at once, and new ones are refused with [`Refusal::Stopped`]. The panic is contained there:
/// the panic hook has seen it, [`Serial::stopped`] says so, and it does not count as the runtime's ([`Tasks::faulted`]),
/// since what a stopped owner means is its owner's to say. A job that must carry on after a panic catches its own.
///
/// The queue is unbounded, so [`Serial::enqueue`] never waits and is safe on a UI thread. Cheap to clone; the clones
/// share one queue, and the last one dropped lets the queued jobs run and the worker end.
pub struct Serial<S> {
    inner: Arc<Inner<S>>,
}

impl<S> Clone for Serial<S> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

struct Inner<S> {
    jobs: Mutex<Option<mpsc::UnboundedSender<Job<S>>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    tasks: Tasks,
    stopped: Arc<AtomicBool>,
}

impl<S: Send + 'static> Serial<S> {
    /// Starts the worker on `tasks`. On a runtime already closing it starts none, and every job is refused.
    pub fn start(state: S, tasks: &Tasks) -> Self {
        let (jobs, queue) = mpsc::unbounded_channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = tasks.spawn(serve(state, queue, tasks.clone(), tasks.token(), stopped.clone())).ok();
        let jobs = worker.is_some().then_some(jobs);
        Self {
            inner: Arc::new(Inner {
                jobs: Mutex::new(jobs),
                worker: Mutex::new(worker),
                tasks: tasks.clone(),
                stopped,
            }),
        }
    }

    /// Queues `job` behind every job accepted before it. The receiver has its result, or ends without one if the job
    /// never ran, when a job before it panicked.
    pub fn enqueue<T: Send + 'static>(&self, job: impl FnOnce(&mut S) -> T + Send + 'static) -> Result<oneshot::Receiver<T>, Refusal> {
        let refusal = || if self.stopped() { Refusal::Stopped } else { Refusal::Closed };
        let (reply, result) = oneshot::channel();
        let jobs = self.inner.jobs.lock().unwrap();
        let jobs = jobs.as_ref().ok_or_else(refusal)?;
        jobs.send(Box::new(move |state, stopped| match catch_unwind(AssertUnwindSafe(|| job(state))) {
            Ok(result) => {
                let _ = reply.send(result);
                true
            }
            Err(_) => {
                stopped.store(true, Ordering::Release);
                false
            }
        }))
        .map_err(|_| refusal())?;
        Ok(result)
    }

    /// Whether a job panicked: see [`Serial`].
    pub fn stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::Acquire)
    }

    /// Refuses new jobs and waits on this thread until every accepted one has run. Calling it again, or from several
    /// threads at once, waits the same way. Like [`Tasks::block_on`], never call it inside a task on the reactor, nor in
    /// one of its own jobs.
    pub fn close(&self) {
        self.inner.jobs.lock().unwrap_or_else(PoisonError::into_inner).take();
        // Held across the wait: a closer that comes while another waits finds the worker gone only once it has ended.
        let mut worker = self.inner.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(handle) = worker.take() {
            let _ = self.inner.tasks.block_on(handle);
        }
    }
}

/// Runs the jobs in order until every sender is gone, or until the runtime closes around a worker nobody closed. Either
/// way the jobs already queued run first.
async fn serve<S: Send + 'static>(mut state: S, mut queue: mpsc::UnboundedReceiver<Job<S>>, tasks: Tasks, closing: CancellationToken, stopped: Arc<AtomicBool>) {
    while let Some(job) = next(&mut queue, &closing).await {
        let flag = stopped.clone();
        // A job that panicked drops the state: nothing may run on what it left.
        let run = tasks.run_accepted(move || job(&mut state, &flag).then_some(state));
        match run.await {
            Ok(Some(kept)) => state = kept,
            // The job catches its own panic, so a JoinError is one in the state's own drop.
            Ok(None) | Err(_) => {
                stopped.store(true, Ordering::Release);
                queue.close();
                while queue.try_recv().is_ok() {}
                return;
            }
        }
    }
}

/// The next job. Once `closing` is cancelled, the queue takes no more, and the ones in it still come out.
async fn next<J>(queue: &mut mpsc::UnboundedReceiver<J>, closing: &CancellationToken) -> Option<J> {
    if !queue.is_closed() {
        if let Some(next) = closing.run_until_cancelled(queue.recv()).await {
            return next;
        }
        queue.close();
    }
    queue.recv().await
}
