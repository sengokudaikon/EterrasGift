//! One-way UI state, with no UI toolkit in it.
//!
//! A [`Feature`] is pure: `reduce` takes an intent, changes the state and lists effects as plain data. A [`Host`] is the
//! impure half: it renders the state and performs the effects. A [`Store`] runs one against the other, on one thread.
//!
//! ```text
//! view callback ─┐
//! finished work ─┼─ Intent ─► reduce ─► State ─► Host::render ─► view
//! timer tick ────┘               └────► Effect ─► Host::perform ─► finished work
//! ```
//!
//! What this leaves out on purpose:
//! - **Outside state as a separate input.** A worker's snapshot reaches a feature as an intent and is kept in its state, so
//!   a render reads one consistent snapshot rather than asking a live handle row by row.
//! - **Whole-view recomputes.** [`Tracked`] and [`Slice`] let a render redo only the parts whose inputs changed.
//! - **Scope machinery.** A scope is a state inside a state: a parent's `reduce` hands the child its state, intent and
//!   [`Effects::scoped`]. A scope that ends is an `Option` going back to `None`.

mod store;
mod subscriptions;
mod tracked;

pub use store::{Effects, Feature, Host, Intents, Panicked, Store, step};
pub use subscriptions::Subscriptions;
pub use tracked::{CheckedSlice, Slice, Tracked, Version};
