//! What a Slint [`spiderland::Host`] needs to do its work, built on Slint's event loop and `task_runtime`.
//!
//! - [`Background`]: work off the UI thread whose result comes back as an intent.
//! - [`every`]: a repeating intent, whose timer is a guard for [`spiderland::Subscriptions`].
//! - [`on_wake`] and [`on_watch`]: a service on another thread telling a feature that something changed.
//! - [`local`]: waiting on the UI thread for work that can be let go of, even from what its own result causes.
//! - [`Rows`]: a list model updated row by row, so a row being dragged or typed in survives its neighbours' changes.

mod background;
mod rows;
mod wake;

pub use background::Background;
pub use rows::Rows;
pub use wake::{Dropping, Listening, Waker, local, on_wake, on_watch};

use slint::{Timer, TimerMode};
use spiderland::Intents;
use std::time::Duration;

/// Sends `make()` every `interval` until the returned timer is dropped.
pub fn every<I: 'static>(interval: Duration, intents: &Intents<I>, make: impl Fn() -> I + 'static) -> Timer {
    let timer = Timer::default();
    let intents = intents.clone();
    timer.start(TimerMode::Repeated, interval, move || intents.send(make()));
    timer
}
