//! What the worker runs on: the game's processes (`ProcessLocator`), the preparation of a connection (`Connector`),
//! the connection itself (`BackendConnection`, which a helper `Session` is) and the clock.
use super::*;
use phrases::phrases;

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

phrases! { pub HelperRestartRequired: "The helper couldn't leave the game. Restart Last Epoch before playing online." }

pub trait BackendConnection: Send + 'static {
    fn target(&self) -> &ProcessIdentity;
    /// Connection health is independent of a particular write's certainty.
    fn is_ready(&self) -> bool {
        true
    }
    /// The reads, edits and hooks this connection serves, whatever the worker is doing; the worker offers them only
    /// while ready and idle.
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    /// Return certainty after entering transport; Err is pre-submission unless delivery explicitly says MayHaveRun.
    /// Confirmed/NotKept require a complete independent read under renewed admission, not only a native ACK. No retry.
    fn write(&mut self, _: &ForgePlan, _: &mut dyn WriteControl) -> impl Future<Output = Result<WriteStatus, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "writes are unavailable")) }
    }
    fn refresh(&mut self) -> impl Future<Output = Result<RuntimeStatus, BackendError>> + Send;
    fn read_locale(&mut self) -> impl Future<Output = Result<crate::definitions::catalog::GameLocale, BackendError>> + Send;
    /// The running build's binary and metadata hashes: the game data read from files with the same hashes describes
    /// this game. Definitions are never captured from the game.
    fn build_hashes(&self) -> (String, String);
    fn read_forge(&mut self) -> impl Future<Output = Result<Option<ForgeSnapshot>, BackendError>> + Send;
    fn read_fields(&mut self, _: eg_trainer_wire::field_edits::FieldSet) -> impl Future<Output = Result<eg_trainer_wire::field_edits::Snapshot, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "field reads are unavailable")) }
    }
    fn apply_fields(
        &mut self,
        _: &eg_trainer_wire::field_edits::Edit,
        _: &mut dyn WriteControl,
    ) -> impl Future<Output = Result<WriteOutcome<eg_trainer_wire::field_edits::Snapshot>, BackendError>> + Send {
        async { Ok(WriteOutcome::refused("field edits are unavailable")) }
    }
    fn describe_implicits(&mut self, _: &ForgeSnapshot) -> impl Future<Output = Result<Option<Vec<ModifierSnapshot>>, BackendError>> + Send {
        async { Ok(None) }
    }
    fn describe_unique(&mut self, _: &ForgeSnapshot) -> impl Future<Output = Result<Option<Vec<ModifierSnapshot>>, BackendError>> + Send {
        async { Ok(None) }
    }
    fn preview_unique(
        &mut self,
        _: &crate::item::edit::ExpectedObservation,
        _: &crate::item::edit::RollEdit,
        _: &crate::item::values::BoundTooltip,
    ) -> impl Future<Output = Result<crate::item::values::BoundPreview, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::MappingUnavailable, "Unique preview is unavailable.")) }
    }
    /// Optional reads may stop between completed exchanges, as `observer` says. Implementations must drain an in-flight
    /// exchange and preserve fatal transport failures. `tooltip` is the game's own description of the item's unique or
    /// implicit lines, which a unique or implicit request is checked against.
    fn resolve_mapping(
        &mut self,
        _: &crate::item::edit::ExpectedObservation,
        _: &MappingRequest,
        _: Option<&crate::item::values::BoundTooltip>,
        _: &mut dyn ReadObserver,
    ) -> impl Future<Output = Result<ValueTable, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::MappingUnavailable, "Actual values are unavailable for this modifier.")) }
    }
    /// Adds these definitions to the game's list; one it already holds stays. A connection without a game has nothing to add to.
    fn register_affixes(&mut self, _: &[eg_trainer_wire::new_affixes::NewAffix]) -> impl Future<Output = Result<(), BackendError>> + Send {
        async { Ok(()) }
    }
    fn observed_hook(&self, _: GameplayHook) -> Option<bool> {
        None
    }
    /// Sets the hook as `request` asks, then answers with the game's own report of it: `Confirmed(on)` when it matches
    /// the request, `NotKept(on)` when it does not.
    fn set_hook(&mut self, _: &HookRequest, _: &mut dyn WriteControl) -> impl Future<Output = Result<WriteOutcome<bool>, BackendError>> + Send {
        async { Ok(WriteOutcome::refused(HOOK_NOT_AVAILABLE)) }
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
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            read_forge: true,
            level_requirement: true,
            status_fields: [true; 7],
            base_change: true,
            identity_change: true,
            affix_rolls: true,
            modifier_rolls: true,
            affix_change: true,
            affix_list: true,
            gameplay_hooks: [true; GameplayHook::COUNT],
            fields: [true; eg_trainer_wire::field_edits::FieldSet::COUNT],
        }
    }
    async fn write(&mut self, plan: &ForgePlan, control: &mut dyn WriteControl) -> Result<WriteStatus, BackendError> {
        Ok(self.edit_forge_controlled(plan, control).await?)
    }
    async fn refresh(&mut self) -> Result<RuntimeStatus, BackendError> {
        Session::refresh(self).await?;
        self.refresh_gameplay_hooks().await?;
        self.runtime_status()
            .copied()
            .ok_or_else(|| BackendError::blocked(ErrorKind::Protocol, "missing runtime status"))
    }
    async fn read_locale(&mut self) -> Result<crate::definitions::catalog::GameLocale, BackendError> {
        Ok(Session::read_locale(self).await?)
    }
    fn build_hashes(&self) -> (String, String) {
        (self.build().binary_hash().to_owned(), self.build().metadata_hash().to_owned())
    }
    async fn describe_implicits(&mut self, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(Session::describe_implicits(self, expected).await?)
    }
    async fn describe_unique(&mut self, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(Session::describe_unique(self, expected).await?)
    }
    async fn preview_unique(
        &mut self,
        expected: &crate::item::edit::ExpectedObservation,
        draft: &crate::item::edit::RollEdit,
        evidence: &crate::item::values::BoundTooltip,
    ) -> Result<crate::item::values::BoundPreview, BackendError> {
        Ok(Session::preview_unique(self, expected, draft, evidence).await?)
    }
    async fn resolve_mapping(
        &mut self,
        expected: &crate::item::edit::ExpectedObservation,
        request: &MappingRequest,
        tooltip: Option<&crate::item::values::BoundTooltip>,
        observer: &mut dyn ReadObserver,
    ) -> Result<ValueTable, BackendError> {
        Ok(Session::resolve_mapping_observed(self, expected, request, tooltip, observer).await?)
    }
    async fn read_forge(&mut self) -> Result<Option<ForgeSnapshot>, BackendError> {
        Ok(Session::read_forge(self).await?)
    }
    async fn read_fields(&mut self, set: eg_trainer_wire::field_edits::FieldSet) -> Result<eg_trainer_wire::field_edits::Snapshot, BackendError> {
        Ok(Session::read_fields(self, set).await?)
    }
    async fn apply_fields(
        &mut self,
        edit: &eg_trainer_wire::field_edits::Edit,
        control: &mut dyn WriteControl,
    ) -> Result<WriteOutcome<eg_trainer_wire::field_edits::Snapshot>, BackendError> {
        Ok(Session::apply_fields(self, edit, control).await?)
    }
    async fn register_affixes(&mut self, affixes: &[eg_trainer_wire::new_affixes::NewAffix]) -> Result<(), BackendError> {
        Ok(Session::register_affixes(self, affixes).await?)
    }
    fn observed_hook(&self, hook: GameplayHook) -> Option<bool> {
        self.hook_observation(hook).map(|actual| actual.enabled)
    }
    async fn set_hook(&mut self, request: &HookRequest, control: &mut dyn WriteControl) -> Result<WriteOutcome<bool>, BackendError> {
        let Some(hook) = request.id.native() else {
            return Ok(WriteOutcome::refused(HOOK_NOT_AVAILABLE));
        };
        let Some(value) = hook.value_kind().encode(&request.values) else {
            return Ok(WriteOutcome::refused("Invalid hook selection"));
        };
        Ok(self.set_gameplay_hook_with_value(hook, request.on, value, control).await?)
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
