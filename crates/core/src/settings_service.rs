//! One owner for settings IO. Callers enqueue changes; reads clone a cached snapshot and never wait for disk.
//! Revisions order observations, not game writes. Failed local persistence remains explicit and retryable.

use crate::json;
use crate::paths::AppPaths;
use crate::settings::{AppSettings, Locations, SettingsStore};
use eg_runtime::{CancellationToken, JoinHandle, Tasks};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

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

struct Control {
    sender: Option<mpsc::UnboundedSender<(u64, Change)>>,
    inline: Option<SettingsStore>,
    closed: bool,
    paused: bool,
}

struct Shared {
    snapshot: watch::Sender<Snapshot>,
    listeners: Mutex<Vec<Box<dyn Fn() + Send>>>,
}

impl Shared {
    /// Wakes listeners once a change has completed.
    fn completed(&self) {
        for listener in self.listeners.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter() {
            listener();
        }
    }
}

struct Owner {
    control: Mutex<Control>,
    shared: Arc<Shared>,
    tasks: Option<Tasks>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct SettingsService(Arc<Owner>);

impl SettingsService {
    /// The writer runs on `tasks`. Dropping every handle lets it finish what was accepted and end; the runtime waits for
    /// that, and its close also ends a writer nobody closed. [`SettingsService::close`] is the barrier to use.
    pub fn new(store: SettingsStore, tasks: &Tasks) -> Self {
        Self::start(store, Some(tasks.clone()))
    }

    /// Deterministic fixture mode; executes exactly the same command path without a worker.
    pub fn inline(store: SettingsStore) -> Self {
        Self::start(store, None)
    }

    fn start(store: SettingsStore, tasks: Option<Tasks>) -> Self {
        let (snapshot, _) = watch::channel(Snapshot {
            submitted: 0,
            completed: 0,
            persisted_revision: 0,
            locations: None,
            pending_location: None,
            outcome: if store.unreadable() { SaveOutcome::MemoryOnly } else { SaveOutcome::Persisted },
            store: SettingsView::from(&store),
        });
        let shared = Arc::new(Shared {
            snapshot,
            listeners: Mutex::default(),
        });
        let (sender, receiver) = mpsc::unbounded_channel();
        // Refused only on a runtime already closing: with the receiver gone, every submit then reports the fault.
        let worker = tasks
            .as_ref()
            .and_then(|tasks| tasks.spawn(serve(shared.clone(), store.clone(), receiver, tasks.token())).ok());
        let inline = tasks.is_none();
        Self(Arc::new(Owner {
            control: Mutex::new(Control {
                sender: (!inline).then_some(sender),
                inline: inline.then_some(store),
                closed: false,
                paused: false,
            }),
            shared,
            tasks,
            worker: Mutex::new(worker),
        }))
    }

    /// No filesystem operations run here in production. Submission order is the mutation order.
    pub fn submit(&self, change: Change) -> Result<u64, String> {
        let mut control = self.0.control.lock().unwrap();
        if control.closed || control.paused {
            return Err("Settings are closing; no new changes were accepted.".into());
        }
        if matches!(self.0.shared.snapshot.borrow().outcome, SaveOutcome::Faulted(_)) {
            return Err("The settings writer stopped after a fault.".into());
        }
        // Only here, under the admission lock, does `submitted` change.
        let mut revision = 0;
        self.0.shared.snapshot.send_modify(|snapshot| {
            snapshot.submitted += 1;
            revision = snapshot.submitted;
        });
        if let Some(store) = &mut control.inline {
            execute(&self.0.shared, store, revision, change);
        } else if control.sender.as_ref().unwrap().send((revision, change)).is_err() {
            self.0.shared.snapshot.send_modify(|snapshot| {
                snapshot.completed = revision;
                snapshot.outcome = SaveOutcome::Faulted("The settings worker stopped.".into());
            });
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

    /// Every change to the snapshot, for a UI to await on its own thread. It needs no runtime to be awaited.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.0.shared.snapshot.subscribe()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.0.shared.snapshot.borrow().clone()
    }

    pub fn is_saving(&self) -> bool {
        self.0.shared.snapshot.borrow().saving()
    }

    /// A nonblocking close gate: once idle, pause admission atomically with the observation.
    /// Call `resume` if the window stays open to show a failed save; otherwise call `close` after the event loop.
    pub fn pause_when_idle(&self) -> bool {
        let mut control = self.0.control.lock().unwrap();
        if self.0.shared.snapshot.borrow().saving() {
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
        let snapshot = self.0.shared.snapshot.borrow();
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
        let Some(tasks) = &self.0.tasks else {
            // Inline work has already completed.
            return self.snapshot();
        };
        let mut snapshots = self.subscribe();
        tasks.block_on(async {
            let snapshot = snapshots.wait_for(|snapshot| snapshot.completed >= revision).await;
            snapshot.expect("the service holds the sender").clone()
        })
    }

    /// Stops admission, drains every accepted save, then joins the worker. Does not silently retry failed writes.
    pub fn close(&self) -> Snapshot {
        {
            let mut control = self.0.control.lock().unwrap();
            control.closed = true;
            control.sender = None;
        }
        if let (Some(worker), Some(tasks)) = (self.0.worker.lock().unwrap().take(), &self.0.tasks) {
            let _ = tasks.block_on(worker);
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
        self.service.0.shared.snapshot.borrow().store.desired().module_settings.get(&self.id).cloned()
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
            // Every spelling goes, so the one written is the only one read.
            while json::take(fields, key).is_some() {}
            fields.insert(key.into(), Value::Bool(value));
            Ok(())
        })
    }
}

/// The one writer: saves in submission order, each on the blocking pool, and everything accepted is saved before it ends.
/// It ends when every sender is gone, or when the runtime closes around a service nobody closed.
async fn serve(shared: Arc<Shared>, store: SettingsStore, mut commands: mpsc::UnboundedReceiver<(u64, Change)>, closing: CancellationToken) {
    let mut store = Some(store);
    while let Some((revision, change)) = next(&mut commands, &closing).await {
        let Some(mut owned) = store.take() else {
            shared.snapshot.send_modify(|snapshot| snapshot.completed = revision);
            shared.completed();
            continue;
        };
        let writer = shared.clone();
        let saved = tokio::task::spawn_blocking(move || {
            execute(&writer, &mut owned, revision, change);
            owned
        })
        .await;
        match saved {
            Ok(owned) => store = Some(owned),
            // Only a listener can get here: `execute` turns a panic in the change itself into its outcome.
            Err(_) => shared.snapshot.send_modify(|snapshot| {
                snapshot.completed = revision;
                snapshot.outcome = SaveOutcome::Faulted("The settings worker panicked; no further changes will be written.".into());
            }),
        }
    }
}

async fn next(commands: &mut mpsc::UnboundedReceiver<(u64, Change)>, closing: &CancellationToken) -> Option<(u64, Change)> {
    if !commands.is_closed() {
        if let Some(next) = closing.run_until_cancelled(commands.recv()).await {
            return next;
        }
        commands.close();
    }
    commands.recv().await
}

fn execute(shared: &Shared, store: &mut SettingsStore, revision: u64, change: Change) {
    // After a panic no further mutations run. Queued commands still receive terminal observations for draining.
    if matches!(shared.snapshot.borrow().outcome, SaveOutcome::Faulted(_)) {
        shared.snapshot.send_modify(|snapshot| snapshot.completed = revision);
        shared.completed();
        return;
    }
    let locations = matches!(&change, Change::Locations(_));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let desired = store.plan(|candidate| apply(candidate, change))?;
        store.stage(desired, locations);
        // Immediate module choices become observable before disk IO completes. Persisted/active state stays separate.
        shared.snapshot.send_modify(|snapshot| {
            snapshot.store = SettingsView::from(&*store);
            if locations {
                snapshot.pending_location = Some(revision);
            }
        });
        store.persist(locations).map_err(|error| error.to_string())
    }));
    let outcome = match result {
        Ok(Ok(())) if store.unreadable() => SaveOutcome::MemoryOnly,
        Ok(Ok(())) => SaveOutcome::Persisted,
        Ok(Err(error)) => SaveOutcome::Failed(error),
        Err(_) => SaveOutcome::Faulted("The settings worker panicked; no further changes will be written.".into()),
    };
    shared.snapshot.send_modify(|snapshot| {
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
    });
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
