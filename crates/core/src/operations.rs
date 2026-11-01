//! Cooperative cancellation. It never claims an already started write was undone; its owner must inspect the actual result.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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
