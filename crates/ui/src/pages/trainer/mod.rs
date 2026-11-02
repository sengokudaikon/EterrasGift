//! The trainer's pages in the window: their binding to `pages/trainer.slint`. What they show and how they answer is
//! `eg_trainer::screens`. With the trainer's worker they run on its snapshots; without one, on `Unconnected`, which tells
//! the truth: no offline character is connected. Captures drive other states through fake sessions of their own.

pub mod connection;
mod view;

pub use view::{Pages, bind};

use crate::MainWindow;
use eg_trainer::backend::BackendHandle;
use eg_trainer::screens::backend_session::BackendSession;
use eg_trainer::screens::registry;
use eg_trainer::screens::session::Unconnected;
use spiderland_slint::Background;
use std::rc::Rc;

/// Binds the trainer's pages, on the worker when there is one; the worker keeps the profiles. Navigation offers the pages
/// only in a run with the trainer on; binding them costs nothing else. `planned` lists features with no row yet (a source
/// checkout). Row presentation and native availability are separate; the backend decides what can run.
pub fn install(window: &MainWindow, backend: Option<BackendHandle>, planned: bool, background: Background) {
    match backend {
        Some(backend) => {
            bind(
                window,
                Rc::new(BackendSession::new(backend.clone())),
                registry::FEATURES.to_vec(),
                planned,
                Some(backend),
                background,
            );
        }
        None => {
            bind(window, Rc::new(Unconnected), registry::FEATURES.to_vec(), planned, None, background);
        }
    }
}
