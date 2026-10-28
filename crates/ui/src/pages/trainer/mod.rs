//! The trainer's pages: the catalog of features, the page and row models, and their binding to
//! `pages/trainer.slint`. With the trainer's worker they run on its snapshots
//! (`backend_session`); without one, on `session::Unconnected`, which tells the truth: no offline character is connected.
//! Captures drive other states through fake sessions of their own. The catalog is `eg_trainer::features`, which `catalog`
//! adapts.
//!
//! The hooks' wanted state and values are the worker's profiles (`eg_trainer::features::profiles`); this module keeps only the rows
//! and their binding.

pub mod backend_session;
pub mod catalog;
pub mod connection;
pub mod pages;
pub mod profiles;
pub mod session;
mod view;

pub use view::{Pages, WeakPages, bind};

use crate::MainWindow;
use eg_trainer::backend::BackendHandle;
use std::rc::Rc;

/// Binds the trainer's pages, on the worker when there is one; the worker keeps the profiles. Navigation offers the pages
/// only in a run with the trainer on; binding them costs nothing else.
/// TODO(trainer): list planned features (`presented: false`) in a source checkout, once the composition says it is one.
/// Row presentation and native availability are separate; the backend decides what can run.
pub fn install(window: &MainWindow, backend: Option<BackendHandle>) {
    let Some(backend) = backend else {
        bind(window, Rc::new(session::Unconnected), catalog::FEATURES.to_vec(), false);
        return;
    };
    let pages = bind(window, Rc::new(backend_session::BackendSession::new(backend.clone())), catalog::FEATURES.to_vec(), false).downgrade();
    follow(&backend, move || pages.upgrade().map(|pages| pages.refresh()).is_some());
}

/// Calls `changed` on every worker snapshot until the worker ends or `changed` says its owner is gone. Without an event
/// loop there is nothing to show, and nothing follows.
pub(crate) fn follow(backend: &BackendHandle, mut changed: impl FnMut() -> bool + 'static) {
    let mut updates = backend.subscribe();
    let spawned = slint::spawn_local(async move {
        loop {
            let ended = updates.changed().await.is_err();
            if !changed() || ended {
                break;
            }
        }
    });
    drop(spawned);
}
