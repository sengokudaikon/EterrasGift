//! Work that must not hold the UI thread. It runs on the app's pool and comes back on the event loop as an intent.

use spiderland::{Intents, Panicked};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::oneshot;

/// One piece of work for the app's pool to start.
pub type Job = Box<dyn FnOnce() + Send>;

/// Tells the feature that its work panicked: only an unwinding drops this with the reply unsent.
struct Reply<T>(Option<oneshot::Sender<Result<T, Panicked>>>);

impl<T> Drop for Reply<T> {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(Err(Panicked));
        }
    }
}

/// Counts a piece of accepted work off when its result has gone, or can't go.
struct Delivered(Arc<AtomicUsize>);

impl Drop for Delivered {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Starts work on the app's pool. Clone it into whatever hosts a feature.
#[derive(Clone)]
pub struct Background {
    start: Arc<dyn Fn(Job) -> bool + Send + Sync>,
    /// Accepted work whose result hasn't been delivered yet.
    pending: Arc<AtomicUsize>,
}

impl Background {
    /// `start` starts a job on the app's pool and says whether the pool took it. The app closes that pool on the way out, after
    /// cancelling anything long. A panic in a job goes on to the pool once the feature has been told, so the pool can keep a record.
    ///
    /// ```
    /// let background = spiderland_slint::Background::new(|job| {
    ///     std::thread::spawn(job);
    ///     true
    /// });
    /// # drop(background);
    /// ```
    pub fn new(start: impl Fn(Job) -> bool + Send + Sync + 'static) -> Background {
        Background {
            start: Arc::new(start),
            pending: Arc::default(),
        }
    }

    /// Runs `work` off the UI thread, then sends `then(result)` to `intents` on the UI thread. A panic reaches `then` as
    /// [`Panicked`], so each feature decides what it means. Call it from the UI thread.
    ///
    /// Accepted work always runs to its end. If the store is gone by then, the result goes nowhere, so anything whose outcome
    /// must be recorded belongs to a store that lives as long as that record matters. Work the pool refuses, or lets go of
    /// before it starts, sends nothing back: the app is on its way out.
    pub fn run<T, I>(&self, intents: &Intents<I>, work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(Result<T, Panicked>) -> I + 'static)
    where
        T: Send + 'static,
        I: 'static,
    {
        let (reply, replied) = oneshot::channel();
        let job = Box::new(move || {
            let mut reply = Reply(Some(reply));
            let result = work();
            if let Some(reply) = reply.0.take() {
                let _ = reply.send(Ok(result));
            }
        });
        if !(self.start)(job) {
            return;
        }
        let delivered = self.accepted();
        let intents = intents.clone();
        // Fails only where no event loop can ever run, and then nothing could show the result anyway.
        let _ = slint::spawn_local(async move {
            let _delivered = delivered;
            // No reply: the pool let go of the job before it began.
            if let Ok(result) = replied.await {
                intents.send(then(result));
            }
        });
    }

    /// Waits for `work` on the UI thread, then sends `then(result)` to `intents`: for an answer that arrives by itself, such as
    /// a reply from a worker, with no pool thread held while it does. It always runs to its end, and the pool closing doesn't
    /// stop it. Call it from the UI thread. A panic here is a panic on the UI thread, so `work` should only wait.
    pub fn wait<T, I>(&self, intents: &Intents<I>, work: impl Future<Output = T> + 'static, then: impl FnOnce(T) -> I + 'static)
    where
        T: 'static,
        I: 'static,
    {
        let delivered = self.accepted();
        let intents = intents.clone();
        // Fails only where no event loop can ever run, and then nothing could show the result anyway.
        let _ = slint::spawn_local(async move {
            let _delivered = delivered;
            let result = work.await;
            intents.send(then(result));
        });
    }

    fn accepted(&self) -> Delivered {
        self.pending.fetch_add(1, Ordering::AcqRel);
        Delivered(self.pending.clone())
    }

    /// Accepted work whose result hasn't been delivered yet, for a test to wait on.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }
}
