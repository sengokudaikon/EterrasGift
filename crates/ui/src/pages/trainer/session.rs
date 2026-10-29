//! What the trainer's pages ask of the game session: its connection, the hooks of the shown profile and where they
//! stand in the game, and the field editors' reads and writes. The app runs on the trainer's worker (`backend_session`)
//! when it has one, and otherwise on [`Unconnected`], which tells the truth: no offline character is connected, and no
//! profile is known.

use super::profiles::Profile;
use eg_core::phrase::Phrase;

/// Where a hook of the live profile stands in the game: the worker's own report.
pub use eg_trainer::backend::HookRun as HookState;

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

pub type FieldResult = Result<Vec<GameField>, FieldError>;

/// A read or write the game answers later. The pages look again whenever the session changes, and while a request is
/// unanswered; `poll` never waits.
pub trait PendingFields {
    fn poll(&mut self) -> Option<FieldResult>;
}

/// A read or write as the session took it: answered at once, or on its way to the game.
pub enum FieldRequest {
    Done(FieldResult),
    Pending(Box<dyn PendingFields>),
}

impl From<FieldResult> for FieldRequest {
    fn from(result: FieldResult) -> FieldRequest {
        FieldRequest::Done(result)
    }
}

/// What a connected row says when this version cannot serve its feature yet.
pub const NOT_AVAILABLE: Phrase = Phrase::new("Not available in this version yet.");

pub trait TrainerSession {
    /// An offline character is connected (the session is Ready or Applying).
    fn is_connected(&self) -> bool;
    /// Why the connected game cannot serve this feature in this version; None when it can, and while nothing is connected
    /// (the row then waits for a connection).
    fn unavailable(&self, _id: &str) -> Option<&'static str> {
        None
    }
    /// Every kept profile, the last played first.
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    /// Whose hooks the rows are: the profile chosen while no game runs, or the character in play. None while no game runs
    /// and no profile is kept.
    fn shown_profile(&self) -> Option<Profile> {
        None
    }
    /// The shown hooks are a kept profile's; false for a character in play without one, whose hooks last this connection.
    fn is_profile_kept(&self) -> bool {
        true
    }
    /// Shows another profile; only while no game runs.
    fn select_profile(&self, _profile: &Profile) {}
    /// Keeps the hooks of the character in play, which has no profile, as its profile.
    fn keep_profile(&self) {}
    /// While no game runs, or the profile of the character in play, whose hooks then last this connection only.
    fn delete_profile(&self, _profile: &Profile) {}
    /// The hooks the shown profile wants on. Changing them changes the profile, and while it is in play the game too.
    fn is_hook_wanted(&self, id: &str) -> bool;
    fn set_hook(&self, id: &str, on: bool);
    /// A hook's values in the shown profile; empty when it has none.
    fn hook_values(&self, id: &str) -> Vec<f64>;
    fn set_hook_values(&self, id: &str, values: &[f64]);
    /// Where a hook stands in the game, while its character is in play.
    fn hook_state(&self, id: &str) -> Option<HookState>;
    /// A change is on its way and not yet taken: the pages look again until it is.
    fn is_busy(&self) -> bool {
        false
    }
    fn read_fields(&self, id: &str) -> FieldRequest;
    fn write_fields(&self, id: &str, fields: &[FieldWrite]) -> FieldRequest;
}

/// The session without a trainer worker: never connected, and without profiles, so no hook can be set up.
#[derive(Default)]
pub struct Unconnected;

const NOT_CONNECTED: Phrase = Phrase::new("No offline character is connected.");

/// The refusal of a session with nothing connected: nothing reached the game.
pub fn not_connected() -> FieldRequest {
    FieldRequest::Done(Err(FieldError {
        details: NOT_CONNECTED.into(),
        nothing_sent: true,
    }))
}

impl TrainerSession for Unconnected {
    fn is_connected(&self) -> bool {
        false
    }

    fn is_hook_wanted(&self, _id: &str) -> bool {
        false
    }

    fn set_hook(&self, _id: &str, _on: bool) {}

    fn hook_values(&self, _id: &str) -> Vec<f64> {
        Vec::new()
    }

    fn set_hook_values(&self, _id: &str, _values: &[f64]) {}

    fn hook_state(&self, _id: &str) -> Option<HookState> {
        None
    }

    fn read_fields(&self, _id: &str) -> FieldRequest {
        not_connected()
    }

    fn write_fields(&self, _id: &str, _fields: &[FieldWrite]) -> FieldRequest {
        not_connected()
    }
}
