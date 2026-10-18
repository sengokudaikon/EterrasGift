//! Serialized control session and owned read-only forge snapshots. No injection or automatic retry.
use crate::{
    gate::{self, GateDecision, GatePolicy, GateVerdict, LogSource},
    profile::BuildProfile,
};
use eg_core::game_log::GameReadiness;
use eg_game_access::{
    definitions::{self, ItemLimits},
    forge::{self, ForgeSnapshot},
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, HookState, ProtocolError, Request, ResponseCode, RuntimeStatus},
    transport::{Delivery, Transport, TransportError, TransportFailure},
};
use jiff::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Disabled,
    Disconnected,
    WaitingForGame,
    WaitingForCharacter,
    Connecting,
    Closing,
    Ready,
    Blocked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleanup {
    NotNeeded,
    ConfirmedOff,
    Unconfirmed,
}
#[derive(Debug)]
pub enum SessionError {
    Disabled,
    Busy,
    NotConnected,
    Gate(GateDecision),
    NotOffline(GameReadiness),
    Protocol(ProtocolError),
    Transport(TransportError),
    Identity(String),
    Helper { code: ResponseCode, message: String, may_have_run: bool },
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "trainer session: {self:?}")
    }
}
impl std::error::Error for SessionError {}

pub struct Session<T, L> {
    target: ProcessIdentity,
    started: Timestamp,
    transport: T,
    log: L,
    profile: BuildProfile,
    policy: GatePolicy,
    enabled: bool,
    state: SessionState,
    discovery: Option<Discovery>,
    status: Option<RuntimeStatus>,
    cleanup: Cleanup,
}
impl<T: Transport, L: LogSource> Session<T, L> {
    pub fn new(target: ProcessIdentity, transport: T, log: L, profile: BuildProfile, policy: GatePolicy) -> Result<Self, SessionError> {
        if transport.target() != &target {
            return Err(SessionError::Identity("transport belongs to a different process identity".into()));
        }
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(SessionError::Identity("invalid process identity".into()));
        }
        profile.validate_target(&target.executable).map_err(|e| SessionError::Identity(e.to_string()))?;
        let started = Timestamp::from_nanosecond((i128::from(target.created) - 116_444_736_000_000_000) * 100).map_err(|e| SessionError::Identity(e.to_string()))?;
        Ok(Self {
            target,
            started,
            transport,
            log,
            profile,
            policy,
            enabled: false,
            state: SessionState::Disabled,
            discovery: None,
            status: None,
            cleanup: Cleanup::NotNeeded,
        })
    }
    pub fn state(&self) -> SessionState {
        self.state
    }
    pub fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    pub fn runtime_status(&self) -> Option<&RuntimeStatus> {
        self.status.as_ref()
    }
    pub fn cleanup(&self) -> Cleanup {
        self.cleanup
    }
    pub fn enable(&mut self) {
        self.enabled = true;
        if self.state == SessionState::Disabled {
            self.state = SessionState::Disconnected;
        }
    }
    fn gate(&mut self, now: Timestamp) -> Result<(), SessionError> {
        if !self.enabled {
            return Err(SessionError::Disabled);
        }
        let decision = gate::evaluate(&self.log.read(self.started), self.started, now, self.policy);
        match decision.verdict {
            GateVerdict::Allow | GateVerdict::Probe => Ok(()),
            GateVerdict::Wait | GateVerdict::Refuse => {
                self.release_without_io();
                self.state = if decision.verdict == GateVerdict::Refuse {
                    SessionState::Blocked
                } else if decision.readiness == GameReadiness::Starting {
                    SessionState::WaitingForGame
                } else {
                    SessionState::WaitingForCharacter
                };
                Err(SessionError::Gate(decision))
            }
        }
    }
    /// Leaves Connecting if this future is dropped: call disconnect before another attachment.
    pub async fn connect(&mut self, now: Timestamp) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        if self.state == SessionState::Ready {
            return self.refresh(now).await;
        }
        self.gate(now)?;
        self.state = SessionState::Connecting;
        self.status = None;
        let elapsed = std::time::Instant::now();
        let result = self.attach(now, elapsed).await;
        if let Err(error) = &result
            && self.state == SessionState::Connecting
        {
            self.state = match error {
                SessionError::Helper { code: ResponseCode::NotReady, .. }
                | SessionError::Transport(TransportError {
                    delivery: Delivery::NotSent,
                    failure: TransportFailure::Busy,
                    ..
                }) => SessionState::WaitingForGame,
                SessionError::Helper {
                    code: ResponseCode::OfflineRequired,
                    ..
                } => SessionState::WaitingForCharacter,
                _ => SessionState::Blocked,
            };
        }
        result
    }
    async fn attach(&mut self, now: Timestamp, elapsed: std::time::Instant) -> Result<(), SessionError> {
        let payload = self.request(&Request::Discover).await?;
        let discovery = Discovery::decode(&payload, self.target.pid).map_err(SessionError::Protocol)?;
        if !discovery.build_id.eq_ignore_ascii_case(self.profile.helper_build_id()) {
            return Err(SessionError::Identity("loaded helper source build ID differs; restart required".into()));
        }
        self.profile.validate(&discovery.dispatcher).map_err(|e| SessionError::Identity(e.to_string()))?;
        self.discovery = Some(discovery);
        // A prior cleanup acknowledgement does not describe a newly attached helper session.
        self.cleanup = Cleanup::Unconfirmed;
        // Discovery may have taken seconds; read Player.log again before Status can install anything.
        let rechecked_at = Timestamp::from_nanosecond(now.as_nanosecond() + elapsed.elapsed().as_nanos() as i128).map_err(|e| SessionError::Identity(e.to_string()))?;
        self.gate(rechecked_at)?;
        self.read_status().await
    }
    pub async fn refresh(&mut self, now: Timestamp) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.gate(now)?;
        if self.discovery.is_none() {
            return Err(SessionError::NotConnected);
        }
        self.status = None;
        self.state = SessionState::Connecting;
        let result = self.read_status().await;
        if let Err(error) = &result
            && self.state == SessionState::Connecting
        {
            self.state = match error {
                SessionError::Helper { code: ResponseCode::NotReady, .. }
                | SessionError::Transport(TransportError {
                    delivery: Delivery::NotSent,
                    failure: TransportFailure::Busy,
                    ..
                }) => SessionState::WaitingForGame,
                SessionError::Helper {
                    code: ResponseCode::OfflineRequired,
                    ..
                } => SessionState::WaitingForCharacter,
                _ => SessionState::Blocked,
            };
        }
        result
    }
    /// Copy the current forge on Unity's main thread. Returned data owns no managed references.
    /// The item address/individual ID are observations scoped to this process identity, never
    /// permission to dereference memory or edit an item. A dropped future leaves Connecting;
    /// explicitly disconnect before another attachment or operation.
    pub async fn read_forge(&mut self, now: Timestamp) -> Result<Option<ForgeSnapshot>, SessionError> {
        self.read_owned(now, |dispatcher| Request::ReadForge { dispatcher }, forge::decode).await
    }
    /// Copy the game's literal item ranges. This carries no locale, rarity names or
    /// claim that the rest of the definitions catalog has been captured. The build
    /// profile is revalidated on every read, and the helper independently gates it.
    pub async fn read_item_limits(&mut self, now: Timestamp) -> Result<ItemLimits, SessionError> {
        self.read_owned(now, |dispatcher| Request::ReadItemLimits { dispatcher }, definitions::decode).await
    }
    /// Read only an already-completed runtime locale selection and the game application version.
    /// This does not start localization or wait for its asynchronous initialization.
    pub async fn read_locale(&mut self, now: Timestamp) -> Result<GameLocale, SessionError> {
        self.read_owned(now, |dispatcher| Request::ReadLocale { dispatcher }, locale::decode).await
    }
    async fn read_owned<R>(&mut self, now: Timestamp, request: fn(DispatcherSpec) -> Request, decode: fn(&[u8]) -> Result<R, ProtocolError>) -> Result<R, SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.gate(now)?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let elapsed = std::time::Instant::now();
        if let Err(error) = self.profile.validate(&dispatcher) {
            self.release_without_io();
            self.state = SessionState::Blocked;
            return Err(SessionError::Identity(error.to_string()));
        }
        // File verification can take time. Admission must still hold immediately before I/O.
        self.gate(advanced(now, elapsed)?)?;
        let status = self.status.take();
        self.state = SessionState::Connecting;
        let result = self.request(&request(dispatcher)).await.and_then(|bytes| decode(&bytes).map_err(SessionError::Protocol));
        match result {
            Ok(snapshot) => {
                // The helper independently checks runtime offline/native-manager life in the
                // same main-thread operation, before reading data. Recheck the log too.
                self.gate(advanced(now, elapsed)?)?;
                self.status = status;
                self.state = SessionState::Ready;
                Ok(snapshot)
            }
            Err(error) => {
                self.release_without_io();
                if self.state == SessionState::Connecting {
                    self.state = SessionState::Blocked;
                }
                Err(error)
            }
        }
    }
    async fn read_status(&mut self) -> Result<(), SessionError> {
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let bytes = self.request(&Request::Status { dispatcher }).await?;
        let status = RuntimeStatus::decode(&bytes).map_err(SessionError::Protocol)?;
        let readiness = if status.manager == 0 {
            GameReadiness::NoCharacter
        } else if status.online {
            GameReadiness::Online
        } else {
            GameReadiness::Offline
        };
        self.status = Some(status);
        if readiness != GameReadiness::Offline {
            self.release_without_io();
            self.state = SessionState::WaitingForCharacter;
            return Err(SessionError::NotOffline(readiness));
        }
        self.state = SessionState::Ready;
        Ok(())
    }
    async fn request(&mut self, request: &Request) -> Result<Vec<u8>, SessionError> {
        let response = self.transport.send(request).await.map_err(SessionError::Transport)?;
        if response.code == ResponseCode::Done {
            return Ok(response.payload);
        }
        if response.code == ResponseCode::OfflineRequired {
            self.release_without_io();
            self.state = SessionState::WaitingForCharacter;
        }
        Err(SessionError::Helper {
            code: response.code,
            message: String::from_utf8_lossy(&response.payload).into_owned(),
            may_have_run: response.code == ResponseCode::Failed,
        })
    }
    fn release_without_io(&mut self) {
        if self.discovery.take().is_some() {
            self.cleanup = Cleanup::Unconfirmed;
        }
        self.status = None;
        // Once offline admission is lost, there is no more client process/pipe access. The helper
        // must independently remove gameplay features on its main-thread callback.
    }
    async fn clear_hooks(&mut self) {
        let Some(discovery) = &self.discovery else {
            return;
        };
        // Empty wanted set is permitted online too. It never removes the scheduler/helper itself.
        self.cleanup = Cleanup::Unconfirmed;
        if let Ok(bytes) = self.request(&Request::ClearHooks { dispatcher: discovery.dispatcher }).await
            && let Ok(states) = HookState::decode_all(&bytes)
            && states.iter().all(|state| !state.on)
        {
            self.cleanup = Cleanup::ConfirmedOff;
        }
    }
    /// Explicit asynchronous cleanup; dropping Session never claims to turn effects off or unload code.
    pub async fn disconnect(&mut self) -> Cleanup {
        self.status = None;
        self.state = SessionState::Closing;
        // Best effort, as in HookSync.TurnOffAsync. Cancellation never proves a submitted clear ran.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), self.clear_hooks()).await;
        self.discovery = None;
        self.state = if self.enabled { SessionState::Disconnected } else { SessionState::Disabled };
        self.cleanup
    }
    pub async fn disable(&mut self) -> Cleanup {
        self.enabled = false;
        self.disconnect().await
    }
}

fn advanced(now: Timestamp, elapsed: std::time::Instant) -> Result<Timestamp, SessionError> {
    Timestamp::from_nanosecond(now.as_nanosecond() + elapsed.elapsed().as_nanos() as i128).map_err(|error| SessionError::Identity(error.to_string()))
}
