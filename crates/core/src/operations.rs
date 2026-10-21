//! Owned background operations: cooperative cancellation, panic outcomes, and a close barrier.
//! Cancellation never claims an already started write was undone; its owner must inspect the actual result.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationError {
    Cancelled,
    Panicked,
    Closing,
}

#[derive(Default)]
struct State {
    closing: bool,
    workers: Vec<JoinHandle<()>>,
}

#[derive(Default)]
pub struct Operations {
    state: Mutex<State>,
    joining: Mutex<()>,
    cancellation: Cancellation,
    faulted: Arc<AtomicBool>,
}

impl Operations {
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl FnOnce(Cancellation) -> T + Send + 'static,
        complete: impl FnOnce(Result<T, OperationError>) + Send + 'static,
    ) -> Result<(), OperationError> {
        let mut state = self.state.lock().unwrap();
        if state.closing {
            return Err(OperationError::Closing);
        }
        // Reap finished handles so repeated refreshes do not accumulate resources.
        let mut index = 0;
        while index < state.workers.len() {
            if state.workers[index].is_finished() {
                let _ = state.workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        let cancel = self.cancellation.clone();
        let faulted = self.faulted.clone();
        let worker = thread::spawn(move || {
            let outcome = if cancel.is_cancelled() {
                Err(OperationError::Cancelled)
            } else {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(cancel))).map_err(|_| {
                    faulted.store(true, Ordering::Release);
                    OperationError::Panicked
                })
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| complete(outcome))).is_err() {
                faulted.store(true, Ordering::Release);
            }
        });
        state.workers.push(worker);
        Ok(())
    }

    /// Refuses new work and asks accepted operations to stop. Durable saves use their own draining queue.
    pub fn cancel(&self) {
        self.state.lock().unwrap().closing = true;
        self.cancellation.cancel();
    }

    pub fn faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }

    pub fn close(&self) {
        let _joining = self.joining.lock().unwrap();
        self.cancel();
        let workers = std::mem::take(&mut self.state.lock().unwrap().workers);
        for worker in workers {
            if worker.join().is_err() {
                self.faulted.store(true, Ordering::Release);
            }
        }
    }
}

impl Drop for Operations {
    fn drop(&mut self) {
        self.close();
    }
}
