//! A service on another thread telling a feature that something changed.

use spiderland::Intents;
use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::rc::{Rc, Weak};
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

/// Waits for `work` on the UI thread and hands its result to `then`, whatever else happens. Nothing can stop it, so it suits
/// a result that must always be heard, such as whether a write went through. Work that may be let go of goes through [`local`].
pub fn finish<T: 'static>(work: impl Future<Output = T> + 'static, then: impl FnOnce(T) + 'static) {
    // Fails only where no event loop can ever run, and then nothing could show the result anyway.
    let _ = slint::spawn_local(async move { then(work.await) });
}

/// Stops listening when dropped, so it can be a guard for [`spiderland::Subscriptions`]. That is safe from inside the intent
/// the listener itself sent: nothing is sent after that, and what the listener holds is let go of on the next turn of the
/// event loop, with no need for anything further to be published.
pub struct Listening {
    _work: Dropping,
    _wanted: Rc<()>,
}

impl Listening {
    fn start<F: Future<Output = ()> + 'static>(body: impl FnOnce(Weak<()>) -> F) -> Listening {
        let wanted = Rc::new(());
        Listening {
            _work: local(body(Rc::downgrade(&wanted)), |()| {}),
            _wanted: wanted,
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
    Listening::start(move |wanted| async move {
        loop {
            let ended = updates.changed().await.is_err();
            // The last intent may have let go of this: a value published since doesn't call it back.
            if wanted.strong_count() == 0 {
                break;
            }
            intents.send(make());
            if ended {
                break;
            }
        }
    })
}

/// Sends `make()` on the UI thread each time the returned [`Waker`] fires.
pub fn on_wake<I: 'static>(intents: &Intents<I>, make: impl Fn() -> I + 'static) -> (Waker, Listening) {
    let notify = Arc::new(Notify::new());
    let intents = intents.clone();
    let waiting = notify.clone();
    let listening = Listening::start(move |wanted| async move {
        loop {
            waiting.notified().await;
            if wanted.strong_count() == 0 {
                break;
            }
            intents.send(make());
        }
    });
    (Waker(notify), listening)
}
