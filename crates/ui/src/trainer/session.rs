//! What the trainer's pages ask of the game session: the part of the C# app's `GameSessionCoordinator` (and its
//! `HookValueMemory`) that the page view models use. The app runs on the trainer's worker (`backend_session`) when it has
//! one, and otherwise on [`Unconnected`], which tells the truth: no offline character is connected.

use super::catalog::Feature;
use eg_core::settings_service::ModuleSettings;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
pub const NOT_AVAILABLE: &str = "Not available in this version yet.";

pub trait TrainerSession {
    /// An offline character is connected (the session is Ready or Applying).
    fn is_connected(&self) -> bool;
    /// Why the connected game cannot serve this feature in this version; None when it can, and while nothing is connected
    /// (the row then waits for a connection, as the C# app's does).
    fn unavailable(&self, _id: &str) -> Option<&'static str> {
        None
    }
    /// The hooks the user wants on; each starts off with every run.
    fn is_hook_wanted(&self, id: &str) -> bool;
    fn set_hook(&self, id: &str, on: bool);
    /// A hook's remembered values; empty when it has none.
    fn hook_values(&self, id: &str) -> Vec<f64>;
    fn set_hook_values(&self, id: &str, values: &[f64]);
    /// Where a hook stands in the game, while connected.
    fn hook_state(&self, id: &str) -> Option<HookState>;
    fn read_fields(&self, id: &str) -> FieldRequest;
    fn write_fields(&self, id: &str, fields: &[FieldWrite]) -> FieldRequest;
}

/// The switches the user wants on for this run, and the hooks' values remembered across runs (`HookValueMemory`).
#[derive(Default)]
pub struct Hooks {
    wanted: RefCell<BTreeSet<String>>,
    pub memory: HookMemory,
}

impl Hooks {
    pub fn new(memory: HookMemory) -> Hooks {
        Hooks {
            wanted: RefCell::default(),
            memory,
        }
    }

    pub fn is_wanted(&self, id: &str) -> bool {
        self.wanted.borrow().contains(id)
    }

    pub fn set(&self, id: &str, on: bool) {
        let mut wanted = self.wanted.borrow_mut();
        if on {
            wanted.insert(id.to_owned());
        } else {
            wanted.remove(id);
        }
    }
}

/// The settings key the values are saved under, in `moduleSettings.trainer`.
pub const HOOK_VALUES_KEY: &str = "hookValues";

/// The hooks' values (multipliers, counts, a choice as its index), remembered across runs in `moduleSettings.trainer.hookValues`
/// (spec/data-and-persistence.md, the C# `HookValueMemory`). Values of a feature the catalog no longer has, or that now
/// takes another number of values, are dropped on load and the settings saved once without them. Changes in quick
/// succession are saved once, with the latest values; a failed save is ignored (the value comes back at its default).
#[derive(Default)]
pub struct HookMemory {
    values: Arc<Mutex<BTreeMap<String, Vec<f64>>>>,
    settings: Option<ModuleSettings>,
    /// A save is queued and not yet started: it will take the latest values, so another change needs no save of its own.
    queued: Arc<AtomicBool>,
}

impl HookMemory {
    /// Loads the saved values fitted to `features`, saving once if any were dropped. Without settings, it remembers for
    /// this run only.
    pub fn load(settings: Option<ModuleSettings>, features: &[Feature]) -> HookMemory {
        let saved = settings.as_ref().and_then(ModuleSettings::read).unwrap_or(Value::Null);
        let (values, dropped) = fit_saved(&saved, features);
        let memory = HookMemory {
            values: Arc::new(Mutex::new(values)),
            settings,
            queued: Arc::default(),
        };
        if dropped {
            memory.save();
        }
        memory
    }

    pub fn get(&self, id: &str) -> Vec<f64> {
        self.values.lock().unwrap().get(id).cloned().unwrap_or_default()
    }

    pub fn set(&self, id: &str, values: &[f64]) {
        let changed = self.values.lock().unwrap().insert(id.to_owned(), values.to_vec()).as_deref() != Some(values);
        if changed {
            self.save();
        }
    }

    pub fn snapshot(&self) -> BTreeMap<String, Vec<f64>> {
        self.values.lock().unwrap().clone()
    }

    fn save(&self) {
        let Some(settings) = &self.settings else {
            return;
        };
        if self.queued.swap(true, Ordering::AcqRel) {
            return;
        }
        let (values, queued) = (self.values.clone(), self.queued.clone());
        let result = settings.update(move |fields| {
            // From here a change saves again: this save may already have read the values it would miss.
            queued.store(false, Ordering::Release);
            let values = values.lock().unwrap().clone();
            let aliases: Vec<_> = fields.keys().filter(|name| name.eq_ignore_ascii_case(HOOK_VALUES_KEY)).cloned().collect();
            for alias in aliases {
                fields.remove(&alias);
            }
            fields.insert(HOOK_VALUES_KEY.into(), to_json(&values));
            Ok(())
        });
        if result.is_err() {
            // Settings are closing: best effort, as the C# app's.
            self.queued.store(false, Ordering::Release);
        }
    }
}

/// The saved values the catalog still takes: each feature it has, with as many numbers as the feature has values. True
/// when anything was dropped.
fn fit_saved(slice: &Value, features: &[Feature]) -> (BTreeMap<String, Vec<f64>>, bool) {
    let mut values = BTreeMap::new();
    let mut dropped = false;
    let saved = slice
        .as_object()
        .and_then(|fields| fields.iter().find(|(name, _)| name.eq_ignore_ascii_case(HOOK_VALUES_KEY)))
        .map(|(_, value)| value);
    let Some(saved) = saved else {
        return (values, false);
    };
    let Some(entries) = saved.as_object() else {
        return (values, !saved.is_null());
    };
    for (id, numbers) in entries {
        let numbers: Option<Vec<f64>> = numbers.as_array().and_then(|list| list.iter().map(Value::as_f64).collect());
        let wanted = features.iter().find(|feature| feature.id == id).map(|feature| feature.values.len());
        match (numbers, wanted) {
            (Some(numbers), Some(count)) if count > 0 && numbers.len() == count => {
                values.insert(id.clone(), numbers);
            }
            _ => dropped = true,
        }
    }
    (values, dropped)
}

fn to_json(values: &BTreeMap<String, Vec<f64>>) -> Value {
    let mut map = Map::new();
    for (id, numbers) in values {
        map.insert(id.clone(), Value::Array(numbers.iter().map(|&number| Value::from(number)).collect()));
    }
    Value::Object(map)
}

/// The session without a trainer worker: never connected. It keeps the switches the user turns on and their values, as
/// the C# session does before a connection, so a page shows what it would send.
#[derive(Default)]
pub struct Unconnected {
    hooks: Hooks,
}

impl Unconnected {
    pub fn new(memory: HookMemory) -> Unconnected {
        Unconnected { hooks: Hooks::new(memory) }
    }
}

const NOT_CONNECTED: &str = "No offline character is connected.";

/// The refusal of a session with nothing connected: nothing reached the game.
pub fn not_connected() -> FieldRequest {
    FieldRequest::Done(Err(FieldError {
        details: NOT_CONNECTED.to_owned(),
        nothing_sent: true,
    }))
}

impl TrainerSession for Unconnected {
    fn is_connected(&self) -> bool {
        false
    }

    fn is_hook_wanted(&self, id: &str) -> bool {
        self.hooks.is_wanted(id)
    }

    fn set_hook(&self, id: &str, on: bool) {
        self.hooks.set(id, on);
    }

    fn hook_values(&self, id: &str) -> Vec<f64> {
        self.hooks.memory.get(id)
    }

    fn set_hook_values(&self, id: &str, values: &[f64]) {
        self.hooks.memory.set(id, values);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::catalog;
    use eg_core::settings::SettingsStore;
    use eg_core::settings_service::SettingsService;

    #[test]
    fn saved_values_are_fitted_to_the_catalog() {
        let saved = serde_json::json!({
            "autoConnect": true,
            "hookValues": { "damage-dealt": [3.0], "drop-rates": [0.1, 1.0], "gone": [1.0], "god-mode": [] }
        });
        let (values, dropped) = fit_saved(&saved, catalog::FEATURES);
        assert!(dropped);
        assert_eq!(values.into_iter().collect::<Vec<_>>(), [("damage-dealt".to_owned(), vec![3.0])]);
        let (_, dropped) = fit_saved(&serde_json::json!({ "hookValues": { "damage-dealt": [3.0] } }), catalog::FEATURES);
        assert!(!dropped);
        assert!(!fit_saved(&Value::Null, catalog::FEATURES).1);
    }

    #[test]
    fn values_are_remembered_across_runs() {
        let folder = tempfile::tempdir().unwrap();
        let open = || {
            let store = SettingsStore::open(&folder.path().join("app"), &folder.path().join("settings.json"), &[]);
            SettingsService::inline(store)
        };
        let service = open();
        service
            .module("trainer")
            .update(|fields| {
                fields.insert("hookValues".into(), serde_json::json!({ "damage-dealt": [3.0], "gone": [1.0] }));
                Ok(())
            })
            .unwrap();
        // Loading drops the stale entry and saves once without it.
        let memory = HookMemory::load(Some(service.module("trainer")), catalog::FEATURES);
        assert_eq!(memory.get("damage-dealt"), [3.0]);
        let saved = service.module("trainer").read().unwrap();
        assert_eq!(saved["hookValues"], serde_json::json!({ "damage-dealt": [3.0] }));
        memory.set("move-speed", &[1.5]);
        memory.set("damage-dealt", &[4.0]);
        drop(memory);
        let memory = HookMemory::load(Some(open().module("trainer")), catalog::FEATURES);
        assert_eq!(
            memory.snapshot().into_iter().collect::<Vec<_>>(),
            [("damage-dealt".to_owned(), vec![4.0]), ("move-speed".to_owned(), vec![1.5])]
        );
    }
}
