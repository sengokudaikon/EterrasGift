//! One owner for settings IO. Callers enqueue changes; reads clone a cached snapshot and never wait for disk.
//! Revisions order observations, not game writes. Failed local persistence remains explicit and retryable.

use crate::paths::AppPaths;
use crate::settings::{AppSettings, Locations, SettingsStore};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};

type Patch = Box<dyn FnOnce(&mut Map<String, Value>) -> Result<(), String> + Send>;

pub enum Change {
    ModuleEnabled { id: String, enabled: bool, default: bool },
    Module { id: String, patch: Patch },
    Locations(Locations),
    Retry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Persisted,
    MemoryOnly,
    Failed(String),
    Faulted(String),
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Last accepted command and last finished command. A gap means work is queued or in flight.
    pub submitted: u64,
    pub completed: u64,
    pub persisted_revision: u64,
    pub outcome: SaveOutcome,
    /// Location acknowledgement is retained independently of later switch saves.
    pub locations: Option<(u64, Result<AppSettings, String>)>,
    pub pending_location: Option<u64>,
    /// An immutable view of saved, desired and startup state. Never the worker's mutable store.
    pub store: SettingsView,
}

/// Cached data only: a snapshot cannot bypass the writer to persist a stale document.
#[derive(Clone, Debug)]
pub struct SettingsView {
    saved: AppSettings,
    desired: AppSettings,
    paths: AppPaths,
    active_game_executable: Option<PathBuf>,
    active_melon_loader_directory: Option<PathBuf>,
    startup_modules: BTreeMap<String, bool>,
    startup_error: Option<String>,
    unreadable: bool,
    pending: bool,
}

impl SettingsView {
    pub fn saved(&self) -> &AppSettings {
        &self.saved
    }
    pub fn desired(&self) -> &AppSettings {
        &self.desired
    }
    pub fn persisted(&self) -> Option<&AppSettings> {
        (!self.unreadable).then_some(&self.saved)
    }
    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }
    pub fn active_game_executable(&self) -> Option<&Path> {
        self.active_game_executable.as_deref()
    }
    pub fn active_melon_loader_directory(&self) -> Option<&Path> {
        self.active_melon_loader_directory.as_deref()
    }
    pub fn startup_modules(&self) -> &BTreeMap<String, bool> {
        &self.startup_modules
    }
    pub fn startup_error(&self) -> Option<&str> {
        self.startup_error.as_deref()
    }
    pub fn unreadable(&self) -> bool {
        self.unreadable
    }
    pub fn has_pending_write(&self) -> bool {
        self.pending
    }
}

impl From<&SettingsStore> for SettingsView {
    fn from(store: &SettingsStore) -> Self {
        Self {
            saved: store.saved().clone(),
            desired: store.desired().clone(),
            paths: store.paths().clone(),
            active_game_executable: store.active_game_executable().map(Path::to_path_buf),
            active_melon_loader_directory: store.active_melon_loader_directory().map(Path::to_path_buf),
            startup_modules: store.startup_modules().clone(),
            startup_error: store.startup_error().map(str::to_owned),
            unreadable: store.unreadable(),
            pending: store.has_pending_write(),
        }
    }
}

impl Snapshot {
    pub fn saving(&self) -> bool {
        self.completed < self.submitted
    }
}

enum Command {
    Save(u64, Change),
    Stop,
}

struct Control {
    sender: Option<mpsc::Sender<Command>>,
    inline: Option<SettingsStore>,
    closed: bool,
    paused: bool,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    completed: Condvar,
    listeners: Mutex<Vec<Box<dyn Fn() + Send>>>,
}

impl Shared {
    /// Wakes waiters and listeners once a change has completed. Call it with the snapshot unlocked.
    fn completed(&self) {
        self.completed.notify_all();
        for listener in self.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter() {
            listener();
        }
    }
}

struct Owner {
    control: Mutex<Control>,
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(sender) = self.control.get_mut().unwrap().sender.take() {
            let _ = sender.send(Command::Stop);
        }
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub struct SettingsService(Arc<Owner>);

impl SettingsService {
    pub fn new(store: SettingsStore) -> Self {
        Self::start(store, false)
    }

    /// Deterministic fixture mode; executes exactly the same command path without a worker.
    pub fn inline(store: SettingsStore) -> Self {
        Self::start(store, true)
    }

    fn start(store: SettingsStore, inline: bool) -> Self {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                submitted: 0,
                completed: 0,
                persisted_revision: 0,
                locations: None,
                pending_location: None,
                outcome: if store.unreadable() { SaveOutcome::MemoryOnly } else { SaveOutcome::Persisted },
                store: SettingsView::from(&store),
            }),
            completed: Condvar::new(),
            listeners: Mutex::default(),
        });
        let (sender, receiver) = mpsc::channel();
        let worker = if inline {
            None
        } else {
            let shared = shared.clone();
            let mut store = store.clone();
            Some(thread::spawn(move || {
                while let Ok(Command::Save(revision, change)) = receiver.recv() {
                    execute(&shared, &mut store, revision, change);
                }
            }))
        };
        Self(Arc::new(Owner {
            control: Mutex::new(Control {
                sender: (!inline).then_some(sender),
                inline: inline.then_some(store),
                closed: false,
                paused: false,
            }),
            shared,
            worker: Mutex::new(worker),
        }))
    }

    /// No filesystem operations run here in production. Submission order is the mutation order.
    pub fn submit(&self, change: Change) -> Result<u64, String> {
        let mut control = self.0.control.lock().unwrap();
        if control.closed || control.paused {
            return Err("Settings are closing; no new changes were accepted.".into());
        }
        let revision = {
            let mut snapshot = self.0.shared.snapshot.lock().unwrap();
            if matches!(snapshot.outcome, SaveOutcome::Faulted(_)) {
                return Err("The settings writer stopped after a fault.".into());
            }
            snapshot.submitted += 1;
            snapshot.submitted
        };
        if let Some(store) = &mut control.inline {
            execute(&self.0.shared, store, revision, change);
        } else if control.sender.as_ref().unwrap().send(Command::Save(revision, change)).is_err() {
            {
                let mut snapshot = self.0.shared.snapshot.lock().unwrap();
                snapshot.completed = revision;
                snapshot.outcome = SaveOutcome::Faulted("The settings worker stopped.".into());
            }
            self.0.shared.completed();
        }
        Ok(revision)
    }

    /// Calls `listener` after each change completes, on the thread that completed it: the writer's, or the submitter's
    /// for an inline service, while it holds the admission lock. It must not call back into the service; a UI wakes
    /// itself and reads the snapshot there (instead of polling for completions).
    pub fn on_completed(&self, listener: impl Fn() + Send + 'static) {
        self.0.shared.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(Box::new(listener));
    }

    pub fn snapshot(&self) -> Snapshot {
        self.0.shared.snapshot.lock().unwrap().clone()
    }

    pub fn is_saving(&self) -> bool {
        self.0.shared.snapshot.lock().unwrap().saving()
    }

    /// A nonblocking close gate: once idle, pause admission atomically with the observation.
    /// Call `resume` if the window stays open to show a failed save; otherwise call `close` after the event loop.
    pub fn pause_when_idle(&self) -> bool {
        let mut control = self.0.control.lock().unwrap();
        if self.0.shared.snapshot.lock().unwrap().saving() {
            return false;
        }
        control.paused = true;
        true
    }

    pub fn resume(&self) {
        self.0.control.lock().unwrap().paused = false;
    }

    /// Cheap polling while idle: no JSON or path copies unless a command has finished.
    pub fn snapshot_after(&self, completed: u64) -> Option<Snapshot> {
        let snapshot = self.0.shared.snapshot.lock().unwrap();
        (snapshot.completed > completed).then(|| snapshot.clone())
    }

    pub fn module(&self, id: impl Into<String>) -> ModuleSettings {
        ModuleSettings {
            service: self.clone(),
            id: id.into(),
        }
    }

    /// A barrier for tests and shutdown; do not call from an interactive UI callback.
    pub fn wait(&self, revision: u64) -> Snapshot {
        let snapshot = self.0.shared.snapshot.lock().unwrap();
        self.0.shared.completed.wait_while(snapshot, |snapshot| snapshot.completed < revision).unwrap().clone()
    }

    /// Stops admission, drains every accepted save, then joins the worker. Does not silently retry failed writes.
    pub fn close(&self) -> Snapshot {
        {
            let mut control = self.0.control.lock().unwrap();
            control.closed = true;
            if let Some(sender) = control.sender.take() {
                let _ = sender.send(Command::Stop);
            }
        }
        if let Some(worker) = self.0.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        self.snapshot()
    }
}

#[derive(Clone)]
pub struct ModuleSettings {
    service: SettingsService,
    id: String,
}

impl ModuleSettings {
    pub fn read(&self) -> Option<Value> {
        self.service.0.shared.snapshot.lock().unwrap().store.desired().module_settings.get(&self.id).cloned()
    }

    /// The module validates its own values, on the latest slice when the command reaches the writer.
    /// Unknown fields remain unless the module explicitly removes them.
    pub fn update(&self, patch: impl FnOnce(&mut Map<String, Value>) -> Result<(), String> + Send + 'static) -> Result<u64, String> {
        self.service.submit(Change::Module {
            id: self.id.clone(),
            patch: Box::new(patch),
        })
    }

    pub fn set_bool(&self, key: &'static str, value: bool) -> Result<u64, String> {
        self.update(move |fields| {
            let aliases: Vec<_> = fields.keys().filter(|name| name.eq_ignore_ascii_case(key)).cloned().collect();
            for alias in aliases {
                fields.remove(&alias);
            }
            fields.insert(key.into(), Value::Bool(value));
            Ok(())
        })
    }
}

fn execute(shared: &Shared, store: &mut SettingsStore, revision: u64, change: Change) {
    // After a panic no further mutations run. Queued commands still receive terminal observations for draining.
    if matches!(shared.snapshot.lock().unwrap().outcome, SaveOutcome::Faulted(_)) {
        shared.snapshot.lock().unwrap().completed = revision;
        shared.completed();
        return;
    }
    let locations = matches!(&change, Change::Locations(_));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let desired = store.plan(|candidate| apply(candidate, change))?;
        store.stage(desired, locations);
        // Immediate module choices become observable before disk IO completes. Persisted/active state stays separate.
        {
            let mut snapshot = shared.snapshot.lock().unwrap();
            snapshot.store = SettingsView::from(&*store);
            if locations {
                snapshot.pending_location = Some(revision);
            }
        }
        store.persist(locations).map_err(|error| error.to_string())
    }));
    let outcome = match result {
        Ok(Ok(())) if store.unreadable() => SaveOutcome::MemoryOnly,
        Ok(Ok(())) => SaveOutcome::Persisted,
        Ok(Err(error)) => SaveOutcome::Failed(error),
        Err(_) => SaveOutcome::Faulted("The settings worker panicked; no further changes will be written.".into()),
    };
    let mut snapshot = shared.snapshot.lock().unwrap();
    snapshot.completed = revision;
    if outcome == SaveOutcome::Persisted {
        snapshot.persisted_revision = revision;
        if let Some(pending) = snapshot.pending_location.take() {
            snapshot.locations = Some((pending, Ok(store.saved().clone())));
        }
    }
    if locations {
        snapshot.locations = Some((
            revision,
            match &outcome {
                SaveOutcome::Persisted => Ok(store.saved().clone()),
                SaveOutcome::Failed(error) | SaveOutcome::Faulted(error) => Err(error.clone()),
                SaveOutcome::MemoryOnly => unreachable!("explicit location save writes or fails"),
            },
        ));
    }
    snapshot.outcome = outcome;
    snapshot.store = SettingsView::from(&*store);
    drop(snapshot);
    shared.completed();
}

fn apply(store: &mut SettingsStore, change: Change) -> Result<(), String> {
    match change {
        Change::ModuleEnabled { id, enabled, default } => store
            .save_switches(|settings: &mut AppSettings| {
                if enabled == default {
                    settings.modules.remove(&id);
                } else {
                    settings.modules.insert(id, enabled);
                }
            })
            .map_err(|error| error.to_string()),
        Change::Module { id, patch } => {
            let mut fields = match store.desired().module_settings.get(&id) {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(fields)) => fields.clone(),
                Some(_) => return Err(format!("The settings for {id} must be an object; they were left unchanged.")),
            };
            patch(&mut fields)?;
            store.save_module_settings(&id, Value::Object(fields)).map_err(|error| error.to_string())
        }
        Change::Locations(locations) => store.save_locations(&locations),
        Change::Retry => store.retry().map_err(|error| error.to_string()),
    }
}
