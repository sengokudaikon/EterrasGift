//! A service on another thread telling a feature that something changed.

use flow::Intents;
use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::Poll;
use tokio::sync::{Notify, oneshot, watch};

/// Fires from any thread. A burst of fires makes at most two intents, the one under way and one more, however long it is.
/// Each intent should read the latest state, so nothing is lost and nothing piles up.
#[derive(Clone)]
pub struct Waker(Arc<Notify>);

impl Waker {
    pub fn wake(&self) {
        self.0.notify_one();
    }
}

/// Drops the work given to [`local`] when dropped, if it hasn't finished.
pub struct Dropping(oneshot::Sender<()>);

impl Dropping {
    pub fn is_finished(&self) -> bool {
        self.0.is_closed()
    }
}

/// Waits for `work` on the UI thread, then hands its result to `then`. Dropping the returned guard drops the work instead.
/// Unlike aborting a task, that is safe at any moment, including from inside `then`: a store's reducer runs there, and what it
/// asks for may be to let go of this very work.
pub fn local<T: 'static>(work: impl Future<Output = T> + 'static, then: impl FnOnce(T) + 'static) -> Dropping {
    let (guard, mut dropped) = oneshot::channel::<()>();
    // Fails only where no event loop can ever run, and then nothing could show the result anyway.
    let _ = slint::spawn_local(async move {
        let result = {
            let mut work = pin!(work);
            poll_fn(|context| {
                if Pin::new(&mut dropped).poll(context).is_ready() {
                    return Poll::Ready(None);
                }
                work.as_mut().poll(context).map(Some)
            })
            .await
        };
        drop(dropped);
        if let Some(result) = result {
            then(result);
        }
    });
    Dropping(guard)
}

/// Stops listening when dropped, so it can be a guard for [`flow::Subscriptions`].
pub struct Listening(Option<slint::JoinHandle<()>>);

impl Drop for Listening {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

/// Sends `make()` on the UI thread each time a worker publishes on `updates`, and once more when it ends, so a feature can
/// tell. Stops when the returned guard is dropped.
pub fn on_watch<T, I>(mut updates: watch::Receiver<T>, intents: &Intents<I>, make: impl Fn() -> I + 'static) -> Listening
where
    T: 'static,
    I: 'static,
{
    let intents = intents.clone();
    // Fails only where no event loop can ever run, and then nothing could show the result anyway.
    let task = slint::spawn_local(async move {
        loop {
            let ended = updates.changed().await.is_err();
            intents.send(make());
            if ended {
                break;
            }
        }
    })
    .ok();
    Listening(task)
}

/// Sends `make()` on the UI thread each time the returned [`Waker`] fires.
pub fn on_wake<I: 'static>(intents: &Intents<I>, make: impl Fn() -> I + 'static) -> (Waker, Listening) {
    let notify = Arc::new(Notify::new());
    let intents = intents.clone();
    let waiting = notify.clone();
    // Fails only where no event loop can ever run, and then nothing could show the result anyway.
    let task = slint::spawn_local(async move {
        loop {
            waiting.notified().await;
            intents.send(make());
        }
    })
    .ok();
    (Waker(notify), Listening(task))
}
