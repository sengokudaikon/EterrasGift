//! Work that must not hold the UI thread. It runs on the runtime's blocking pool and comes back on the event loop as an
//! intent.

use flow::{Intents, Panicked};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use task_runtime::Tasks;

/// Starts work on the pool. Clone it into whatever hosts a feature. Whoever composes the app gives it a scope of the runtime
/// and closes that scope on the way out, after cancelling anything long; the runtime keeps the record of panics.
#[derive(Clone)]
pub struct Background {
    tasks: Tasks,
    /// Accepted work whose result hasn't been delivered yet.
    pending: Arc<AtomicUsize>,
}

impl Background {
    pub fn new(tasks: Tasks) -> Background {
        Background { tasks, pending: Arc::default() }
    }

    /// Runs `work` off the UI thread, then sends `then(result)` to `intents` on the UI thread. A panic reaches `then` as
    /// [`Panicked`], so each feature decides what it means. Call it from the UI thread.
    ///
    /// Accepted work always runs to its end. If the store is gone by then, the result goes nowhere, so anything whose outcome
    /// must be recorded belongs to a store that lives as long as that record matters. Once the scope closes no work is
    /// accepted: the app is on its way out.
    pub fn run<T, I>(&self, intents: &Intents<I>, work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(Result<T, Panicked>) -> I + 'static)
    where
        T: Send + 'static,
        I: 'static,
    {
        struct Delivered(Arc<AtomicUsize>);
        impl Drop for Delivered {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let Ok(finished) = self.tasks.spawn_blocking(work) else {
            return;
        };
        self.pending.fetch_add(1, Ordering::AcqRel);
        let delivered = Delivered(self.pending.clone());
        let intents = intents.clone();
        // Fails only where no event loop can ever run, and then nothing could show the result anyway.
        let _ = slint::spawn_local(async move {
            let _delivered = delivered;
            match finished.await {
                Ok(result) => intents.send(then(Ok(result))),
                Err(error) if error.is_panic() => intents.send(then(Err(Panicked))),
                // The runtime ended before the work began: the app is on its way out.
                Err(_) => {}
            }
        });
    }

    /// Accepted work whose result hasn't been delivered yet, for a test to wait on.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }
}
