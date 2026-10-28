//! What the worker publishes: its snapshot, phase, activity and capabilities, the game data and item it holds, and
//! the errors it reports.
use super::*;

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
    pub(super) fn retryable(&self) -> bool {
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
impl Capabilities {
    /// Whether these capabilities include `edit`.
    pub fn allows(&self, edit: &crate::item::edit::ItemEdit) -> bool {
        use crate::item::edit::{ItemEdit, RollTarget};
        match edit {
            ItemEdit::AddAffix(_) | ItemEdit::RemoveAffix(_) => self.affix_list,
            ItemEdit::Affix(_) => self.affix_change,
            ItemEdit::Roll(edit) if edit.target == RollTarget::Affix => self.affix_rolls,
            ItemEdit::Status(edit) => self.status_fields[edit.field as usize],
            ItemEdit::BaseType(_) => self.base_change,
            ItemEdit::UniqueIdentity(_) => self.identity_change,
            _ => false,
        }
    }
}
#[derive(Clone, Debug)]
pub enum CatalogState {
    Missing,
    Current { generation: u64, data: Arc<GameCatalog> },
    Stale(Arc<GameCatalog>),
}
/// Every publish compares the state with the last one, several times a second, and a catalog is large: the same
/// allocation is equal without reading it. Distinct allocations still compare by content.
impl PartialEq for CatalogState {
    fn eq(&self, other: &Self) -> bool {
        let same = |a: &Arc<GameCatalog>, b: &Arc<GameCatalog>| Arc::ptr_eq(a, b) || **a == **b;
        match (self, other) {
            (Self::Missing, Self::Missing) => true,
            (Self::Current { generation: a, data: x }, Self::Current { generation: b, data: y }) => a == b && same(x, y),
            (Self::Stale(x), Self::Stale(y)) => same(x, y),
            _ => false,
        }
    }
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
/// The game data service's state (`crate::definitions::service`), for the views. A session uses its catalog when it runs the build
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
    pub undo_forge: Option<crate::item::undo::ForgeUndo>,
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
    pub expected: crate::item::edit::ExpectedObservation,
    pub draft: crate::item::edit::RollEdit,
    pub result: Result<crate::item::values::BoundPreview, String>,
}
/// Optional display data for one complete observed item and exact draft query.
/// Kept in memory only; a changed item, catalog or connection discards it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMapping {
    pub expected: crate::item::edit::ExpectedObservation,
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
    pub fn mapping(&self, expected: &crate::item::edit::ExpectedObservation, request: &MappingRequest) -> Option<&Result<Arc<ValueTable>, String>> {
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
    pub fn expected_observation(&self) -> Result<crate::item::edit::ExpectedObservation, crate::item::edit::EditError> {
        use crate::item::edit::{EditError, ExpectedObservation};
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
    pub fn level_requirement_plan(&self, value: i32) -> Result<ForgePlan, crate::item::edit::EditError> {
        self.status_plan(crate::item::edit::StatusField::LevelRequirement, value)
    }
    pub fn status_plan(&self, field: crate::item::edit::StatusField, value: i32) -> Result<ForgePlan, crate::item::edit::EditError> {
        self.status_edit_plan(crate::item::edit::StatusEdit {
            field,
            value,
            unrestricted: false,
        })
    }
    pub fn status_edit_plan(&self, edit: crate::item::edit::StatusEdit) -> Result<ForgePlan, crate::item::edit::EditError> {
        self.forge_plan(crate::item::edit::ItemEdit::Status(edit))
    }
    pub fn forge_plan(&self, edit: crate::item::edit::ItemEdit) -> Result<ForgePlan, crate::item::edit::EditError> {
        use crate::item::edit::EditError;
        let expected = self.expected_observation()?;
        let status = self
            .runtime_status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(EditError::OfflineRequired)?;
        let catalog = self.catalog.data().ok_or(EditError::CatalogNotVerified)?;
        ForgePlan::edit(&expected, status.manager, catalog, edit, self.override_enabled)
    }
    pub fn forge_undo_plan(&self, undo: &crate::item::undo::ForgeUndo) -> Result<ForgePlan, crate::item::edit::EditError> {
        use crate::item::edit::EditError;
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
