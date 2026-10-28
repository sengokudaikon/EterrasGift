//! What the worker runs on: the game's processes (`ProcessLocator`), the preparation of a connection (`Connector`),
//! the connection itself (`BackendConnection`, which a helper `Session` is) and the clock.
use super::*;

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
    /// The reads and edits this connection serves, whatever the worker is doing; the worker offers them only while
    /// ready and idle. Hooks are asked one by one (`supports_hook`); `gameplay_hooks` is the worker's to fill.
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn supports_hook(&self, _: GameplayHook) -> bool {
        false
    }
    /// Return certainty after entering transport; Err is pre-submission unless delivery explicitly says MayHaveRun.
    /// Confirmed/NotKept require a complete independent read under renewed admission, not only a native ACK. No retry.
    fn write(&mut self, _: &WritePlan, _: &mut dyn WriteControl) -> impl Future<Output = Result<WriteOutcome<WriteObservation>, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "writes are unavailable")) }
    }
    fn refresh(&mut self) -> impl Future<Output = Result<RuntimeStatus, BackendError>> + Send;
    fn read_locale(&mut self) -> impl Future<Output = Result<crate::definitions::catalog::GameLocale, BackendError>> + Send;
    /// The running build's binary and metadata hashes: the game data read from files with the same hashes describes
    /// this game. Definitions are never captured from the game.
    fn build_hashes(&self) -> (String, String);
    fn read_forge(&mut self) -> impl Future<Output = Result<Option<ForgeSnapshot>, BackendError>> + Send;
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
    /// exchange and preserve fatal transport failures.
    fn resolve_mapping(
        &mut self,
        _: &crate::item::edit::ExpectedObservation,
        _: &MappingRequest,
        _: &mut dyn ReadObserver,
    ) -> impl Future<Output = Result<ValueTable, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::MappingUnavailable, "Actual values are unavailable for this modifier.")) }
    }
    fn read_attack_leech(&mut self) -> impl Future<Output = Result<AttackLeechRead, BackendError>> + Send {
        async { Err(BackendError::blocked(ErrorKind::NotReady, "attack/leech reads are unavailable")) }
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
            read_attack_leech: true,
            attack_leech_rate: true,
            level_requirement: true,
            status_fields: [true; 7],
            base_change: true,
            identity_change: true,
            affix_rolls: true,
            affix_change: true,
            affix_list: true,
            ..Capabilities::default()
        }
    }
    fn supports_hook(&self, _: GameplayHook) -> bool {
        true
    }
    async fn write(&mut self, plan: &WritePlan, control: &mut dyn WriteControl) -> Result<WriteOutcome<WriteObservation>, BackendError> {
        Ok(self.write_controlled(plan, control).await?)
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
        (self.profile().binary_hash().to_owned(), self.profile().metadata_hash().to_owned())
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
        observer: &mut dyn ReadObserver,
    ) -> Result<ValueTable, BackendError> {
        Ok(Session::resolve_mapping_observed(self, expected, request, observer).await?)
    }
    async fn read_forge(&mut self) -> Result<Option<ForgeSnapshot>, BackendError> {
        Ok(Session::read_forge(self).await?)
    }
    async fn read_attack_leech(&mut self) -> Result<AttackLeechRead, BackendError> {
        Ok(Session::read_attack_leech(self).await?)
    }
    fn observed_hook(&self, hook: GameplayHook) -> Option<bool> {
        self.hook_observation(hook).map(|actual| actual.enabled)
    }
    async fn set_hook(&mut self, request: &HookRequest, control: &mut dyn WriteControl) -> Result<WriteOutcome<bool>, BackendError> {
        let Some(hook) = request.id.native() else {
            return Ok(WriteOutcome::refused(HOOK_NOT_AVAILABLE));
        };
        let value = match (hook, request.values.as_slice()) {
            (GameplayHook::MysteriousRift, [mode]) if *mode == 1.0 => 1,
            (GameplayHook::MysteriousRift, [mode]) if *mode == 2.0 => 0,
            (GameplayHook::MysteriousRift, [mode]) if *mode == 0.0 && !request.on => 1,
            (GameplayHook::Offhand, []) => 1,
            (GameplayHook::IncomingDamage, [percent]) if percent.is_finite() && (0.0..=100.0).contains(percent) && percent.fract() == 0.0 => *percent as u64,
            (GameplayHook::DamageDealt, [multiplier]) if multiplier.is_finite() && ((0.0..=100.0).contains(multiplier) || *multiplier == 101.0) => multiplier.to_bits(),
            _ => return Ok(WriteOutcome::refused("Invalid hook selection")),
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
