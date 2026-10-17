//! What the trainer's pages ask of the game session: the part of the C# app's `GameSessionCoordinator` (and its
//! `HookValueMemory`) that the page view models use. The session itself is not ported, so the app runs on [`Unconnected`],
//! which tells the truth: no offline character is connected. TODO(trainer): the real session behind this trait.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

/// Where a hook stands in the game, as the helper reported it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookState {
    pub on: bool,
    pub failure: Option<String>,
}

/// A field as the game answered: unnamed for a fixed editor, named by the game for a listed one (what the character holds).
#[derive(Clone, Debug, PartialEq)]
pub struct GameField {
    pub label: String,
    pub value: f64,
}

impl GameField {
    pub fn new(label: impl Into<String>, value: f64) -> GameField {
        GameField { label: label.into(), value }
    }
}

/// One field of a write: the value it was read with, the value wanted, and its name for the game's messages (a listed
/// field is picked by it).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldWrite {
    pub read: f64,
    pub wanted: f64,
    pub label: String,
}

impl FieldWrite {
    /// Exact: the game hands back the double it was sent, and any other number is a change.
    #[allow(clippy::float_cmp)]
    pub fn changes(&self) -> bool {
        self.wanted != self.read
    }
}

/// A request the session could not complete. `nothing_sent`: it was refused before anything reached the game.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldError {
    pub details: String,
    pub nothing_sent: bool,
}

pub trait TrainerSession {
    /// An offline character is connected (the session is Ready or Applying).
    fn is_connected(&self) -> bool;
    /// The hooks the user wants on; each starts off with every run.
    fn is_hook_wanted(&self, id: &str) -> bool;
    fn set_hook(&self, id: &str, on: bool);
    /// A hook's remembered values; empty when it has none.
    fn hook_values(&self, id: &str) -> Vec<f64>;
    fn set_hook_values(&self, id: &str, values: &[f64]);
    /// Where a hook stands in the game, while connected.
    fn hook_state(&self, id: &str) -> Option<HookState>;
    fn read_fields(&self, id: &str) -> Result<Vec<GameField>, FieldError>;
    fn write_fields(&self, id: &str, fields: &[FieldWrite]) -> Result<Vec<GameField>, FieldError>;
}

/// The session as this build has it: never connected. It keeps the switches the user turns on and their values for the
/// run, as the C# session does before a connection, so a page shows what it would send.
/// TODO(trainer): remembered values are saved in `moduleSettings.trainer.hookValues` (`HookValueMemory`).
#[derive(Default)]
pub struct Unconnected {
    wanted: RefCell<BTreeSet<String>>,
    values: RefCell<BTreeMap<String, Vec<f64>>>,
}

const NOT_CONNECTED: &str = "No offline character is connected.";

impl TrainerSession for Unconnected {
    fn is_connected(&self) -> bool {
        false
    }

    fn is_hook_wanted(&self, id: &str) -> bool {
        self.wanted.borrow().contains(id)
    }

    fn set_hook(&self, id: &str, on: bool) {
        let mut wanted = self.wanted.borrow_mut();
        if on {
            wanted.insert(id.to_owned());
        } else {
            wanted.remove(id);
        }
    }

    fn hook_values(&self, id: &str) -> Vec<f64> {
        self.values.borrow().get(id).cloned().unwrap_or_default()
    }

    fn set_hook_values(&self, id: &str, values: &[f64]) {
        self.values.borrow_mut().insert(id.to_owned(), values.to_vec());
    }

    fn hook_state(&self, _id: &str) -> Option<HookState> {
        None
    }

    fn read_fields(&self, _id: &str) -> Result<Vec<GameField>, FieldError> {
        Err(FieldError {
            details: NOT_CONNECTED.to_owned(),
            nothing_sent: true,
        })
    }

    fn write_fields(&self, _id: &str, _fields: &[FieldWrite]) -> Result<Vec<GameField>, FieldError> {
        Err(FieldError {
            details: NOT_CONNECTED.to_owned(),
            nothing_sent: true,
        })
    }
}
