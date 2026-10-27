//! Application-owned, serialized trainer service with explicit write admission.
//!
//! Construction does not spawn, discover, or access files/processes. The owner
//! explicitly runs the worker on a background Tokio runtime. All accepted work
//! runs to completion; dropping an ordinary receipt is not cancellation. Optional
//! mapping reads have a separate disposable receipt and stop between RPCs. Cleanup barriers
//! suppress publication from an earlier operation, then drain it before cleanup.
//! Watch snapshots are authoritative; broadcast events can lag and are advisory.
mod faults;
mod fields;
mod mapping;
mod profiles;
mod rate_write;
mod unique;
mod write;

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
    fields::{AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate, RateUnavailable},
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    mapping::ModifierSnapshot,
    process::ProcessIdentity,
    protocol::RuntimeStatus,
};
use eg_trainer_wire::{
    protocol::ResponseCode,
    transport::{Transport, TransportError, TransportFailure},
};
pub use faults::forge_record;
use jiff::Timestamp;
pub use profiles::{HOOK_NOT_AVAILABLE, HookRequest, HookRun, ProfileLock};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    catalog::{GameCatalog, ResolvedItemName},
    features::{FeatureIntents, HookId},
    field_writes::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, FieldPlanError, RateWriteRecord, RateWriteStatus},
    game_data::{GameData, GameDataHandle},
    gate::{self, GateDecision, GatePolicy, GateVerdict, LogSource},
    mapping::{MappingRequest, ValueTable},
    profiles::{ProfileBook, ProfileKey, ProfileSaver},
    session::{Cleanup, Session, SessionError},
    writes::{ForgePlan, WriteControl, WriteObservation, WriteOutcome, WritePlan, WriteRecord, WriteStatus},
};

pub trait ProcessLocator: Send + 'static {
    /// Query identities only. A matching filename alone is not an attachment.
    fn find(&mut self) -> impl Future<Output = Result<Vec<ProcessIdentity>, BackendError>> + Send;
}

pub trait Connector: Send + 'static {
    type Connection: BackendConnection;
    /// Release local resources for processes whose exit was proved, without remote I/O.
    fn reap_exited(&mut self) -> Vec<ProcessIdentity> {
        Vec::new()
    }
    /// Called only after the backend's process-bound log gate allows access.
    /// Implementations must repeat admission after preparation and bind the exact
    /// identity/build/helper. Existing retained modules must never be reinjected.
    fn connect(&mut self, target: ProcessIdentity, now: Timestamp) -> impl Future<Output = Result<Self::Connection, BackendError>> + Send;
}

pub const HELPER_RESTART_REQUIRED: &str = "The helper could not leave the game; restart Last Epoch before playing online.";

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
    fn read_locale(&mut self, now: Timestamp) -> impl Future<Output = Result<crate::catalog::GameLocale, BackendError>> + Send;
    /// The running build's binary and metadata hashes: the game data read from files with the same hashes describes
    /// this game. Definitions are never captured from the game.
    fn build_hashes(&self) -> (String, String);
    fn read_forge(&mut self, now: Timestamp) -> impl Future<Output = Result<Option<ForgeSnapshot>, BackendError>> + Send;
    fn describe_implicits(&mut self, _: Timestamp, _: &ForgeSnapshot) -> impl Future<Output = Result<Option<Vec<ModifierSnapshot>>, BackendError>> + Send {
        async { Ok(None) }
    }
    fn describe_unique(&mut self, _: Timestamp, _: &ForgeSnapshot) -> impl Future<Output = Result<Option<Vec<ModifierSnapshot>>, BackendError>> + Send {
        async { Ok(None) }
    }
    fn preview_unique(
        &mut self,
        _: Timestamp,
        _: &crate::edits::ExpectedObservation,
        _: &crate::edits::RollEdit,
        _: &crate::mapping::BoundTooltip,
    ) -> impl Future<Output = Result<crate::mapping::BoundPreview, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::MappingUnavailable, "Unique preview is unavailable.")) }
    }
    fn resolve_mapping(&mut self, _: Timestamp, _: &crate::edits::ExpectedObservation, _: &MappingRequest) -> impl Future<Output = Result<ValueTable, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::MappingUnavailable, "Actual values are unavailable for this modifier.")) }
    }
    /// Optional reads may stop between completed exchanges. Implementations
    /// must drain an in-flight exchange and preserve fatal transport failures.
    fn resolve_mapping_observed(
        &mut self,
        now: Timestamp,
        expected: &crate::edits::ExpectedObservation,
        request: &MappingRequest,
        observer: &mut dyn ReadObserver,
    ) -> impl Future<Output = Result<ValueTable, BackendError>> + Send {
        async move {
            if observer.control() == ReadControl::Stop {
                return Err(BackendError::blocked(ErrorKind::Superseded, "Value request canceled."));
            }
            self.resolve_mapping(now, expected, request).await
        }
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
    /// Whether this connection implements the feature's native lifecycle.
    fn supports_hook(&self, _: HookId) -> bool {
        false
    }
    fn observed_hook(&self, _: HookId) -> Option<bool> {
        None
    }
    /// Sets the hook as `request` asks, then answers with the game's own report of it: `Confirmed(on)` when it matches
    /// the request, `NotKept(on)` when it does not.
    fn set_hook(&mut self, _: Timestamp, _: &HookRequest, _: &mut dyn WriteControl) -> impl Future<Output = Result<WriteOutcome<bool>, BackendError>> + Send {
        async { Ok(WriteOutcome::refused(HOOK_NOT_AVAILABLE)) }
    }
    fn edit_forge(&mut self, _: Timestamp, _: &ForgePlan, _: &mut dyn WriteControl) -> impl Future<Output = Result<WriteStatus, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "level requirement writes are unavailable")) }
    }
    fn disconnect(&mut self) -> impl Future<Output = Cleanup> + Send;
    // Dropping a connection performs no I/O. After admission loss the worker
    // drains only owner shutdown and read-only native departure evidence.
}

impl<T: Transport + Send + 'static> BackendConnection for Session<T> {
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
        self.refresh_gameplay_hooks().await?;
        self.runtime_status()
            .copied()
            .ok_or_else(|| BackendError::blocked(ErrorKind::Protocol, "missing runtime status"))
    }
    async fn read_locale(&mut self, now: Timestamp) -> Result<crate::catalog::GameLocale, BackendError> {
        Ok(Session::read_locale(self, now).await?)
    }
    fn build_hashes(&self) -> (String, String) {
        (self.profile().binary_hash().to_owned(), self.profile().metadata_hash().to_owned())
    }
    async fn describe_implicits(&mut self, now: Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(Session::describe_implicits(self, now, expected).await?)
    }
    async fn describe_unique(&mut self, now: Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(Session::describe_unique(self, now, expected).await?)
    }
    async fn preview_unique(
        &mut self,
        now: Timestamp,
        expected: &crate::edits::ExpectedObservation,
        draft: &crate::edits::RollEdit,
        evidence: &crate::mapping::BoundTooltip,
    ) -> Result<crate::mapping::BoundPreview, BackendError> {
        Ok(Session::preview_unique(self, now, expected, draft, evidence).await?)
    }
    async fn resolve_mapping(&mut self, now: Timestamp, expected: &crate::edits::ExpectedObservation, request: &MappingRequest) -> Result<ValueTable, BackendError> {
        Ok(Session::resolve_mapping(self, now, expected, request).await?)
    }
    async fn resolve_mapping_observed(
        &mut self,
        now: Timestamp,
        expected: &crate::edits::ExpectedObservation,
        request: &MappingRequest,
        observer: &mut dyn ReadObserver,
    ) -> Result<ValueTable, BackendError> {
        Ok(Session::resolve_mapping_observed(self, now, expected, request, observer).await?)
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
    fn supports_hook(&self, id: HookId) -> bool {
        eg_trainer_wire::hooks::GameplayHook::from_id(id.as_str()).is_some()
    }
    fn observed_hook(&self, id: HookId) -> Option<bool> {
        eg_trainer_wire::hooks::GameplayHook::from_id(id.as_str())
            .and_then(|hook| self.hook_observation(hook))
            .map(|actual| actual.enabled)
    }
    async fn set_hook(&mut self, _: Timestamp, request: &HookRequest, control: &mut dyn WriteControl) -> Result<WriteOutcome<bool>, BackendError> {
        let Some(hook) = eg_trainer_wire::hooks::GameplayHook::from_id(request.id.as_str()) else {
            return Ok(WriteOutcome::refused(HOOK_NOT_AVAILABLE));
        };
        let value = match (hook, request.values.as_slice()) {
            (eg_trainer_wire::hooks::GameplayHook::MysteriousRift, [mode]) if *mode == 1.0 => 1,
            (eg_trainer_wire::hooks::GameplayHook::MysteriousRift, [mode]) if *mode == 2.0 => 0,
            (eg_trainer_wire::hooks::GameplayHook::MysteriousRift, [mode]) if *mode == 0.0 && !request.on => 1,
            (eg_trainer_wire::hooks::GameplayHook::Offhand, []) => 1,
            (eg_trainer_wire::hooks::GameplayHook::IncomingDamage, [percent]) if percent.is_finite() && (0.0..=100.0).contains(percent) && percent.fract() == 0.0 => {
                *percent as u64
            }
            (eg_trainer_wire::hooks::GameplayHook::DamageDealt, [multiplier]) if multiplier.is_finite() && ((0.0..=100.0).contains(multiplier) || *multiplier == 101.0) => {
                multiplier.to_bits()
            }
            _ => return Ok(WriteOutcome::refused("Invalid hook selection")),
        };
        Ok(self.set_gameplay_hook_with_value(hook, request.on, value, control).await?)
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
    HelperDeparted,
    CharacterLoading,
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
    MappingUnavailable,
    MappingRetryable,
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
            SessionError::CharacterLoading => (ErrorKind::CharacterLoading, Some(Delivery::NotSent), true),
            SessionError::Busy => (ErrorKind::NotReady, None, false),
            SessionError::NotConnected => (ErrorKind::NotConnected, None, false),
            SessionError::NotOffline(_) => (ErrorKind::Gate, None, true),
            SessionError::Identity(_) => (ErrorKind::Identity, None, false),
            SessionError::Protocol(_) => (ErrorKind::Protocol, None, false),
            SessionError::Mapping(_) => (ErrorKind::MappingUnavailable, None, false),
            SessionError::MappingRetryable(_) => (ErrorKind::MappingRetryable, None, false),
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
            SessionError::Transport(TransportError {
                failure: TransportFailure::Departed,
                delivery,
            }) => (ErrorKind::HelperDeparted, Some(delivery), true),
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
    /// Waiting for the game data service (a changed install is read again).
    ReadingGameFiles,
    ReadingForge,
    ReadingAttackLeech,
    Refreshing,
    WritingForge,
    WritingAttackLeech,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// In GameplayHook order; availability comes from this connection, not saved intent.
    pub gameplay_hooks: [bool; 4],
    pub read_forge: bool,
    pub read_attack_leech: bool,
    /// Narrow operation support, not current observation eligibility or lifetime authority.
    pub attack_leech_rate: bool,
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
}
#[derive(Clone, Debug, PartialEq)]
pub enum CatalogState {
    Missing,
    Current { generation: u64, data: Arc<GameCatalog> },
    Stale(Arc<GameCatalog>),
}
impl CatalogState {
    pub fn data(&self) -> Option<&Arc<GameCatalog>> {
        match self {
            Self::Missing => None,
            Self::Current { data, .. } | Self::Stale(data) => Some(data),
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
        unique: Option<Result<Vec<ModifierSnapshot>, String>>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperResidency {
    MayBeLoaded,
    Loaded,
    Leaving,
    Gone,
    Stuck,
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
/// The game data service's state (`crate::game_data`), for the views. A session uses its catalog when it runs the build
/// the files hold; without it the trainer does not open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameFiles {
    /// No game folder is known yet.
    NotConfigured,
    Reading,
    Ready {
        affixes: usize,
        build: String,
        checked_at: Option<Timestamp>,
        updated_at: Option<Timestamp>,
    },
    Unavailable(String),
}
impl From<&GameData> for GameFiles {
    fn from(data: &GameData) -> Self {
        match data {
            GameData::NoGame => Self::NotConfigured,
            GameData::Reading => Self::Reading,
            GameData::Ready(catalog) => Self::Ready {
                affixes: catalog.affix_count(),
                build: catalog.build.clone(),
                checked_at: catalog.checked_at,
                updated_at: catalog.updated_at,
            },
            GameData::Unavailable(reason) => Self::Unavailable(reason.clone()),
        }
    }
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
    pub detected: Option<ProcessIdentity>,
    pub attached: Option<ProcessIdentity>,
    pub retained_helper: Option<RetainedHelper>,
    pub gate: Option<GateDecision>,
    pub cleanup: Cleanup,
    pub runtime_status: Option<RuntimeStatus>,
    pub error: Option<BackendError>,
    pub catalog: CatalogState,
    pub game_files: GameFiles,
    pub forge: ForgeState,
    /// Successful owned reads/captures, independent of item equality. This
    /// allows optional display failures to be retried after Refresh, not in a loop.
    pub forge_read_sequence: u64,
    pub mappings: Vec<ResolvedMapping>,
    pub unique_preview: Option<ResolvedPreview>,
    pub attack_leech: Option<AttackLeechSnapshot>,
    pub capabilities: Capabilities,
    pub last_write: Option<WriteRecord>,
    pub last_rate_write: Option<RateWriteRecord>,
    pub undo_forge: Option<crate::undo::ForgeUndo>,
    pub undo_forge_depth: usize,
    pub undo_forge_needs_override: bool,
    /// Every kept cheat profile, the last played first.
    pub profiles: Arc<ProfileBook>,
    /// The character the pages show: the profile chosen while no game runs, or the character in play, whose hooks are
    /// its profile's or, without one, `unsaved`.
    pub editing: Option<ProfileKey>,
    /// The hooks of a character in play without a profile: they last this connection unless kept as its profile.
    pub unsaved: Option<Arc<FeatureIntents>>,
    pub profile_lock: ProfileLock,
    /// The live profile's hooks in this connection, every hook by ID; empty while no character is in play.
    pub hooks: BTreeMap<HookId, HookRun>,
}
/// Latest optional preview; matching requires both the full observation and exact draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPreview {
    pub expected: crate::edits::ExpectedObservation,
    pub draft: crate::edits::RollEdit,
    pub result: Result<crate::mapping::BoundPreview, String>,
}
/// Optional display data for one complete observed item and exact draft query.
/// Kept in memory only; a changed item, catalog or connection discards it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMapping {
    pub expected: crate::edits::ExpectedObservation,
    pub request: MappingRequest,
    pub result: Result<Arc<ValueTable>, String>,
    /// Unclassified/transient helper refusal may be retried after the next
    /// successful owned read. Stable capability failures survive monitor polls.
    pub retry_on_read: bool,
}
impl BackendSnapshot {
    /// The hooks the pages show: the shown profile's, or those of the character in play for this connection.
    pub fn shown_hooks(&self) -> Option<&FeatureIntents> {
        self.editing
            .as_ref()
            .and_then(|key| self.profiles.get(key))
            .map(|profile| &profile.hooks)
            .or(self.unsaved.as_deref())
    }
    pub fn mapping(&self, expected: &crate::edits::ExpectedObservation, request: &MappingRequest) -> Option<&Result<Arc<ValueTable>, String>> {
        self.mappings
            .iter()
            .find(|mapping| mapping.expected.matches(expected) && &mapping.request == request)
            .map(|mapping| &mapping.result)
    }
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
        use crate::edits::EditError;
        let expected = self.expected_observation()?;
        let status = self
            .runtime_status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(EditError::OfflineRequired)?;
        let catalog = self.catalog.data().ok_or(EditError::CatalogNotVerified)?;
        ForgePlan::edit(&expected, status.manager, catalog, edit, self.override_enabled)
    }
    pub fn forge_undo_plan(&self, undo: &crate::undo::ForgeUndo) -> Result<ForgePlan, crate::edits::EditError> {
        use crate::edits::EditError;
        if self.undo_forge.as_ref() != Some(undo) {
            return Err(EditError::IdentityChanged);
        }
        let current = self.expected_observation()?;
        let status = self
            .runtime_status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(EditError::OfflineRequired)?;
        undo.prepare_with_override(&current, status.manager, self.catalog.data().ok_or(EditError::CatalogNotVerified)?, self.override_enabled)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    Connected { generation: u64, target: ProcessIdentity },
    Released { generation: u64, cleanup: Cleanup },
    ReadPublished { generation: u64 },
    Fault(BackendError),
    Stopped,
}
/// The game's files changed after it started: it may run an older build than they hold.
fn files_changed() -> BackendError {
    BackendError::blocked(
        ErrorKind::Identity,
        "Last Epoch's files changed after it started (a game update?). Restart the game to use the trainer.",
    )
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
        expected: Box<crate::edits::ExpectedObservation>,
    },
    PreviewUnique {
        expected: Box<crate::edits::ExpectedObservation>,
        draft: crate::edits::RollEdit,
    },
    ResolveMapping {
        expected: Box<crate::edits::ExpectedObservation>,
        request: MappingRequest,
    },
    ReadAttackLeech,
    BeginForgeBatch,
    Write(WritePlan),
    ApplyForge(Box<ForgePlan>),
    SetAttackLeechRate(Box<AttackLeechPlan>),
    UndoForge(Box<crate::undo::ForgeUndo>),
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
    pub policy: GatePolicy,
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
}
impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            policy: GatePolicy::Release,
            auto_connect: true,
            override_enabled: false,
            poll_interval: Duration::from_millis(250),
            discovery_interval: Duration::from_secs(1),
            locale_interval: Duration::from_secs(2),
            game_data: None,
            profiles: ProfileBook::default(),
            save_profiles: None,
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
    pub fn subscribe(&self) -> watch::Receiver<BackendSnapshot> {
        self.snapshots.clone()
    }
    pub fn subscribe_events(&self) -> broadcast::Receiver<BackendEvent> {
        self.events.subscribe()
    }
    pub fn try_submit(&self, command: Command) -> Result<Pending, SubmitError> {
        self.submit(command, None)
    }
    pub fn try_resolve_mapping(&self, expected: crate::edits::ExpectedObservation, request: MappingRequest) -> Result<MappingRead, SubmitError> {
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
    commands: mpsc::Receiver<Envelope>,
    snapshots: watch::Sender<BackendSnapshot>,
    events: broadcast::Sender<BackendEvent>,
    intent: Arc<Mutex<u64>>,
    state: BackendSnapshot,
    connection: Option<C::Connection>,
    retiring: Option<C::Connection>,
    blocked: Option<ProcessIdentity>,
    next_discovery: tokio::time::Instant,
    next_locale_check: tokio::time::Instant,
    command_epoch: u64,
    read_cancel: Option<Arc<AtomicBool>>,
    attack_leech_sequence: u64,
    paused: bool,
    faults: Option<eg_core::fault::FaultLog>,
    forge_history: crate::undo::ForgeUndoHistory,
    /// The game data service's state, mirrored into `state.game_files`.
    game_data: Option<watch::Receiver<GameData>>,
    /// Whether the last look for the game found one running (any number of them).
    games_running: bool,
    /// The offline character the last admission let in, as Player.log names it.
    character: Option<eg_core::game_log::LoggedCharacter>,
    /// The profile of the character in play in this connection.
    live: Option<ProfileKey>,
}

pub fn service<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock>(
    locator: P,
    connector: C,
    log: L,
    clock: K,
    options: BackendOptions,
) -> (BackendHandle, BackendWorker<P, C, L, K>) {
    let game_data = options.game_data.as_ref().map(GameDataHandle::subscribe);
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
        attack_leech: None,
        capabilities: Capabilities::default(),
        last_write: None,
        last_rate_write: None,
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
            commands,
            snapshots,
            events,
            intent,
            state,
            connection: None,
            retiring: None,
            blocked: None,
            next_discovery: tokio::time::Instant::now(),
            next_locale_check: tokio::time::Instant::now(),
            command_epoch: 0,
            read_cancel: None,
            attack_leech_sequence: 0,
            paused: false,
            faults: None,
            forge_history: Default::default(),
            game_data,
            games_running: false,
            character: None,
            live: None,
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
                        Command::Write(_)
                            | Command::ApplyForge(_)
                            | Command::UndoForge(_)
                            | Command::SetAttackLeechRate(_)
                            | Command::SetHookWanted { .. }
                            | Command::SetHookValues { .. }
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
        if epoch != self.epoch() {
            Err(BackendError::blocked(ErrorKind::Superseded, "operation superseded by queued cleanup"))
        } else {
            Ok(())
        }
    }
    fn publish(&mut self) {
        self.state.profile_lock = self.profile_lock();
        let expected = self.state.expected_observation().ok();
        self.state
            .mappings
            .retain(|mapping| expected.as_ref().is_some_and(|expected| mapping.expected.matches(expected)));
        if self
            .state
            .unique_preview
            .as_ref()
            .is_some_and(|p| expected.as_ref().is_none_or(|current| !p.expected.matches(current)))
        {
            self.state.unique_preview = None;
        }
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
            self.forge_history.clear();
        }
        self.state.undo_forge_depth = if self.state.undo_forge.is_some() { self.forge_history.len() } else { 0 };
        self.state.undo_forge_needs_override = self.state.undo_forge.is_some() && self.forge_history.needs_override();
        self.state.capabilities = Capabilities {
            gameplay_hooks: eg_trainer_wire::hooks::GameplayHook::ALL.map(|hook| {
                self.state.phase == Phase::Ready
                    && self.state.activity.is_none()
                    && self
                        .connection
                        .as_ref()
                        .is_some_and(|connection| connection.supports_hook(HookId::parse(hook.id()).expect("catalog hook")))
            }),
            read_forge: self.state.phase == Phase::Ready,
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
    async fn game_files_for(&mut self, epoch: u64, target: &ProcessIdentity, again: bool) -> Result<Arc<crate::files::FileCatalog>, BackendError> {
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
        let started = eg_core::time::from_filetime(target.created).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
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
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?;
        let (binary, metadata) = connection.build_hashes();
        if !files.matches(&binary, &metadata) {
            return Err(files_changed());
        }
        self.admit(target)?;
        let locale = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
            .read_locale(self.clock.now())
            .await?;
        self.superseded(epoch)?;
        self.admit(target)?;
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
            Command::ReadAttackLeech => self.read_attack_leech().await,
            Command::Write(WritePlan::Forge(plan)) => self.write_forge(*plan).await,
            Command::Write(WritePlan::Field(plan)) => self.set_attack_leech_rate(*plan).await,
            Command::SetAttackLeechRate(plan) => self.set_attack_leech_rate(*plan).await,
            Command::ApplyForge(plan) => self.write_forge(*plan).await,
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
        self.admit_using_policy(target, self.options.policy)
    }
    fn admit_using_policy(&mut self, target: &ProcessIdentity, policy: GatePolicy) -> Result<(), BackendError> {
        let started = eg_core::time::from_filetime(target.created).map_err(|_| BackendError::blocked(ErrorKind::Identity, "invalid process creation time"))?;
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(BackendError::blocked(ErrorKind::Identity, "invalid process identity"));
        }
        let log = self.log.read(started);
        let decision = gate::evaluate(&log, started, self.clock.now(), policy);
        self.state.gate = Some(decision);
        if decision.verdict == GateVerdict::Allow {
            self.character = log.character.filter(|character| !character.online);
        }
        match decision.verdict {
            GateVerdict::Allow => Ok(()),
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
            let error = BackendError::blocked(ErrorKind::RestartRequired, HELPER_RESTART_REQUIRED);
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
            let status = self.connection.as_mut().unwrap().refresh(self.clock.now()).await?;
            self.superseded(epoch)?;
            self.check_status(status)?;
            self.reconcile_hooks();
            let catalog = self.catalog_for(epoch, target, false).await?;
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
            self.event(BackendEvent::Connected {
                generation: self.state.generation,
                target: target.clone(),
            });
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.handle_connection_gate(error);
            if !matches!(
                error.kind,
                ErrorKind::Superseded | ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading
            ) {
                self.fail(error.clone(), Some(target.clone()));
            }
        }
        result
    }
    fn check_status(&mut self, status: RuntimeStatus) -> Result<(), BackendError> {
        if self.state.runtime_status.is_some_and(|previous| previous.manager != status.manager) {
            self.state.mappings.clear();
            self.state.unique_preview = None;
        }
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
            let status = self
                .connection
                .as_mut()
                .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
                .refresh(self.clock.now())
                .await?;
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
            self.admit(&target)?;
            let forge = self.connection.as_mut().unwrap().read_forge(self.clock.now()).await?;
            self.superseded(epoch)?;
            self.admit(&target)?;
            // The game builds an item's implicit text with its tooltip code on its main thread: the monitor keeps the
            // text it has for an item that has not changed, under the same catalog.
            let kept = match (&forge, &self.state.forge) {
                (
                    Some(item),
                    ForgeState::Item {
                        generation, snapshot, implicits, ..
                    },
                ) if monitor && !locale_changed && *generation == self.state.generation && **snapshot == *item => Some(implicits.clone()),
                _ => None,
            };
            let implicits = match (&forge, kept) {
                (_, Some(kept)) => kept,
                (Some(item), None) => self.connection.as_mut().unwrap().describe_implicits(self.clock.now(), item).await?,
                (None, None) => None,
            };
            self.superseded(epoch)?;
            self.admit(&target)?;
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
            self.event(BackendEvent::ReadPublished {
                generation: self.state.generation,
            });
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.handle_connection_gate(error);
            if !matches!(
                error.kind,
                ErrorKind::Superseded | ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading
            ) {
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
                    resolved_name,
                    implicits: None,
                    unique: match &self.state.forge {
                        ForgeState::Item {
                            generation,
                            snapshot: old,
                            unique,
                            ..
                        } if *generation == self.state.generation && **old == snapshot => unique.clone(),
                        _ => None,
                    },
                    snapshot: Arc::new(snapshot),
                }
            }
        }
    }
    fn handle_connection_gate(&mut self, error: &BackendError) {
        if error.kind == ErrorKind::CharacterLoading && self.connection.is_some() {
            self.state.phase = Phase::WaitingForCharacter;
            self.state.runtime_status = None;
            self.state.forge = ForgeState::Unobserved;
            self.state.attack_leech = None;
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
        self.state.attack_leech = None;
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
        self.state.error = Some(error.clone());
        self.event(BackendEvent::Fault(error));
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
            self.state.error = Some(BackendError::blocked(ErrorKind::RestartRequired, HELPER_RESTART_REQUIRED));
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
        self.event(BackendEvent::Released {
            generation: self.state.generation,
            cleanup: self.state.cleanup,
        });
        if stop {
            self.event(BackendEvent::Stopped);
        }
    }
}
