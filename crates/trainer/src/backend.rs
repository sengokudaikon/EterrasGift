//! Application-owned, serialized trainer service with explicit write admission.
//!
//! Construction does not spawn, discover, or access files/processes. The owner
//! explicitly runs the worker on a background Tokio runtime. All accepted work
//! runs to completion; dropping a receipt is not cancellation. Cleanup barriers
//! suppress publication from an earlier operation, then drain it before cleanup.
//! Watch snapshots are authoritative; broadcast events can lag and are advisory.
mod fields;
mod rate_write;
mod write;

use std::{
    fmt,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub use crate::session::{CaptureControl, CaptureObserver, CaptureOutcome, CaptureProgress};
use eg_core::game_log::GameReadiness;
pub use eg_game_access::transport::Delivery;
pub use eg_game_access::{
    fields::{AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate, RateUnavailable},
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    mapping::ModifierSnapshot,
    process::ProcessIdentity,
    protocol::RuntimeStatus,
};
use eg_game_access::{
    protocol::ResponseCode,
    transport::{Transport, TransportFailure},
};
use jiff::Timestamp;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    catalog::{GameCatalog, ResolvedItemName},
    catalog_cache::CatalogCache,
    field_writes::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, FieldPlanError, RateWriteRecord, RateWriteStatus},
    gate::{self, GateDecision, GatePolicy, GateVerdict, LogSource},
    session::{Cleanup, Session, SessionError},
    writes::{ForgePlan, WriteControl, WriteObservation, WriteOutcome, WritePlan, WriteRecord, WriteStatus},
};

pub trait ProcessLocator: Send + 'static {
    /// Query identities only. A matching filename alone is not an attachment.
    fn find(&mut self) -> impl Future<Output = Result<Vec<ProcessIdentity>, BackendError>> + Send;
}

pub trait Connector: Send + 'static {
    type Connection: BackendConnection;
    /// Called only after the backend's process-bound log gate allows access.
    /// Implementations must repeat admission after preparation and bind the exact
    /// identity/build/helper. Existing retained modules must never be reinjected.
    fn connect(&mut self, target: ProcessIdentity, now: Timestamp) -> impl Future<Output = Result<Self::Connection, BackendError>> + Send;
}

pub trait BackendConnection: Send + 'static {
    fn target(&self) -> &ProcessIdentity;
    /// Connection health is independent of a particular write's certainty.
    fn is_ready(&self) -> bool {
        true
    }
    /// Return certainty after entering transport; Err is pre-submission unless
    /// delivery explicitly says MayHaveRun. Confirmed/NotKept require a complete
    /// independent read under renewed admission, not only a native ACK.
    fn write(&mut self, now: Timestamp, plan: &WritePlan, control: &mut dyn WriteControl) -> impl Future<Output = Result<WriteOutcome<WriteObservation>, BackendError>> + Send {
        async move {
            match plan {
                WritePlan::Forge(plan) => Ok(self.edit_forge(now, plan, control).await?.map(WriteObservation::forge)),
                WritePlan::Field(plan) => Ok(self.set_attack_leech_rate(now, plan, control).await?.map(WriteObservation::Field)),
            }
        }
    }
    fn refresh(&mut self, now: Timestamp) -> impl Future<Output = Result<RuntimeStatus, BackendError>> + Send;
    fn capture_catalog(&mut self, now: Timestamp) -> impl Future<Output = Result<GameCatalog, BackendError>> + Send;
    fn capture_catalog_observed(&mut self, now: Timestamp, observer: &mut dyn CaptureObserver) -> impl Future<Output = Result<CaptureOutcome, BackendError>> + Send {
        async move {
            if observer.control() == CaptureControl::Stop {
                return Ok(CaptureOutcome::Stopped);
            }
            observer.progress(CaptureProgress::Starting);
            let catalog = self.capture_catalog(now).await?;
            if observer.control() == CaptureControl::Stop {
                return Ok(CaptureOutcome::Stopped);
            }
            Ok(CaptureOutcome::Complete(Box::new(catalog)))
        }
    }
    fn read_forge(&mut self, now: Timestamp) -> impl Future<Output = Result<Option<ForgeSnapshot>, BackendError>> + Send;
    fn describe_implicits(&mut self, _: Timestamp, _: &ForgeSnapshot) -> impl Future<Output = Result<Option<Vec<ModifierSnapshot>>, BackendError>> + Send {
        async { Ok(None) }
    }
    fn supports_read_attack_leech(&self) -> bool {
        false
    }
    fn read_attack_leech(&mut self, _: Timestamp) -> impl Future<Output = Result<AttackLeechRead, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "attack/leech reads are unavailable")) }
    }
    fn supports_attack_leech_rate(&self) -> bool {
        false
    }
    /// Err means submission was not attempted, unless it explicitly carries MayHaveRun.
    /// Returned status must retain every outcome after entering transport; no retry.
    /// Confirmed requires full projected native acknowledgement and a separate fresh
    /// read under renewed admission. The backend separately checks its current sequence.
    fn set_attack_leech_rate(&mut self, _: Timestamp, _: &AttackLeechPlan, _: &mut dyn WriteControl) -> impl Future<Output = Result<RateWriteStatus, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "attack/leech writes are unavailable")) }
    }
    fn supports_level_requirement(&self) -> bool {
        false
    }
    fn supports_status(&self, field: crate::edits::StatusField) -> bool {
        field == crate::edits::StatusField::LevelRequirement && self.supports_level_requirement()
    }
    fn supports_base_change(&self) -> bool {
        false
    }
    fn supports_identity_change(&self) -> bool {
        false
    }
    fn supports_affix_list(&self) -> bool {
        false
    }
    fn supports_affix_change(&self) -> bool {
        false
    }
    fn supports_affix_rolls(&self) -> bool {
        false
    }
    fn supports_forge(&self, edit: &crate::edits::ItemEdit) -> bool {
        match edit {
            crate::edits::ItemEdit::AddAffix(_) | crate::edits::ItemEdit::RemoveAffix(_) => self.supports_affix_list(),
            crate::edits::ItemEdit::Affix(_) => self.supports_affix_change(),
            crate::edits::ItemEdit::Roll(edit) if edit.target == crate::edits::RollTarget::Affix => self.supports_affix_rolls(),
            crate::edits::ItemEdit::Status(edit) => self.supports_status(edit.field),
            crate::edits::ItemEdit::BaseType(_) => self.supports_base_change(),
            crate::edits::ItemEdit::UniqueIdentity(_) => self.supports_identity_change(),
            _ => false,
        }
    }
    /// Err must mean submission was not attempted (or carry MayHaveRun). Once
    /// transport is entered, return a certainty-bearing status even on failure.
    fn edit_forge(&mut self, _: Timestamp, _: &ForgePlan, _: &mut dyn WriteControl) -> impl Future<Output = Result<WriteStatus, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "level requirement writes are unavailable")) }
    }
    fn disconnect(&mut self) -> impl Future<Output = Cleanup> + Send;
    // Dropping a connection must perform no process/pipe I/O. Admission loss
    // drops it directly, without asking the helper to perform cleanup.
}

impl<T: Transport + Send + 'static, L: LogSource + Send + 'static> BackendConnection for Session<T, L> {
    fn target(&self) -> &ProcessIdentity {
        self.target()
    }
    fn is_ready(&self) -> bool {
        self.state() == crate::session::SessionState::Ready
    }
    async fn write(&mut self, now: Timestamp, plan: &WritePlan, control: &mut dyn WriteControl) -> Result<WriteOutcome<WriteObservation>, BackendError> {
        Ok(self.write_controlled(now, plan, control).await?)
    }
    async fn refresh(&mut self, now: Timestamp) -> Result<RuntimeStatus, BackendError> {
        Session::refresh(self, now).await?;
        self.runtime_status()
            .copied()
            .ok_or_else(|| BackendError::blocked(ErrorKind::Protocol, "missing runtime status"))
    }
    async fn capture_catalog(&mut self, now: Timestamp) -> Result<GameCatalog, BackendError> {
        Ok(Session::capture_catalog(self, now).await?)
    }
    async fn capture_catalog_observed(&mut self, now: Timestamp, observer: &mut dyn CaptureObserver) -> Result<CaptureOutcome, BackendError> {
        Ok(Session::capture_catalog_observed(self, now, observer).await?)
    }
    async fn describe_implicits(&mut self, now: Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(Session::describe_implicits(self, now, expected).await?)
    }
    async fn read_forge(&mut self, now: Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        Ok(Session::read_forge(self, now).await?)
    }
    fn supports_read_attack_leech(&self) -> bool {
        true
    }
    async fn read_attack_leech(&mut self, now: Timestamp) -> Result<AttackLeechRead, BackendError> {
        Ok(Session::read_attack_leech(self, now).await?)
    }
    fn supports_attack_leech_rate(&self) -> bool {
        true
    }
    async fn set_attack_leech_rate(&mut self, now: Timestamp, plan: &AttackLeechPlan, control: &mut dyn WriteControl) -> Result<RateWriteStatus, BackendError> {
        Ok(Session::set_attack_leech_rate_controlled(self, now, plan, control).await?)
    }
    fn supports_level_requirement(&self) -> bool {
        true
    }
    fn supports_status(&self, _field: crate::edits::StatusField) -> bool {
        true
    }
    fn supports_base_change(&self) -> bool {
        true
    }
    fn supports_identity_change(&self) -> bool {
        true
    }
    fn supports_affix_list(&self) -> bool {
        true
    }
    fn supports_affix_change(&self) -> bool {
        true
    }
    fn supports_affix_rolls(&self) -> bool {
        true
    }
    async fn edit_forge(&mut self, now: Timestamp, plan: &ForgePlan, control: &mut dyn WriteControl) -> Result<WriteStatus, BackendError> {
        Ok(Session::edit_forge_controlled(self, now, plan, control).await?)
    }
    async fn disconnect(&mut self) -> Cleanup {
        Session::disconnect(self).await
    }
}

pub trait Clock: Send + 'static {
    fn now(&self) -> Timestamp;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Disabled,
    NotConnected,
    NotReady,
    MultipleProcesses,
    Gate,
    Transport,
    Protocol,
    Identity,
    RestartRequired,
    Superseded,
    Stopped,
    Fault,
    InvalidEdit,
    WriteNotApplied,
    WriteNotKept,
    WriteUncertain,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError {
    pub kind: ErrorKind,
    pub message: String,
    pub delivery: Option<Delivery>,
    pub retry_when_waiting: bool,
}
impl BackendError {
    pub fn blocked(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            delivery: None,
            retry_when_waiting: false,
        }
    }
    pub fn waiting(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            delivery: None,
            retry_when_waiting: true,
        }
    }
    fn retryable(&self) -> bool {
        self.retry_when_waiting && self.delivery != Some(Delivery::MayHaveRun)
    }
}
impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for BackendError {}
impl From<SessionError> for BackendError {
    fn from(error: SessionError) -> Self {
        let message = error.to_string();
        let (kind, delivery, retry_when_waiting) = match error {
            SessionError::Disabled => (ErrorKind::Disabled, None, false),
            SessionError::Busy => (ErrorKind::NotReady, None, false),
            SessionError::NotConnected => (ErrorKind::NotConnected, None, false),
            SessionError::Gate(_) | SessionError::NotOffline(_) => (ErrorKind::Gate, None, true),
            SessionError::Identity(_) => (ErrorKind::Identity, None, false),
            SessionError::Protocol(_) => (ErrorKind::Protocol, None, false),
            SessionError::Helper {
                code: ResponseCode::NotReady,
                may_have_run: false,
                ..
            } => (ErrorKind::NotReady, Some(Delivery::NotSent), true),
            SessionError::Helper {
                code: ResponseCode::OfflineRequired,
                may_have_run: false,
                ..
            } => (ErrorKind::Gate, Some(Delivery::NotSent), true),
            SessionError::Helper {
                code: ResponseCode::RestartRequired,
                may_have_run,
                ..
            } => (ErrorKind::RestartRequired, Some(if may_have_run { Delivery::MayHaveRun } else { Delivery::NotSent }), false),
            SessionError::Helper { may_have_run, .. } => (ErrorKind::Protocol, Some(if may_have_run { Delivery::MayHaveRun } else { Delivery::NotSent }), false),
            SessionError::Transport(error) => {
                let retry = error.delivery == Delivery::NotSent && matches!(error.failure, TransportFailure::Busy | TransportFailure::Missing);
                (ErrorKind::Transport, Some(error.delivery), retry)
            }
        };
        Self {
            kind,
            message,
            delivery,
            retry_when_waiting,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Disabled,
    Disconnected,
    WaitingForGame,
    WaitingForCharacter,
    Connecting,
    Ready,
    Blocked,
    Closing,
    Stopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Connecting,
    CapturingCatalog,
    ReadingForge,
    ReadingAttackLeech,
    Refreshing,
    WritingForge,
    WritingAttackLeech,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub read_forge: bool,
    pub read_catalog: bool,
    pub read_attack_leech: bool,
    /// Narrow operation support, not current observation eligibility or lifetime authority.
    pub attack_leech_rate: bool,
    pub write_forge: bool,
    /// Operation support on this ready connection. Current item eligibility and
    /// range/observation checks still belong to level_requirement_plan.
    pub level_requirement: bool,
    /// Per-field status support, independent of the selected item's eligibility.
    pub status_fields: [bool; 7],
    pub base_change: bool,
    pub identity_change: bool,
    pub affix_rolls: bool,
    pub affix_change: bool,
    pub affix_list: bool,
    pub hooks: bool,
    pub fields: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum CatalogState {
    Missing,
    Saved(Arc<GameCatalog>),
    Current { generation: u64, data: Arc<GameCatalog> },
    Stale(Arc<GameCatalog>),
}
impl CatalogState {
    pub fn data(&self) -> Option<&Arc<GameCatalog>> {
        match self {
            Self::Missing => None,
            Self::Saved(data) | Self::Current { data, .. } | Self::Stale(data) => Some(data),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum ForgeState {
    Unobserved,
    NoItem {
        generation: u64,
    },
    Item {
        generation: u64,
        snapshot: Arc<ForgeSnapshot>,
        resolved_name: Option<ResolvedItemName>,
        implicits: Option<Vec<ModifierSnapshot>>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperResidency {
    MayBeLoaded,
    Loaded,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedHelper {
    pub target: ProcessIdentity,
    pub residency: HelperResidency,
}
/// One owned read bound to its process, connection generation, admitted manager and monotonically
/// increasing publication sequence. Tokens and raw rates authorize no dereference or write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackLeechSnapshot {
    pub process: ProcessIdentity,
    pub generation: u64,
    pub sequence: u64,
    pub manager: u64,
    pub read: AttackLeechRead,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BackendSnapshot {
    pub revision: u64,
    pub generation: u64,
    pub enabled: bool,
    pub monitoring: bool,
    pub auto_connect: bool,
    pub override_enabled: bool,
    pub phase: Phase,
    pub activity: Option<Activity>,
    pub capture_progress: Option<CaptureProgress>,
    pub detected: Option<ProcessIdentity>,
    pub attached: Option<ProcessIdentity>,
    pub retained_helper: Option<RetainedHelper>,
    pub gate: Option<GateDecision>,
    pub cleanup: Cleanup,
    pub runtime_status: Option<RuntimeStatus>,
    pub error: Option<BackendError>,
    pub cache_error: Option<String>,
    pub catalog: CatalogState,
    pub forge: ForgeState,
    pub attack_leech: Option<AttackLeechSnapshot>,
    pub capabilities: Capabilities,
    pub last_write: Option<WriteRecord>,
    pub last_rate_write: Option<RateWriteRecord>,
    pub undo_forge: Option<crate::undo::ForgeUndo>,
}
impl BackendSnapshot {
    /// The outcome for this exact submitted plan. Historical certainty survives
    /// connection loss; publishing its observation still requires current scope.
    pub fn write_outcome(&self, plan: &WritePlan) -> Option<WriteOutcome<WriteObservation>> {
        match plan {
            WritePlan::Forge(plan) => self
                .last_write
                .as_ref()
                .filter(|r| &r.plan == plan.as_ref())
                .map(|r| r.status.clone().map(WriteObservation::forge)),
            WritePlan::Field(plan) => self
                .last_rate_write
                .as_ref()
                .filter(|r| &r.plan == plan.as_ref())
                .map(|r| r.status.clone().map(WriteObservation::Field)),
        }
    }
    /// Pure CURRENT-player intent from the latest exact owned observation.
    /// Caller must still submit explicitly; this provides no historical lifetime ticket.
    pub fn attack_leech_plan(&self, target: AttackLeechTarget, value: f64) -> Result<AttackLeechDecision, FieldPlanError> {
        let current = self.attack_leech.as_ref().ok_or(FieldPlanError::InvalidObservation)?;
        if self.phase != Phase::Ready
            || self.attached.as_ref() != Some(&current.process)
            || current.generation != self.generation
            || self
                .runtime_status
                .as_ref()
                .is_none_or(|status| status.online || status.manager == 0 || status.manager != current.manager)
        {
            return Err(FieldPlanError::StaleObservation);
        }
        AttackLeechPlan::prepare(current, target, value)
    }
    pub fn expected_observation(&self) -> Result<crate::edits::ExpectedObservation, crate::edits::EditError> {
        use crate::edits::{EditError, ExpectedObservation};
        let (
            CatalogState::Current { generation, data },
            ForgeState::Item {
                generation: item_generation,
                snapshot,
                ..
            },
            Some(target),
        ) = (&self.catalog, &self.forge, &self.attached)
        else {
            return Err(EditError::InvalidObservation);
        };
        if self.phase != Phase::Ready || *generation != self.generation || *item_generation != self.generation {
            return Err(EditError::IdentityChanged);
        }
        ExpectedObservation::new(self.generation, target.clone(), data.identity.clone(), snapshot.as_ref().clone())
    }
    pub fn level_requirement_plan(&self, value: i32) -> Result<ForgePlan, crate::edits::EditError> {
        self.status_plan(crate::edits::StatusField::LevelRequirement, value)
    }
    pub fn status_plan(&self, field: crate::edits::StatusField, value: i32) -> Result<ForgePlan, crate::edits::EditError> {
        self.status_edit_plan(crate::edits::StatusEdit {
            field,
            value,
            unrestricted: false,
        })
    }
    pub fn status_edit_plan(&self, edit: crate::edits::StatusEdit) -> Result<ForgePlan, crate::edits::EditError> {
        self.forge_plan(crate::edits::ItemEdit::Status(edit))
    }
    pub fn forge_plan(&self, edit: crate::edits::ItemEdit) -> Result<ForgePlan, crate::edits::EditError> {
        use crate::edits::{CatalogProvenance, EditError};
        let expected = self.expected_observation()?;
        let status = self
            .runtime_status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(EditError::OfflineRequired)?;
        let catalog = self.catalog.data().ok_or(EditError::CatalogNotVerified)?;
        ForgePlan::edit(
            &expected,
            status.manager,
            catalog,
            &CatalogProvenance::RuntimeRead {
                generation: self.generation,
                process: expected.process().clone(),
            },
            edit,
            self.override_enabled,
        )
    }
    pub fn forge_undo_plan(&self, undo: &crate::undo::ForgeUndo) -> Result<ForgePlan, crate::edits::EditError> {
        use crate::edits::{CatalogProvenance, EditError};
        if self.undo_forge.as_ref() != Some(undo) {
            return Err(EditError::IdentityChanged);
        }
        let current = self.expected_observation()?;
        let status = self
            .runtime_status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(EditError::OfflineRequired)?;
        undo.prepare_with_override(
            &current,
            status.manager,
            self.catalog.data().ok_or(EditError::CatalogNotVerified)?,
            &CatalogProvenance::RuntimeRead {
                generation: self.generation,
                process: current.process().clone(),
            },
            self.override_enabled,
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    Connected { generation: u64, target: ProcessIdentity },
    Released { generation: u64, cleanup: Cleanup },
    ReadPublished { generation: u64 },
    Fault(BackendError),
    CacheFault(String),
    Stopped,
}
#[derive(Clone, Debug, PartialEq, Eq)]
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
    ReadAttackLeech,
    Write(WritePlan),
    ApplyForge(Box<ForgePlan>),
    SetAttackLeechRate(Box<AttackLeechPlan>),
    UndoForge(Box<crate::undo::ForgeUndo>),
    Shutdown,
}
impl Command {
    fn barrier(&self) -> bool {
        matches!(self, Self::Disable | Self::Disconnect | Self::Shutdown | Self::SetAutoConnect(false))
    }
}

#[derive(Clone, Debug)]
pub struct BackendOptions {
    pub policy: GatePolicy,
    pub auto_connect: bool,
    pub override_enabled: bool,
    pub poll_interval: Duration,
    pub discovery_interval: Duration,
}
impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            policy: GatePolicy::Release,
            auto_connect: true,
            override_enabled: false,
            poll_interval: Duration::from_millis(250),
            discovery_interval: Duration::from_secs(1),
        }
    }
}

struct Envelope {
    command: Command,
    epoch: u64,
    reply: oneshot::Sender<Result<(), BackendError>>,
}
pub struct BackendHandle {
    commands: mpsc::Sender<Envelope>,
    snapshots: watch::Receiver<BackendSnapshot>,
    events: broadcast::Sender<BackendEvent>,
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
impl BackendHandle {
    pub fn snapshot(&self) -> BackendSnapshot {
        self.snapshots.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<BackendSnapshot> {
        self.snapshots.clone()
    }
    pub fn subscribe_events(&self) -> broadcast::Receiver<BackendEvent> {
        self.events.subscribe()
    }
    pub fn try_submit(&self, command: Command) -> Result<Pending, SubmitError> {
        let barrier = command.barrier();
        let (reply, receiver) = oneshot::channel();
        let mut epoch = self.intent.lock().unwrap_or_else(|p| p.into_inner());
        let accepted = if barrier { epoch.wrapping_add(1) } else { *epoch };
        self.commands.try_send(Envelope { command, epoch: accepted, reply }).map_err(|e| match e {
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
            events: self.events.clone(),
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
    cache: Option<Arc<CatalogCache>>,
    commands: mpsc::Receiver<Envelope>,
    snapshots: watch::Sender<BackendSnapshot>,
    events: broadcast::Sender<BackendEvent>,
    intent: Arc<Mutex<u64>>,
    state: BackendSnapshot,
    connection: Option<C::Connection>,
    blocked: Option<ProcessIdentity>,
    next_discovery: tokio::time::Instant,
    command_epoch: u64,
    attack_leech_sequence: u64,
    paused: bool,
}

pub fn service<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock>(
    locator: P,
    connector: C,
    log: L,
    clock: K,
    options: BackendOptions,
    cache: Option<CatalogCache>,
) -> (BackendHandle, BackendWorker<P, C, L, K>) {
    let state = BackendSnapshot {
        revision: 0,
        generation: 0,
        enabled: false,
        monitoring: false,
        auto_connect: options.auto_connect,
        override_enabled: options.override_enabled,
        phase: Phase::Disabled,
        activity: None,
        capture_progress: None,
        detected: None,
        attached: None,
        retained_helper: None,
        gate: None,
        cleanup: Cleanup::NotNeeded,
        runtime_status: None,
        error: None,
        cache_error: None,
        catalog: CatalogState::Missing,
        forge: ForgeState::Unobserved,
        attack_leech: None,
        capabilities: Capabilities::default(),
        last_write: None,
        last_rate_write: None,
        undo_forge: None,
    };
    let (sender, commands) = mpsc::channel(32);
    let (snapshots, receiver) = watch::channel(state.clone());
    let (events, _) = broadcast::channel(64);
    let intent = Arc::new(Mutex::new(0));
    (
        BackendHandle {
            commands: sender,
            snapshots: receiver,
            events: events.clone(),
            intent: intent.clone(),
            handles: Arc::new(AtomicUsize::new(1)),
        },
        BackendWorker {
            locator,
            connector,
            log,
            clock,
            options,
            cache: cache.map(Arc::new),
            commands,
            snapshots,
            events,
            intent,
            state,
            connection: None,
            blocked: None,
            next_discovery: tokio::time::Instant::now(),
            command_epoch: 0,
            attack_leech_sequence: 0,
            paused: false,
        },
    )
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub async fn run(mut self) {
        self.load_cache().await;
        let interval = self.options.poll_interval.max(Duration::from_millis(1));
        let mut tick = tokio::time::Instant::now() + interval;
        loop {
            match tokio::time::timeout_at(tick, self.commands.recv()).await {
                Ok(Some(envelope)) => {
                    let stop = envelope.command == Command::Shutdown;
                    self.command_epoch = envelope.epoch;
                    let result = self.command(envelope.command).await;
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
                    self.poll().await;
                    tick = tokio::time::Instant::now() + interval;
                }
            }
        }
    }
    fn epoch(&self) -> u64 {
        *self.intent.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn superseded(&self, epoch: u64) -> Result<(), BackendError> {
        if epoch != self.epoch() {
            Err(BackendError::blocked(ErrorKind::Superseded, "operation superseded by queued cleanup"))
        } else {
            Ok(())
        }
    }
    fn publish(&mut self) {
        if self.state.attack_leech.as_ref().is_some_and(|observation| {
            self.state.phase != Phase::Ready
                || observation.generation != self.state.generation
                || self.state.attached.as_ref() != Some(&observation.process)
                || self
                    .state
                    .runtime_status
                    .as_ref()
                    .is_none_or(|status| status.online || status.manager == 0 || status.manager != observation.manager)
        }) {
            self.state.attack_leech = None;
        }
        if self.state.undo_forge.as_ref().is_some_and(|undo| {
            self.state.expected_observation().as_ref() != Ok(undo.after())
                || self.state.runtime_status.as_ref().is_none_or(|status| status.online || status.manager != undo.manager())
        }) {
            self.state.undo_forge = None;
        }
        self.state.revision = self.state.revision.wrapping_add(1);
        self.state.capabilities = Capabilities {
            read_forge: self.state.phase == Phase::Ready,
            read_catalog: self.state.phase == Phase::Ready,
            read_attack_leech: self.state.phase == Phase::Ready
                && self.state.activity.is_none()
                && self.connection.as_ref().is_some_and(BackendConnection::supports_read_attack_leech),
            attack_leech_rate: self.state.phase == Phase::Ready
                && self.state.activity.is_none()
                && self.connection.as_ref().is_some_and(BackendConnection::supports_attack_leech_rate),
            level_requirement: self.state.phase == Phase::Ready
                && self.state.activity.is_none()
                && self.connection.as_ref().is_some_and(BackendConnection::supports_level_requirement),
            base_change: self.state.phase == Phase::Ready && self.state.activity.is_none() && self.connection.as_ref().is_some_and(BackendConnection::supports_base_change),
            identity_change: self.state.phase == Phase::Ready && self.state.activity.is_none() && self.connection.as_ref().is_some_and(BackendConnection::supports_identity_change),
            affix_list: self.state.phase == Phase::Ready && self.state.activity.is_none() && self.connection.as_ref().is_some_and(BackendConnection::supports_affix_list),
            affix_change: self.state.phase == Phase::Ready && self.state.activity.is_none() && self.connection.as_ref().is_some_and(BackendConnection::supports_affix_change),
            affix_rolls: self.state.phase == Phase::Ready && self.state.activity.is_none() && self.connection.as_ref().is_some_and(BackendConnection::supports_affix_rolls),
            status_fields: std::array::from_fn(|index| {
                self.state.phase == Phase::Ready
                    && self.state.activity.is_none()
                    && self
                        .connection
                        .as_ref()
                        .is_some_and(|connection| connection.supports_status(crate::edits::StatusField::from_wire(index as u32).expect("seven status fields")))
            }),
            ..Capabilities::default()
        };
        self.snapshots.send_replace(self.state.clone());
    }
    fn publish_if_current(&mut self, epoch: u64) -> Result<(), BackendError> {
        let intent = self.intent.clone();
        let guard = intent.lock().unwrap_or_else(|p| p.into_inner());
        if *guard != epoch {
            return Err(BackendError::blocked(ErrorKind::Superseded, "operation superseded by queued cleanup"));
        }
        self.publish();
        Ok(())
    }
    fn commit(&mut self, epoch: u64, change: impl FnOnce(&mut Self)) -> Result<(), BackendError> {
        let intent = self.intent.clone();
        let guard = intent.lock().unwrap_or_else(|p| p.into_inner());
        if *guard != epoch {
            return Err(BackendError::blocked(ErrorKind::Superseded, "operation superseded by queued cleanup"));
        }
        change(self);
        self.publish();
        Ok(())
    }
    fn event(&self, event: BackendEvent) {
        let _ = self.events.send(event);
    }
    async fn load_cache(&mut self) {
        if let Some(cache) = self.cache.clone() {
            match tokio::task::spawn_blocking(move || cache.load_last()).await {
                Ok(Ok(Some(data))) => self.state.catalog = CatalogState::Saved(Arc::new(data)),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => self.cache_fault(e.to_string()),
                Err(e) => self.cache_fault(e.to_string()),
            }
            self.publish();
        }
    }
    fn cache_fault(&mut self, message: String) {
        self.state.cache_error = Some(message.clone());
        self.event(BackendEvent::CacheFault(message));
    }
    async fn save_cache(&mut self, data: Arc<GameCatalog>) {
        if let Some(cache) = self.cache.clone() {
            match tokio::task::spawn_blocking(move || cache.save(&data)).await {
                Ok(Ok(())) => self.state.cache_error = None,
                Ok(Err(e)) => self.cache_fault(e.to_string()),
                Err(e) => self.cache_fault(e.to_string()),
            }
        }
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
            Command::RefreshCatalog => self.read(true).await,
            Command::ReadForge => self.read(false).await,
            Command::ReadAttackLeech => self.read_attack_leech().await,
            Command::Write(WritePlan::Forge(plan)) => self.write_forge(*plan).await,
            Command::Write(WritePlan::Field(plan)) => self.set_attack_leech_rate(*plan).await,
            Command::SetAttackLeechRate(plan) => self.set_attack_leech_rate(*plan).await,
            Command::ApplyForge(plan) => self.write_forge(*plan).await,
            Command::UndoForge(undo) => self.undo_forge(*undo).await,
        }
    }
    async fn poll(&mut self) {
        if !self.state.enabled || !self.state.monitoring || self.paused {
            return;
        }
        if self.connection.is_some() {
            let _ = self.read(false).await;
        } else if self.state.auto_connect && tokio::time::Instant::now() >= self.next_discovery {
            self.next_discovery = tokio::time::Instant::now() + self.options.discovery_interval.max(Duration::from_millis(1));
            let _ = self.connect().await;
        }
    }
    fn admit(&mut self, target: &ProcessIdentity) -> Result<(), BackendError> {
        self.admit_using_policy(target, self.options.policy)
    }
    fn admit_using_policy(&mut self, target: &ProcessIdentity, policy: GatePolicy) -> Result<(), BackendError> {
        let started = Timestamp::from_nanosecond((i128::from(target.created) - 116_444_736_000_000_000) * 100)
            .map_err(|_| BackendError::blocked(ErrorKind::Identity, "invalid process creation time"))?;
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(BackendError::blocked(ErrorKind::Identity, "invalid process identity"));
        }
        let decision = gate::evaluate(&self.log.read(started), started, self.clock.now(), policy);
        self.state.gate = Some(decision);
        match decision.verdict {
            GateVerdict::Allow | GateVerdict::Probe => Ok(()),
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
                    message: format!("offline admission: {:?}", decision.readiness),
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
        if self.connection.is_some() {
            return self.read(false).await;
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
        if targets.len() != 1 {
            self.state.detected = None;
            self.state.phase = Phase::WaitingForGame;
            self.state.error = (targets.len() > 1).then(|| BackendError::waiting(ErrorKind::MultipleProcesses, "close extra matching game instances"));
            self.publish();
            return Ok(());
        }
        let target = targets.into_iter().next().unwrap();
        self.state.detected = Some(target.clone());
        if self.blocked.as_ref() == Some(&target) {
            self.state.phase = Phase::Blocked;
            self.publish();
            return Ok(());
        }
        self.admit(&target)?;
        self.state.generation = self.state.generation.wrapping_add(1);
        self.state.phase = Phase::Connecting;
        self.state.activity = Some(Activity::Connecting);
        self.state.capture_progress = None;
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
        self.state.attached = Some(target.clone());
        self.state.retained_helper = Some(RetainedHelper {
            target: target.clone(),
            residency: HelperResidency::Loaded,
        });
        self.state.cleanup = Cleanup::Unconfirmed;
        self.superseded(epoch)?;
        self.initialize(epoch, &target).await
    }
    async fn initialize(&mut self, epoch: u64, target: &ProcessIdentity) -> Result<(), BackendError> {
        let result: Result<(), BackendError> = async {
            self.admit(target)?;
            let status = self.connection.as_mut().unwrap().refresh(self.clock.now()).await?;
            self.superseded(epoch)?;
            self.check_status(status)?;
            self.state.activity = Some(Activity::CapturingCatalog);
            self.publish_if_current(epoch)?;
            let catalog = self.capture_observed(epoch).await?;
            self.superseded(epoch)?;
            self.admit(target)?;
            self.state.activity = Some(Activity::ReadingForge);
            self.publish_if_current(epoch)?;
            let forge = self.connection.as_mut().unwrap().read_forge(self.clock.now()).await?;
            self.superseded(epoch)?;
            self.admit(target)?;
            let implicits = match &forge {
                Some(item) => self.connection.as_mut().unwrap().describe_implicits(self.clock.now(), item).await?,
                None => None,
            };
            self.superseded(epoch)?;
            self.admit(target)?;
            let data = Arc::new(catalog);
            let mut forge = self.forge_state(forge, &data);
            if let ForgeState::Item { implicits: observed, .. } = &mut forge {
                *observed = implicits;
            }
            self.commit(epoch, |this| {
                this.state.forge = forge;
                this.state.catalog = CatalogState::Current {
                    generation: this.state.generation,
                    data: data.clone(),
                };
                this.state.phase = Phase::Ready;
                this.state.activity = None;
                this.state.capture_progress = None;
                this.state.error = None;
            })?;
            self.event(BackendEvent::Connected {
                generation: self.state.generation,
                target: target.clone(),
            });
            self.save_cache(data).await;
            self.publish_if_current(epoch)?;
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.handle_connection_gate(error);
            if error.kind != ErrorKind::Superseded && error.kind != ErrorKind::Gate {
                self.fail(error.clone(), Some(target.clone()));
            }
        }
        result
    }
    fn check_status(&mut self, status: RuntimeStatus) -> Result<(), BackendError> {
        if self.state.attack_leech.as_ref().is_some_and(|observation| observation.manager != status.manager) {
            self.state.attack_leech = None;
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
    async fn read(&mut self, capture: bool) -> Result<(), BackendError> {
        if !self.state.enabled {
            return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
        }
        if self.state.phase != Phase::Ready {
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
            self.state.activity = Some(if capture { Activity::CapturingCatalog } else { Activity::Refreshing });
            self.publish_if_current(epoch)?;
            let status = self
                .connection
                .as_mut()
                .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
                .refresh(self.clock.now())
                .await?;
            self.superseded(epoch)?;
            self.check_status(status)?;
            let data = if capture {
                let data = self.capture_observed(epoch).await?;
                self.superseded(epoch)?;
                Arc::new(data)
            } else {
                self.state
                    .catalog
                    .data()
                    .cloned()
                    .ok_or_else(|| BackendError::blocked(ErrorKind::Protocol, "current catalog missing"))?
            };
            self.admit(&target)?;
            let forge = self.connection.as_mut().unwrap().read_forge(self.clock.now()).await?;
            self.superseded(epoch)?;
            self.admit(&target)?;
            let implicits = match &forge {
                Some(item) => self.connection.as_mut().unwrap().describe_implicits(self.clock.now(), item).await?,
                None => None,
            };
            self.superseded(epoch)?;
            self.admit(&target)?;
            let mut forge = self.forge_state(forge, &data);
            if let ForgeState::Item { implicits: observed, .. } = &mut forge {
                *observed = implicits;
            }
            self.commit(epoch, |this| {
                this.state.forge = forge;
                this.state.catalog = CatalogState::Current {
                    generation: this.state.generation,
                    data: data.clone(),
                };
                this.state.activity = None;
                this.state.capture_progress = None;
                this.state.error = None;
            })?;
            self.event(BackendEvent::ReadPublished {
                generation: self.state.generation,
            });
            if capture {
                self.save_cache(data).await;
                self.publish_if_current(epoch)?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.handle_connection_gate(error);
            if error.kind != ErrorKind::Superseded && error.kind != ErrorKind::Gate {
                self.fail(error.clone(), Some(target));
            }
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
                    snapshot: Arc::new(snapshot),
                    resolved_name,
                    implicits: None,
                }
            }
        }
    }
    async fn capture_observed(&mut self, epoch: u64) -> Result<GameCatalog, BackendError> {
        let now = self.clock.now();
        let outcome = {
            let mut observer = CapturePublication {
                state: &mut self.state,
                snapshots: &self.snapshots,
                intent: &self.intent,
                epoch,
            };
            self.connection
                .as_mut()
                .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
                .capture_catalog_observed(now, &mut observer)
                .await?
        };
        match outcome {
            CaptureOutcome::Complete(catalog) => Ok(*catalog),
            CaptureOutcome::Stopped => {
                self.superseded(epoch)?;
                Err(BackendError::blocked(ErrorKind::Protocol, "capture stopped without a cleanup barrier"))
            }
        }
    }
    fn handle_connection_gate(&mut self, error: &BackendError) {
        if error.kind == ErrorKind::Gate && self.connection.is_some() {
            self.release_without_io();
            self.state.phase = Phase::WaitingForCharacter;
            self.state.error = Some(error.clone());
            self.publish();
        }
    }
    fn release_without_io(&mut self) {
        self.state.attack_leech = None;
        self.state.undo_forge = None;
        if self.connection.take().is_some() {
            self.state.cleanup = Cleanup::Unconfirmed;
        }
        self.state.attached = None;
        self.state.runtime_status = None;
        self.state.activity = None;
        self.state.capture_progress = None;
        self.state.forge = ForgeState::Unobserved;
        if let CatalogState::Current { data, .. } = &self.state.catalog {
            self.state.catalog = CatalogState::Stale(data.clone());
        }
        self.state.generation = self.state.generation.wrapping_add(1);
    }
    fn fail(&mut self, error: BackendError, target: Option<ProcessIdentity>) {
        self.release_without_io();
        if error.retryable() {
            self.state.phase = Phase::WaitingForGame;
        } else {
            self.blocked = target;
            self.state.phase = Phase::Blocked;
        }
        self.state.error = Some(error.clone());
        self.event(BackendEvent::Fault(error));
        self.publish();
    }
    async fn close(&mut self, disable: bool, stop: bool) {
        self.state.phase = Phase::Closing;
        self.state.activity = None;
        self.state.capture_progress = None;
        self.state.forge = ForgeState::Unobserved;
        self.publish();
        if let Some(mut connection) = self.connection.take() {
            self.state.cleanup = connection.disconnect().await;
        }
        self.release_without_io();
        if disable {
            self.state.enabled = false;
        }
        self.state.phase = if stop {
            Phase::Stopped
        } else if self.state.enabled {
            Phase::Disconnected
        } else {
            Phase::Disabled
        };
        self.publish();
        self.event(BackendEvent::Released {
            generation: self.state.generation,
            cleanup: self.state.cleanup,
        });
        if stop {
            self.event(BackendEvent::Stopped);
        }
    }
}

struct CapturePublication<'a> {
    state: &'a mut BackendSnapshot,
    snapshots: &'a watch::Sender<BackendSnapshot>,
    intent: &'a Mutex<u64>,
    epoch: u64,
}
impl CaptureObserver for CapturePublication<'_> {
    fn control(&mut self) -> CaptureControl {
        if *self.intent.lock().unwrap_or_else(|p| p.into_inner()) == self.epoch {
            CaptureControl::Continue
        } else {
            CaptureControl::Stop
        }
    }
    fn progress(&mut self, progress: CaptureProgress) {
        let epoch = self.intent.lock().unwrap_or_else(|p| p.into_inner());
        if *epoch != self.epoch {
            return;
        }
        self.state.capture_progress = Some(progress);
        self.state.revision = self.state.revision.wrapping_add(1);
        self.snapshots.send_replace(self.state.clone());
    }
}
