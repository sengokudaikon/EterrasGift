//! The trainer's pages (spec/trainer.md §5): the catalog of features, the page and row models ported from the C# app's
//! view models, and their binding to `pages/trainer.slint`. With the trainer's worker they run on its snapshots
//! (`backend_session`); without one, on `session::Unconnected`, which tells the truth: no offline character is connected.
//! Captures drive other states through fake sessions of their own.
//!
//! TODO(trainer): the catalog and the row models move to the trainer's own crates; this module keeps only the binding.

pub mod backend_session;
pub mod catalog;
pub mod pages;
pub mod session;
mod view;

pub use view::{Pages, WeakPages, bind};

use crate::MainWindow;
use eg_core::settings_service::ModuleSettings;
use eg_trainer::backend::BackendHandle;
use std::rc::Rc;

/// Binds the trainer's pages, on the worker when there is one. `settings` is the trainer's slice of the settings, where
/// the hooks' values are remembered; None remembers them for this run only. Navigation offers the pages only in a run
/// with the trainer on; binding them costs nothing else.
/// TODO(trainer): list planned features (`presented: false`) in a source checkout, once the composition says it is one.
/// Row presentation and native availability are separate; the backend decides what can run.
pub fn install(window: &MainWindow, backend: Option<BackendHandle>, settings: Option<ModuleSettings>) {
    let memory = session::HookMemory::load(settings, catalog::FEATURES);
    let Some(backend) = backend else {
        bind(window, Rc::new(session::Unconnected::new(memory)), catalog::FEATURES.to_vec(), false);
        return;
    };
    let pages = bind(
        window,
        Rc::new(backend_session::BackendSession::new(backend.clone(), memory)),
        catalog::FEATURES.to_vec(),
        false,
    );
    follow(&pages, &backend);
}

/// Refreshes the pages on every worker snapshot, as the Item Forge follows them; it ends with the worker or when the pages
/// are bound again.
pub fn follow(pages: &Pages, backend: &BackendHandle) {
    let mut updates = backend.subscribe();
    let pages = pages.downgrade();
    let spawned = slint::spawn_local(async move {
        loop {
            let ended = updates.changed().await.is_err();
            let Some(pages) = pages.upgrade() else { break };
            pages.refresh();
            if ended {
                break;
            }
        }
    });
    // Without an event loop there is nothing to show; the pages stay as they are.
    drop(spawned);
}
