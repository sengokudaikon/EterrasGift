//! The trainer's pages (spec/trainer.md §5): the catalog of features, the page and row models ported from the C# app's
//! view models, and their binding to `pages/trainer.slint`. The session behind them is not ported: the app shows the
//! truth, that no offline character is connected (`session::Unconnected`). Captures drive connected-looking states
//! through a fake session of their own.
//!
//! TODO(trainer): the catalog and the row models move to the trainer's own crates once the session lands; this module
//! keeps only the binding.

pub mod catalog;
pub mod pages;
pub mod session;
mod view;

pub use view::bind;

use crate::MainWindow;
use std::rc::Rc;

/// Binds the trainer's pages. Navigation offers them only in a run with the trainer on; binding them costs nothing else.
/// TODO(trainer): list planned features (`ported: false`) in a source checkout, once the composition says it is one.
/// Every feature is ported today, so nothing is missing.
pub fn install(window: &MainWindow) {
    bind(window, Rc::new(session::Unconnected::default()), catalog::FEATURES.to_vec(), false);
}
