//! Application-owned, serialized trainer service with explicit write admission.
//!
//! Construction does not spawn, discover, or access files/processes. The owner
//! explicitly runs the worker on a background Tokio runtime. All accepted work
//! runs to completion; dropping an ordinary receipt is not cancellation. Optional
//! mapping reads have a separate disposable receipt and stop between RPCs. Cleanup barriers
//! suppress publication from an earlier operation, then drain it before cleanup.
//! The watch snapshot is the worker's one report of its state.
mod connection;
mod faults;
mod hooks;
mod mapping;
mod new_affixes;
mod profiles;
mod snapshot;
mod unique;
mod write;

pub use connection::*;
pub use snapshot::*;

use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

pub use crate::session::{ReadControl, ReadObserver};
use eg_core::game_log::GameReadiness;
pub use eg_trainer_wire::transport::Delivery;
pub use eg_trainer_wire::{
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    hooks::GameplayHook,
    mapping::ModifierSnapshot,
    process::ProcessIdentity,
    protocol::RuntimeStatus,
};
use eg_trainer_wire::{
    protocol::ResponseCode,
    transport::{Transport, TransportError, TransportFailure},
};
pub use faults::forge_record;
pub use hooks::{HOOK_NOT_AVAILABLE, HookRequest, HookRun};
use jiff::Timestamp;
pub use profiles::ProfileLock;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    connect::gate::{self, GateDecision, GateVerdict, LogSource},
    definitions::catalog::{GameCatalog, ResolvedItemName},
    definitions::service::{GameData, GameDataHandle},
    features::profiles::{ProfileBook, ProfileKey, ProfileSaver},
    features::{FeatureIntents, HookId},
    item::values::{MappingRequest, ValueTable},
    session::{Cleanup, Session, SessionError},
    write::{ForgePlan, WriteControl, WriteOutcome, WriteRecord, WriteStatus},
};

fn superseded() -> BackendError {
    BackendError::blocked(ErrorKind::Superseded, "operation superseded by queued cleanup")
}
fn released() -> BackendError {
    BackendError::blocked(ErrorKind::NotConnected, "connection released")
}
/// The game's files changed after it started: it may run an older build than they hold.
fn files_changed() -> BackendError {
    BackendError::blocked(
        ErrorKind::Identity,
        "Last Epoch's files changed after it started (a game update?). Restart the game to use the trainer.",
    )
}
/// The same item, since changed: writes keep the game's object and its individual ID.
fn same_item(old: &ForgeSnapshot, new: &ForgeSnapshot) -> bool {
    old.item == new.item && old.individual_id == new.individual_id
}
/// Whether the game would describe the item's unique lines the same: the same base, rarity, unique and rolls.
fn same_unique_text(old: &ForgeSnapshot, new: &ForgeSnapshot) -> bool {
    same_item(old, new) && old.item_type == new.item_type && old.sub_type == new.sub_type && old.rarity == new.rarity && old.identity == new.identity
}
/// Whether the game would describe the item's implicits the same: the same base, rarity, unique and implicit rolls.
fn same_implicit_text(old: &ForgeSnapshot, new: &ForgeSnapshot) -> bool {
    same_item(old, new)
        && old.item_type == new.item_type
        && old.sub_type == new.sub_type
        && old.rarity == new.rarity
        && old.identity.as_ref().map(|identity| identity.id) == new.identity.as_ref().map(|identity| identity.id)
        && old.implicit_rolls == new.implicit_rolls
}
/// What a read does with the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogRead {
    Keep,
    /// The game's language changed: the catalog again from the game data service, relabelled.
    Reload,
    /// Settings' Read again: the files read again first.
    ReadFilesAgain,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Enable,
    Disable,
    StartMonitoring,
    SetAutoConnect(bool),
    /// Serialized with edits: accepted work drains before this setting changes.
    SetOverride(bool),
    Connect,
    Retry,
    Disconnect,
    RefreshCatalog,
    ReadForge,
    DescribeUnique {
        expected: Box<crate::item::edit::ExpectedObservation>,
    },
    PreviewUnique {
        expected: Box<crate::item::edit::ExpectedObservation>,
        draft: crate::item::edit::RollEdit,
    },
    ResolveMapping {
        expected: Box<crate::item::edit::ExpectedObservation>,
        request: MappingRequest,
    },
    BeginForgeBatch,
    Write(Box<ForgePlan>),
    UndoForge(Box<crate::item::undo::ForgeUndo>),
    /// Shows another profile; only while no game runs.
    SelectProfile(ProfileKey),
    /// Keeps the hooks of the character in play, which has no profile, as its profile.
    CreateProfile,
    /// While no game runs, or the profile of the character in play, which goes on with its hooks for this connection.
    DeleteProfile(ProfileKey),
    /// Changes the shown hooks, saving a profile's. While their character is in play the hook is applied too.
    SetHookWanted {
        id: HookId,
        on: bool,
    },
    /// As `SetHookWanted`, for a hook's values.
    SetHookValues {
        id: HookId,
        values: Vec<f64>,
    },
    Shutdown,
}
impl Command {
    /// Whether it supersedes the work queued and in flight before it. Turning automatic connection off stops a connection
    /// being made, but leaves one that is made alone: a write in it would otherwise lose its confirmation (the session
    /// skips the readback of a superseded write and lets the helper go) while the session went on looking Ready.
    fn barrier(&self, phase: Phase) -> bool {
        match self {
            Self::Disable | Self::Disconnect | Self::Shutdown => true,
            Self::SetAutoConnect(false) => phase != Phase::Ready,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackendOptions {
    pub auto_connect: bool,
    pub override_enabled: bool,
    pub poll_interval: Duration,
    pub discovery_interval: Duration,
    /// How often the monitor asks the game for its language; a command always asks.
    pub locale_interval: Duration,
    /// The game data service, the catalog's only source. Without it the trainer does not open.
    pub game_data: Option<GameDataHandle>,
    /// The profiles as saved, and where a change to them goes; without a saver they last for the run.
    pub profiles: ProfileBook,
    pub save_profiles: Option<ProfileSaver>,
    /// Where the unique modifiers in use are kept between runs; without it they last for the run.
    pub unique_mods_in_use: Option<std::path::PathBuf>,
}
impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            auto_connect: true,
            override_enabled: false,
            poll_interval: Duration::from_millis(250),
            discovery_interval: Duration::from_secs(1),
            locale_interval: Duration::from_secs(2),
            game_data: None,
            profiles: ProfileBook::default(),
            save_profiles: None,
            unique_mods_in_use: None,
        }
    }
}

struct Envelope {
    command: Command,
    epoch: u64,
    reply: oneshot::Sender<Result<(), BackendError>>,
    read_cancel: Option<Arc<AtomicBool>>,
}
pub struct BackendHandle {
    commands: mpsc::Sender<Envelope>,
    snapshots: watch::Receiver<BackendSnapshot>,
    intent: Arc<Mutex<u64>>,
    handles: Arc<AtomicUsize>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Full,
    Stopped,
}
pub struct Pending {
    reply: oneshot::Receiver<Result<(), BackendError>>,
}
impl Pending {
    /// Poll completion without blocking the desktop event loop. A dropped worker
    /// is a completion failure; accepted writes must still inspect their record.
    pub fn try_result(&mut self) -> Option<Result<(), BackendError>> {
        match self.reply.try_recv() {
            Ok(result) => Some(result),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(BackendError::blocked(ErrorKind::Stopped, "backend stopped"))),
        }
    }
    pub async fn wait(self) -> Result<(), BackendError> {
        self.reply.await.unwrap_or_else(|_| Err(BackendError::blocked(ErrorKind::Stopped, "backend stopped")))
    }
}
/// Receipt for an explicitly disposable value lookup. Dropping it requests
/// cancellation between RPCs. Ordinary Pending receipts, including all writes,
/// keep their existing drain-to-completion behavior.
pub struct MappingRead {
    pending: Pending,
    cancel: Arc<AtomicBool>,
}
impl MappingRead {
    pub fn try_result(&mut self) -> Option<Result<(), BackendError>> {
        self.pending.try_result()
    }
}
impl Drop for MappingRead {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl BackendHandle {
    /// Reads the current snapshot in place, without copying it, for a question of a field or two. `read` must not call
    /// back into a handle: the worker publishes under the command lock, so submitting while the snapshot is borrowed would
    /// deadlock.
    pub fn peek<R>(&self, read: impl FnOnce(&BackendSnapshot) -> R) -> R {
        read(&self.snapshots.borrow())
    }
    pub fn snapshot(&self) -> BackendSnapshot {
        self.snapshots.borrow().clone()
    }
    /// The worker still runs: its snapshots can still change.
    pub fn is_running(&self) -> bool {
        self.snapshots.has_changed().is_ok()
    }
    pub fn subscribe(&self) -> watch::Receiver<BackendSnapshot> {
        self.snapshots.clone()
    }
    pub fn try_submit(&self, command: Command) -> Result<Pending, SubmitError> {
        self.submit(command, None)
    }
    pub fn try_resolve_mapping(&self, expected: crate::item::edit::ExpectedObservation, request: MappingRequest) -> Result<MappingRead, SubmitError> {
        let cancel = Arc::new(AtomicBool::new(false));
        let pending = self.submit(
            Command::ResolveMapping {
                expected: Box::new(expected),
                request,
            },
            Some(cancel.clone()),
        )?;
        Ok(MappingRead { pending, cancel })
    }
    fn submit(&self, command: Command, read_cancel: Option<Arc<AtomicBool>>) -> Result<Pending, SubmitError> {
        let barrier = command.barrier(self.snapshots.borrow().phase);
        let (reply, receiver) = oneshot::channel();
        let mut epoch = self.intent.lock().unwrap_or_else(|p| p.into_inner());
        let accepted = if barrier { epoch.wrapping_add(1) } else { *epoch };
        self.commands
            .try_send(Envelope {
                command,
                epoch: accepted,
                reply,
                read_cancel,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => SubmitError::Full,
                mpsc::error::TrySendError::Closed(_) => SubmitError::Stopped,
            })?;
        *epoch = accepted;
        Ok(Pending { reply: receiver })
    }
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        if self.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            let mut epoch = self.intent.lock().unwrap_or_else(|p| p.into_inner());
            *epoch = epoch.wrapping_add(1);
        }
    }
}

impl Clone for BackendHandle {
    fn clone(&self) -> Self {
        self.handles.fetch_add(1, Ordering::Relaxed);
        Self {
            commands: self.commands.clone(),
            snapshots: self.snapshots.clone(),
            intent: self.intent.clone(),
            handles: self.handles.clone(),
        }
    }
}

pub struct BackendWorker<P, C, L, K>
where
    C: Connector,
{
    locator: P,
    connector: C,
    log: L,
    clock: K,
    options: BackendOptions,
    commands: mpsc::Receiver<Envelope>,
    snapshots: watch::Sender<BackendSnapshot>,
    intent: Arc<Mutex<u64>>,
    state: BackendSnapshot,
    connection: Option<C::Connection>,
    retiring: Option<C::Connection>,
    blocked: Option<ProcessIdentity>,
    next_discovery: tokio::time::Instant,
    next_locale_check: tokio::time::Instant,
    command_epoch: u64,
    read_cancel: Option<Arc<AtomicBool>>,
    paused: bool,
    faults: Option<eg_core::fault::FaultLog>,
    forge_history: crate::item::undo::ForgeUndoHistory,
    /// The game data service's state, mirrored into `state.game_files`.
    game_data: Option<watch::Receiver<GameData>>,
    /// Whether the last look for the game found one running (any number of them).
    games_running: bool,
    /// The offline character the last admission let in, as Player.log names it.
    character: Option<eg_core::game_log::LoggedCharacter>,
    /// The profile of the character in play in this connection.
    live: Option<ProfileKey>,
    /// The added affixes in use, kept between runs.
    in_use: crate::definitions::unique_mods::InUse,
    /// Those this connection has had the game add, or has given up on.
    registered: std::collections::BTreeSet<u16>,
}

pub fn service<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock>(
    locator: P,
    connector: C,
    log: L,
    clock: K,
    options: BackendOptions,
) -> (BackendHandle, BackendWorker<P, C, L, K>) {
    let game_data = options.game_data.as_ref().map(GameDataHandle::subscribe);
    let in_use = crate::definitions::unique_mods::InUse::open(options.unique_mods_in_use.clone());
    let state = BackendSnapshot {
        revision: 0,
        generation: 0,
        enabled: false,
        monitoring: false,
        auto_connect: options.auto_connect,
        override_enabled: options.override_enabled,
        phase: Phase::Disabled,
        activity: None,
        detected: None,
        attached: None,
        retained_helper: None,
        gate: None,
        cleanup: Cleanup::NotNeeded,
        runtime_status: None,
        error: None,
        catalog: CatalogState::Missing,
        game_files: game_data.as_ref().map_or(GameFiles::NotConfigured, |data| GameFiles::from(&*data.borrow())),
        forge: ForgeState::Unobserved,
        forge_read_sequence: 0,
        mappings: vec![],
        unique_preview: None,
        capabilities: Capabilities::default(),
        last_write: None,
        undo_forge: None,
        undo_forge_depth: 0,
        undo_forge_needs_override: false,
        editing: options.profiles.profiles().first().map(|profile| profile.key.clone()),
        profiles: Arc::new(options.profiles.clone()),
        unsaved: None,
        profile_lock: ProfileLock::Editable,
        hooks: BTreeMap::new(),
    };
    let (sender, commands) = mpsc::channel(32);
    let (snapshots, receiver) = watch::channel(state.clone());
    let intent = Arc::new(Mutex::new(0));
    (
        BackendHandle {
            commands: sender,
            snapshots: receiver,
            intent: intent.clone(),
            handles: Arc::new(AtomicUsize::new(1)),
        },
        BackendWorker {
            locator,
            connector,
            log,
            clock,
            options,
            commands,
            snapshots,
            intent,
            state,
            connection: None,
            retiring: None,
            blocked: None,
            next_discovery: tokio::time::Instant::now(),
            next_locale_check: tokio::time::Instant::now(),
            command_epoch: 0,
            read_cancel: None,
            paused: false,
            faults: None,
            forge_history: Default::default(),
            game_data,
            games_running: false,
            character: None,
            live: None,
            in_use,
            registered: std::collections::BTreeSet::new(),
        },
    )
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    /// Persist faults at their source, independently of advisory event delivery.
    pub fn with_fault_log(mut self, faults: eg_core::fault::FaultLog) -> Self {
        self.faults = Some(faults);
        self
    }
    pub async fn run(mut self) {
        self.mirror_game_data();
        let interval = self.options.poll_interval.max(Duration::from_millis(1));
        let mut tick = tokio::time::Instant::now() + interval;
        loop {
            match tokio::time::timeout_at(tick, self.commands.recv()).await {
                Ok(Some(envelope)) => {
                    let stop = envelope.command == Command::Shutdown;
                    self.command_epoch = envelope.epoch;
                    self.read_cancel = envelope.read_cancel;
                    let reading = !matches!(
                        envelope.command,
                        Command::Write(_) | Command::UndoForge(_) | Command::SetHookWanted { .. } | Command::SetHookValues { .. }
                    );
                    let result = self.command(envelope.command).await;
                    self.drain_departure().await;
                    if reading && result.as_ref().is_err_and(|error| error.kind == ErrorKind::Transport) {
                        self.recover_departed_read();
                    }
                    self.read_cancel = None;
                    let _ = envelope.reply.send(result);
                    if stop {
                        break;
                    }
                }
                Ok(None) => {
                    self.close(false, true).await;
                    break;
                }
                Err(_) => {
                    let reading = self.connection.is_some();
                    self.poll().await;
                    self.drain_departure().await;
                    if reading {
                        self.recover_departed_read();
                    }
                    tick = tokio::time::Instant::now() + interval;
                }
            }
        }
    }
    fn epoch(&self) -> u64 {
        *self.intent.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn superseded(&self, epoch: u64) -> Result<(), BackendError> {
        if epoch != self.epoch() { Err(superseded()) } else { Ok(()) }
    }
    /// One call into the connected game: admitted before it, and after it answers still the current intent and
    /// still admitted. The call's own result is the caller's to settle.
    async fn call<T>(&mut self, epoch: u64, target: &ProcessIdentity, op: impl AsyncFnOnce(&mut C::Connection) -> T) -> Result<T, BackendError> {
        self.admit(target)?;
        let result = op(self.connection.as_mut().ok_or_else(released)?).await;
        self.superseded(epoch)?;
        self.admit(target)?;
        Ok(result)
    }
    fn publish(&mut self) {
        self.state.profile_lock = self.profile_lock();
        let expected = self.state.expected_observation().ok();
        self.state.mappings.retain(|mapping| expected.as_ref().is_some_and(|expected| mapping.holds_for(expected)));
        if self
            .state
            .unique_preview
            .as_ref()
            .is_some_and(|p| expected.as_ref().is_none_or(|current| !p.expected.matches(current)))
        {
            self.state.unique_preview = None;
        }
        if self.state.undo_forge.as_ref().is_some_and(|undo| {
            self.state.expected_observation().as_ref() != Ok(undo.after())
                || self.state.runtime_status.as_ref().is_none_or(|status| status.online || status.manager != undo.manager())
        }) {
            self.state.undo_forge = None;
            self.forge_history.clear();
        }
        self.state.undo_forge_depth = if self.state.undo_forge.is_some() { self.forge_history.len() } else { 0 };
        self.state.undo_forge_needs_override = self.state.undo_forge.is_some() && self.forge_history.needs_override();
        let ready = self.state.phase == Phase::Ready;
        let idle = ready && self.state.activity.is_none();
        let served = self.connection.as_ref().map(BackendConnection::capabilities).unwrap_or_default();
        self.state.capabilities = Capabilities {
            gameplay_hooks: served.gameplay_hooks.map(|hook| idle && hook),
            read_forge: ready,
            level_requirement: idle && served.level_requirement,
            status_fields: served.status_fields.map(|field| idle && field),
            base_change: idle && served.base_change,
            identity_change: idle && served.identity_change,
            affix_rolls: idle && served.affix_rolls,
            modifier_rolls: idle && served.modifier_rolls,
            affix_change: idle && served.affix_change,
            affix_list: idle && served.affix_list,
        };
        // An unchanged state is not sent again: every observer would wake to redraw the same thing (the monitor reads four
        // times a second).
        if self.same_as_published(true) {
            return;
        }
        self.state.revision = self.snapshots.borrow().revision.wrapping_add(1);
        self.snapshots.send_replace(self.state.clone());
    }
    /// Whether the state is what was last published, apart from its revision (and its read sequence unless
    /// `sequence_too`).
    fn same_as_published(&mut self, sequence_too: bool) -> bool {
        let published = self.snapshots.borrow();
        let (revision, sequence) = (self.state.revision, self.state.forge_read_sequence);
        self.state.revision = published.revision;
        if !sequence_too {
            self.state.forge_read_sequence = published.forge_read_sequence;
        }
        let same = *published == self.state;
        self.state.revision = revision;
        self.state.forge_read_sequence = sequence;
        same
    }
    fn publish_if_current(&mut self, epoch: u64) -> Result<(), BackendError> {
        let intent = self.intent.clone();
        let guard = intent.lock().unwrap_or_else(|p| p.into_inner());
        if *guard != epoch {
            return Err(superseded());
        }
        self.publish();
        Ok(())
    }
    fn commit(&mut self, epoch: u64, change: impl FnOnce(&mut Self)) -> Result<(), BackendError> {
        let intent = self.intent.clone();
        let guard = intent.lock().unwrap_or_else(|p| p.into_inner());
        if *guard != epoch {
            return Err(superseded());
        }
        change(self);
        self.publish();
        Ok(())
    }
    /// Mirrors the game data service's state into the snapshot, for the views.
    fn mirror_game_data(&mut self) {
        let Some(game_data) = &mut self.game_data else {
            return;
        };
        let files = GameFiles::from(&*game_data.borrow_and_update());
        if files != self.state.game_files {
            self.state.game_files = files;
            self.publish();
        }
    }
    /// The game data for the folder `target` runs from (read again first when `again`, or when its files changed), if
    /// it can describe the running game: none of its files written since the game started. Needs no connection, so a
    /// connect asks before anything is loaded into the game.
    async fn game_files_for(&mut self, epoch: u64, target: &ProcessIdentity, again: bool) -> Result<Arc<crate::definitions::files::FileCatalog>, BackendError> {
        let not_ready = |reason: String| BackendError::blocked(ErrorKind::NotReady, reason);
        let handle = self
            .options
            .game_data
            .clone()
            .ok_or_else(|| not_ready("The game's files are not configured, so there is no game data.".into()))?;
        let folder = target
            .executable
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| not_ready("The game's folder is unknown.".into()))?;
        let started = win_process::from_filetime(target.created).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
        self.state.activity = Some(Activity::ReadingGameFiles);
        self.publish_if_current(epoch)?;
        let data = if again {
            handle.read_again(Some(&folder)).await
        } else {
            handle.for_folder(&folder).await
        };
        self.superseded(epoch)?;
        self.mirror_game_data();
        let files = match data {
            GameData::Ready(files) => files,
            GameData::Unavailable(reason) => return Err(not_ready(format!("The game data could not be read from the game's files: {reason}"))),
            GameData::NoGame | GameData::Reading => return Err(not_ready("The game data is not ready.".into())),
        };
        if files.written_after(std::time::SystemTime::from(started)) {
            return Err(files_changed());
        }
        Ok(files)
    }
    /// The connected game's catalog: its game data (`game_files_for`), if the session runs the build they hold,
    /// labelled with the game's language.
    async fn catalog_for(&mut self, epoch: u64, target: &ProcessIdentity, again: bool) -> Result<GameCatalog, BackendError> {
        let files = self.game_files_for(epoch, target, again).await?;
        let (binary, metadata) = self.connection.as_mut().ok_or_else(released)?.build_hashes();
        if !files.matches(&binary, &metadata) {
            return Err(files_changed());
        }
        let locale = self.call(epoch, target, async |connection| connection.read_locale().await).await??;
        files
            .catalog(locale)
            .map_err(|e| BackendError::blocked(ErrorKind::Protocol, format!("The game data was refused for this session: {e:?}")))
    }
    async fn command(&mut self, command: Command) -> Result<(), BackendError> {
        match command {
            Command::SetOverride(value) => {
                self.state.override_enabled = value;
                self.publish();
                Ok(())
            }
            Command::Enable => {
                self.state.enabled = true;
                self.paused = false;
                if self.state.phase == Phase::Disabled {
                    self.state.phase = Phase::Disconnected;
                }
                self.next_discovery = tokio::time::Instant::now();
                self.publish();
                Ok(())
            }
            Command::StartMonitoring => {
                self.state.monitoring = true;
                if self.state.enabled && self.state.auto_connect && self.connection.is_none() {
                    self.state.phase = Phase::WaitingForGame;
                }
                self.publish();
                Ok(())
            }
            Command::SetAutoConnect(value) => {
                self.state.auto_connect = value;
                self.next_discovery = tokio::time::Instant::now();
                if value {
                    self.paused = false;
                } else if self.state.phase == Phase::Connecting {
                    self.close(false, false).await;
                } else {
                    self.state.activity = None;
                }
                self.publish();
                Ok(())
            }
            Command::Connect | Command::Retry => {
                if !self.state.enabled {
                    return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
                }
                if command == Command::Retry {
                    self.blocked = None;
                }
                self.paused = false;
                self.connect().await
            }
            Command::Disconnect => {
                self.paused = true;
                self.close(false, false).await;
                Ok(())
            }
            Command::Disable => {
                self.close(true, false).await;
                Ok(())
            }
            Command::Shutdown => {
                self.close(true, true).await;
                Ok(())
            }
            // Settings' Read again: the files are read again whether or not a game is connected.
            Command::RefreshCatalog if self.state.phase == Phase::Ready => self.read(CatalogRead::ReadFilesAgain, true).await,
            Command::RefreshCatalog => {
                if let Some(handle) = self.options.game_data.clone() {
                    handle.read_again(None).await;
                    self.mirror_game_data();
                }
                Ok(())
            }
            Command::ReadForge => self.read(CatalogRead::Keep, true).await,
            Command::BeginForgeBatch => {
                self.forge_history.begin_batch();
                Ok(())
            }
            Command::DescribeUnique { expected } => self.describe_unique(*expected).await,
            Command::PreviewUnique { expected, draft } => self.preview_unique(*expected, draft).await,
            Command::ResolveMapping { expected, request } => self.resolve_mapping(*expected, request).await,
            Command::Write(plan) => self.write_forge(*plan).await,
            Command::UndoForge(undo) => self.undo_forge(*undo).await,
            Command::SelectProfile(key) => self.select_profile(key),
            Command::CreateProfile => self.create_profile(),
            Command::DeleteProfile(key) => self.delete_profile(key),
            Command::SetHookWanted { id, on } => self.set_hook_wanted(id, on).await,
            Command::SetHookValues { id, values } => self.set_hook_values(id, values).await,
        }
    }
    async fn poll(&mut self) {
        self.mirror_game_data();
        let exited = self.connector.reap_exited();
        if self.state.retained_helper.as_ref().is_some_and(|helper| exited.contains(&helper.target)) {
            self.state.retained_helper = None;
            self.state.cleanup = Cleanup::ConfirmedOff;
            if self.blocked.as_ref().is_some_and(|target| exited.contains(target)) {
                self.blocked = None;
                self.state.error = None;
                self.state.phase = Phase::WaitingForGame;
            }
            self.publish();
        }
        if self.connection.as_ref().is_some_and(|connection| exited.contains(connection.target())) {
            self.release_without_io();
            self.games_running = false;
            self.state.detected = None;
            self.state.phase = Phase::WaitingForGame;
            self.publish();
        }
        if !self.state.enabled || !self.state.monitoring {
            return;
        }
        if self.connection.is_some() {
            // Hooks refused while the character loaded go on once it is in play again.
            if !self.paused && self.read(CatalogRead::Keep, false).await.is_ok() {
                let _ = self.apply_queued(self.command_epoch).await;
            }
        } else if tokio::time::Instant::now() >= self.next_discovery {
            self.next_discovery = tokio::time::Instant::now() + self.options.discovery_interval.max(Duration::from_millis(1));
            if self.state.auto_connect && !self.paused {
                let _ = self.connect().await;
            } else {
                self.watch().await;
            }
        }
    }
    /// Without automatic connection only looks, so the profiles lock while a game runs. Opens nothing, and a failed
    /// look changes nothing.
    async fn watch(&mut self) {
        if let Ok(targets) = self.locator.find().await {
            self.observe(&targets);
            self.publish();
        }
    }
    fn observe(&mut self, targets: &[ProcessIdentity]) {
        self.games_running = !targets.is_empty();
        self.state.detected = match targets {
            [target] => Some(target.clone()),
            _ => None,
        };
    }
    fn admit(&mut self, target: &ProcessIdentity) -> Result<(), BackendError> {
        let started = win_process::from_filetime(target.created).map_err(|_| BackendError::blocked(ErrorKind::Identity, "invalid process creation time"))?;
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(BackendError::blocked(ErrorKind::Identity, "invalid process identity"));
        }
        let log = self.log.read(started);
        let decision = gate::evaluate(&log, started, self.clock.now());
        self.state.gate = Some(decision);
        match decision.verdict {
            GateVerdict::Allow => {
                self.character = log.character.filter(|character| !character.online);
                Ok(())
            }
            GateVerdict::Wait | GateVerdict::Refuse => {
                self.release_without_io();
                self.state.phase = if decision.verdict == GateVerdict::Refuse {
                    Phase::Blocked
                } else if decision.readiness == GameReadiness::Starting {
                    Phase::WaitingForGame
                } else {
                    Phase::WaitingForCharacter
                };
                let error = BackendError {
                    kind: ErrorKind::Gate,
                    message: gate::refusal_message(&log),
                    delivery: None,
                    retry_when_waiting: decision.verdict == GateVerdict::Wait,
                };
                self.state.error = Some(error.clone());
                self.publish();
                Err(error)
            }
        }
    }
    async fn connect(&mut self) -> Result<(), BackendError> {
        self.drain_departure().await;
        if self.connection.is_some() {
            return self.read(CatalogRead::Keep, true).await;
        }
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        let targets = match self.locator.find().await {
            Ok(targets) => targets,
            Err(error) => {
                self.fail(error.clone(), None);
                return Err(error);
            }
        };
        self.superseded(epoch)?;
        self.observe(&targets);
        if targets.len() != 1 {
            self.state.phase = Phase::WaitingForGame;
            self.state.error = (targets.len() > 1).then(|| BackendError::waiting(ErrorKind::MultipleProcesses, "close extra matching game instances"));
            self.publish();
            return Ok(());
        }
        let target = targets.into_iter().next().unwrap();
        if self
            .state
            .retained_helper
            .as_ref()
            .is_some_and(|helper| helper.target == target && helper.residency == HelperResidency::Stuck)
        {
            let error = BackendError::blocked(ErrorKind::RestartRequired, HelperRestartRequired);
            self.state.phase = Phase::Blocked;
            self.state.error = Some(error.clone());
            self.publish();
            return Err(error);
        }
        if self.blocked.as_ref() == Some(&target) {
            self.state.phase = Phase::Blocked;
            self.publish();
            return Ok(());
        }
        self.admit(&target)?;
        // Nothing is loaded into a game its game data cannot describe.
        if let Err(error) = self.game_files_for(epoch, &target, false).await {
            if error.kind != ErrorKind::Superseded {
                self.fail(error.clone(), Some(target));
            }
            return Err(error);
        }
        self.admit(&target)?;
        self.state.generation = self.state.generation.wrapping_add(1);
        self.state.phase = Phase::Connecting;
        self.state.activity = Some(Activity::Connecting);
        self.state.error = None;
        self.state.retained_helper = Some(RetainedHelper {
            target: target.clone(),
            residency: HelperResidency::MayBeLoaded,
        });
        // A previous acknowledgement cannot describe a new preparation attempt,
        // which may load/start a helper before returning an error.
        self.state.cleanup = Cleanup::Unconfirmed;
        self.publish_if_current(epoch)?;
        let connection = match self.connector.connect(target.clone(), self.clock.now()).await {
            Ok(connection) => connection,
            Err(error) => {
                self.fail(error.clone(), Some(target));
                return Err(error);
            }
        };
        if connection.target() != &target {
            let error = BackendError::blocked(ErrorKind::Identity, "connector returned a different process identity");
            self.fail(error.clone(), Some(target));
            return Err(error);
        }
        self.connection = Some(connection);
        self.registered.clear();
        self.state.attached = Some(target.clone());
        self.state.retained_helper = Some(RetainedHelper {
            target: target.clone(),
            residency: HelperResidency::Loaded,
        });
        self.state.cleanup = Cleanup::Unconfirmed;
        self.superseded(epoch)?;
        self.initialize(epoch, &target).await?;
        self.apply_queued(epoch).await
    }
    async fn initialize(&mut self, epoch: u64, target: &ProcessIdentity) -> Result<(), BackendError> {
        let result: Result<(), BackendError> = async {
            self.admit(target)?;
            let status = self.connection.as_mut().unwrap().refresh().await?;
            self.superseded(epoch)?;
            self.check_status(status)?;
            self.reconcile_hooks();
            let catalog = self.catalog_for(epoch, target, false).await?;
            self.admit(target)?;
            self.state.activity = Some(Activity::ReadingForge);
            self.publish_if_current(epoch)?;
            let forge = self.call(epoch, target, async |connection| connection.read_forge().await).await??;
            self.register_in_play(epoch, target, &catalog, forge.as_ref()).await?;
            let implicits = match &forge {
                Some(item) => self.call(epoch, target, async |connection| connection.describe_implicits(item).await).await??,
                None => None,
            };
            let data = Arc::new(catalog);
            let mut forge = self.forge_state(forge, &data);
            if let ForgeState::Item { implicits: observed, .. } = &mut forge {
                *observed = implicits;
            }
            self.commit(epoch, |this| {
                if let ForgeState::Item { unique, .. } = &mut forge {
                    *unique = None;
                }
                this.state.forge = forge;
                this.state.forge_read_sequence = this.state.forge_read_sequence.wrapping_add(1);
                this.state.mappings.retain(|mapping| mapping.result.is_ok());
                this.state.catalog = CatalogState::Current {
                    generation: this.state.generation,
                    data: data.clone(),
                };
                this.state.phase = Phase::Ready;
                this.state.activity = None;
                this.state.error = None;
                // The character in play is shown in the same snapshot that says Ready.
                this.enter_play();
            })?;
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.settle(error, target.clone());
        }
        result
    }
    fn check_status(&mut self, status: RuntimeStatus) -> Result<(), BackendError> {
        if self.state.runtime_status.is_some_and(|previous| previous.manager != status.manager) {
            self.state.mappings.clear();
            self.state.unique_preview = None;
        }
        if status.online || status.manager == 0 {
            self.release_without_io();
            self.state.phase = Phase::WaitingForCharacter;
            let error = BackendError::waiting(ErrorKind::Gate, "offline character required");
            self.state.error = Some(error.clone());
            self.publish();
            Err(error)
        } else {
            self.state.runtime_status = Some(status);
            Ok(())
        }
    }
    async fn read(&mut self, catalog: CatalogRead, retry_optional: bool) -> Result<(), BackendError> {
        let reload = catalog != CatalogRead::Keep;
        if !self.state.enabled {
            return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
        }
        if self.state.phase != Phase::Ready && !(self.state.phase == Phase::WaitingForCharacter && self.connection.is_some()) {
            return Err(BackendError::blocked(ErrorKind::NotConnected, "session is not ready"));
        }
        let target = self
            .state
            .attached
            .clone()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "no offline character connected"))?;
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        let result: Result<(), BackendError> = async {
            self.admit(&target)?;
            // The monitor's own reads run quietly: saying Refreshing four times a second would take every action away
            // (capabilities need no activity) and give it back a moment later. Commands wait for the worker regardless.
            let monitor = !reload && !retry_optional;
            if !monitor {
                self.state.activity = Some(Activity::Refreshing);
                self.publish_if_current(epoch)?;
            }
            let status = self.connection.as_mut().ok_or_else(released)?.refresh().await?;
            self.superseded(epoch)?;
            self.check_status(status)?;
            self.reconcile_hooks();
            // Successful value tables can be reused without another mapping
            // command. Poll locale here too, so even those tables expire when
            // the game changes language while the editor is idle; the monitor
            // asks every `locale_interval`, not on every tick.
            let check_locale = !reload && (!monitor || tokio::time::Instant::now() >= self.next_locale_check);
            let locale_changed = check_locale && self.locale_changed(epoch, &target).await?;
            if check_locale {
                self.next_locale_check = tokio::time::Instant::now() + self.options.locale_interval;
            }
            let data = if reload || locale_changed {
                Arc::new(self.catalog_for(epoch, &target, catalog == CatalogRead::ReadFilesAgain).await?)
            } else {
                self.state
                    .catalog
                    .data()
                    .cloned()
                    .ok_or_else(|| BackendError::blocked(ErrorKind::Protocol, "current catalog missing"))?
            };
            let forge = self.call(epoch, &target, async |connection| connection.read_forge().await).await??;
            self.register_in_play(epoch, &target, &data, forge.as_ref()).await?;
            // The game builds an item's implicit text with its tooltip code on its main thread: the monitor keeps the
            // text it has while the base and implicit rolls are unchanged, under the same catalog.
            let kept = match &forge {
                Some(item) if monitor && !locale_changed => self.kept_implicits(item),
                _ => None,
            };
            let implicits = match (&forge, kept) {
                (_, Some(kept)) => kept,
                (Some(item), None) => self.call(epoch, &target, async |connection| connection.describe_implicits(item).await).await??,
                (None, None) => None,
            };
            let mut forge = self.forge_state(forge, &data);
            if let ForgeState::Item { implicits: observed, .. } = &mut forge {
                *observed = implicits;
            }
            self.commit(epoch, |this| {
                if let ForgeState::Item { unique, .. } = &mut forge
                    && (reload || locale_changed || retry_optional && unique.as_ref().is_some_and(Result::is_err))
                {
                    *unique = None;
                }
                if this.state.unique_preview.as_ref().is_some_and(|p| retry_optional && p.result.is_err()) {
                    this.state.unique_preview = None;
                }
                this.state.forge = forge;
                this.state.phase = Phase::Ready;
                this.state.mappings.retain(|mapping| mapping.result.is_ok() || !retry_optional && !mapping.retry_on_read);
                this.state.catalog = CatalogState::Current {
                    generation: this.state.generation,
                    data: data.clone(),
                };
                this.state.activity = None;
                this.state.error = None;
                // A fresh read lets the view ask again for what failed; a monitor read that saw nothing new is not one,
                // so it publishes nothing.
                if !monitor || reload || locale_changed || !this.same_as_published(false) {
                    this.state.forge_read_sequence = this.state.forge_read_sequence.wrapping_add(1);
                }
            })?;
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.settle(error, target);
        }
        result
    }
    fn forge_state(&self, item: Option<ForgeSnapshot>, catalog: &GameCatalog) -> ForgeState {
        match item {
            None => ForgeState::NoItem {
                generation: self.state.generation,
            },
            Some(snapshot) => {
                let resolved_name = catalog.resolve_item_name(&snapshot);
                ForgeState::Item {
                    generation: self.state.generation,
                    resolved_name,
                    implicits: None,
                    unique: match &self.state.forge {
                        ForgeState::Item {
                            generation,
                            snapshot: old,
                            unique,
                            ..
                        } if *generation == self.state.generation && (**old == snapshot || matches!(unique, Some(Ok(_))) && same_unique_text(old, &snapshot)) => unique.clone(),
                        _ => None,
                    },
                    snapshot: Arc::new(snapshot),
                }
            }
        }
    }
    /// The implicit text the state holds for `item` when the game would build it the same, so a changed item whose base
    /// and implicit rolls are unchanged is not described again. A failed description is kept only for the same item.
    fn kept_implicits(&self, item: &ForgeSnapshot) -> Option<Option<Vec<ModifierSnapshot>>> {
        match &self.state.forge {
            ForgeState::Item {
                generation, snapshot, implicits, ..
            } if *generation == self.state.generation && (**snapshot == *item || implicits.is_some() && same_implicit_text(snapshot, item)) => Some(implicits.clone()),
            _ => None,
        }
    }
    /// After a failed read of the connected game: a gate or a departure takes the connection back, a superseded read
    /// is left as it is, and anything else fails the worker.
    fn settle(&mut self, error: &BackendError, target: ProcessIdentity) {
        self.handle_connection_gate(error);
        if !matches!(
            error.kind,
            ErrorKind::Superseded | ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading
        ) {
            self.fail(error.clone(), Some(target));
        }
    }

    fn handle_connection_gate(&mut self, error: &BackendError) {
        if error.kind == ErrorKind::CharacterLoading && self.connection.is_some() {
            self.state.phase = Phase::WaitingForCharacter;
            self.state.runtime_status = None;
            self.state.forge = ForgeState::Unobserved;
            self.state.undo_forge = None;
            self.state.activity = None;
            self.state.error = Some(error.clone());
            self.publish();
            return;
        }
        if matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted) && self.connection.is_some() {
            self.release_without_io();
            self.state.phase = Phase::WaitingForCharacter;
            self.state.error = Some(error.clone());
            self.publish();
        }
    }
    fn release_without_io(&mut self) {
        // The helper takes its hooks with it, and hooks without a profile end with the connection.
        self.live = None;
        self.state.hooks.clear();
        self.state.unsaved = None;
        if self.state.editing.as_ref().is_some_and(|key| self.state.profiles.get(key).is_none()) {
            self.state.editing = self.state.profiles.profiles().first().map(|profile| profile.key.clone());
        }
        self.forge_history.clear();
        self.state.undo_forge = None;
        if let Some(connection) = self.connection.take() {
            debug_assert!(self.retiring.is_none());
            self.retiring = Some(connection);
            self.state.cleanup = Cleanup::Unconfirmed;
        }
        self.state.attached = None;
        self.state.runtime_status = None;
        self.state.activity = None;
        self.state.forge = ForgeState::Unobserved;
        if let CatalogState::Current { data, .. } = &self.state.catalog {
            self.state.catalog = CatalogState::Stale(data.clone());
        }
        self.state.generation = self.state.generation.wrapping_add(1);
    }
    fn fail(&mut self, error: BackendError, target: Option<ProcessIdentity>) {
        if let Some(faults) = &self.faults
            && !error.retryable()
            && !matches!(error.kind, ErrorKind::WriteUncertain | ErrorKind::WriteNotKept)
        {
            faults.write(
                "Trainer connection",
                Some(&error.to_string()),
                None,
                Some("The session was released. Reconnect before continuing."),
            );
        }
        self.release_without_io();
        if error.kind == ErrorKind::HelperDeparted {
            self.state.retained_helper = target.clone().map(|target| RetainedHelper {
                target,
                residency: HelperResidency::Gone,
            });
            self.state.cleanup = Cleanup::ConfirmedOff;
            self.blocked = None;
            self.state.phase = Phase::WaitingForCharacter;
        } else if error.retryable() {
            self.state.phase = Phase::WaitingForGame;
        } else {
            self.blocked = target;
            self.state.phase = Phase::Blocked;
        }
        self.state.error = Some(error);
        self.publish();
    }
    fn recover_departed_read(&mut self) {
        if self.state.phase == Phase::Blocked
            && self.state.cleanup == Cleanup::ConfirmedOff
            && self.state.error.as_ref().is_some_and(|error| error.kind == ErrorKind::Transport)
            && self.state.retained_helper.as_ref().is_some_and(|helper| helper.residency == HelperResidency::Gone)
        {
            // The read is over and no write is replayed. Native proof permits a
            // new connection for the next admitted offline character.
            self.blocked = None;
            self.state.phase = Phase::WaitingForCharacter;
            self.state.error = Some(BackendError::waiting(ErrorKind::Gate, "The helper left Last Epoch. Waiting for an offline character."));
            self.publish();
        }
    }
    async fn drain_departure(&mut self) {
        let Some(mut connection) = self.retiring.take() else { return };
        let target = connection.target().clone();
        let phase = self.state.phase;
        self.state.phase = Phase::Closing;
        self.state.retained_helper = Some(RetainedHelper {
            target: target.clone(),
            residency: HelperResidency::Leaving,
        });
        self.publish();
        self.state.cleanup = connection.disconnect().await;
        let gone = self.state.cleanup == Cleanup::ConfirmedOff;
        self.state.retained_helper = Some(RetainedHelper {
            target: target.clone(),
            residency: if gone { HelperResidency::Gone } else { HelperResidency::Stuck },
        });
        self.state.phase = if gone { phase } else { Phase::Blocked };
        if !gone {
            self.blocked = Some(target);
            self.state.error = Some(BackendError::blocked(ErrorKind::RestartRequired, HelperRestartRequired));
        }
        self.publish();
    }
    async fn close(&mut self, disable: bool, stop: bool) {
        self.state.phase = Phase::Closing;
        self.state.activity = None;
        self.state.forge = ForgeState::Unobserved;
        self.publish();
        self.release_without_io();
        self.drain_departure().await;
        if disable {
            self.state.enabled = false;
        }
        self.state.phase = if stop {
            Phase::Stopped
        } else if self.state.enabled && self.state.retained_helper.as_ref().is_some_and(|helper| helper.residency == HelperResidency::Stuck) {
            Phase::Blocked
        } else if self.state.enabled {
            Phase::Disconnected
        } else {
            Phase::Disabled
        };
        self.publish();
    }
}
