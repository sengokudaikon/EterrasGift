//! Serialized control session, owned observations and explicit guarded writes. No automatic retry.
mod hooks;
mod mapping;
mod rate_write;
mod unique;
mod write;
use crate::profile::{BuildProfile, ProfileLease};
use eg_core::game_log::GameReadiness;
use eg_trainer_wire::{
    fields::{self, AttackLeechRead, AttackLeechUnavailable},
    forge::{self, ForgeSnapshot},
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, ProtocolError, Request, ResponseCode, RuntimeStatus},
    transport::{Transport, TransportError},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Disabled,
    Disconnected,
    Connecting,
    Closing,
    Ready,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleanup {
    NotNeeded,
    /// The helper module is absent and its dispatcher entry is pristine.
    ConfirmedOff,
    Unconfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadControl {
    Continue,
    Stop,
}
/// Called only between completed synchronous work/RPCs, never to abort an
/// exchange. Returning Stop leaves Ready and discovery available for cleanup.
pub trait ReadObserver: Send {
    fn control(&mut self) -> ReadControl {
        ReadControl::Continue
    }
}
/// An observer that never stops a read.
struct ContinueRead;
impl ReadObserver for ContinueRead {}
enum OwnedRead<R> {
    Complete(R),
    Stopped,
}
#[derive(Debug)]
pub enum SessionError {
    Disabled,
    Busy,
    NotConnected,
    CharacterLoading,
    NotOffline(GameReadiness),
    Protocol(ProtocolError),
    Transport(TransportError),
    Identity(String),
    Mapping(String),
    MappingRetryable(String),
    Helper { code: ResponseCode, message: String, may_have_run: bool },
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("The trainer is disabled."),
            Self::Busy => f.write_str("Wait for the current trainer operation to finish."),
            Self::CharacterLoading => f.write_str("The offline character is changing area."),
            Self::NotConnected => f.write_str("Connect before reading or editing an item."),
            Self::NotOffline(readiness) => f.write_str(readiness_message(*readiness)),
            Self::Protocol(error) => write!(f, "The helper returned an invalid response: {error}"),
            Self::Transport(error) => error.fmt(f),
            Self::Identity(message) | Self::Mapping(message) | Self::MappingRetryable(message) => f.write_str(message),
            Self::Helper { message, may_have_run, .. } => {
                f.write_str(message)?;
                if *may_have_run {
                    f.write_str(" The edit may have reached the game; do not automatically retry.")?;
                }
                Ok(())
            }
        }
    }
}
impl std::error::Error for SessionError {}

fn readiness_message(readiness: GameReadiness) -> &'static str {
    match readiness {
        GameReadiness::Starting => "Waiting for the game to finish loading.",
        GameReadiness::NoCharacter => "Load an offline character to continue.",
        GameReadiness::Online => "The trainer requires an offline character. Online play is not supported.",
        GameReadiness::Offline => "The offline session could not be verified. Reconnect before continuing.",
    }
}

pub struct Session<T> {
    target: ProcessIdentity,
    transport: T,
    profile: BuildProfile,
    lease: Option<ProfileLease>,
    state: SessionState,
    discovery: Option<Discovery>,
    status: Option<RuntimeStatus>,
    cleanup: Cleanup,
    unload_needed: bool,
    last_write: Option<crate::writes::WriteRecord>,
    last_rate_write: Option<crate::field_writes::RateWriteRecord>,
    hooks: [Option<eg_trainer_wire::hooks::HookObservation>; 4],
}
impl<T: Transport> Session<T> {
    fn observation_matches(&self, expected: &crate::edits::ExpectedObservation) -> bool {
        let identity = expected.catalog_identity();
        expected.process() == &self.target
            && identity.binary_hash.eq_ignore_ascii_case(self.profile.binary_hash())
            && identity.metadata_hash.eq_ignore_ascii_case(self.profile.metadata_hash())
            && identity.extraction_version == crate::catalog::EXTRACTION_VERSION
    }

    /// The build this session runs: the hashes its catalog must carry.
    pub fn profile(&self) -> &BuildProfile {
        &self.profile
    }

    pub fn new(target: ProcessIdentity, transport: T, profile: BuildProfile) -> Result<Self, SessionError> {
        if transport.target() != &target {
            return Err(SessionError::Identity("transport belongs to a different process identity".into()));
        }
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(SessionError::Identity("invalid process identity".into()));
        }
        profile.validate_target(&target.executable).map_err(|e| SessionError::Identity(e.to_string()))?;
        Ok(Self {
            target,
            transport,
            profile,
            lease: None,
            hooks: [None; 4],
            state: SessionState::Disabled,
            discovery: None,
            status: None,
            cleanup: Cleanup::NotNeeded,
            unload_needed: false,
            last_write: None,
            last_rate_write: None,
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
    /// Content bytes read by the retained artifact lease, for I/O diagnostics.
    /// None when disconnected or when the platform requires full revalidation.
    pub fn artifact_bytes_read(&self) -> Option<u64> {
        self.lease.as_ref().and_then(ProfileLease::bytes_read)
    }
    /// With the hashing fallback, how often the artifacts were read whole.
    pub fn artifact_full_validations(&self) -> Option<u64> {
        self.lease.as_ref().and_then(ProfileLease::full_validations)
    }
    pub fn cleanup(&self) -> Cleanup {
        self.cleanup
    }
    pub fn enable(&mut self) {
        if self.state == SessionState::Disabled {
            self.state = SessionState::Disconnected;
        }
    }
    // Player.log admission belongs to the serialized backend. The session owns only
    // transport lifecycle; the helper checks the current runtime graph on its main thread.
    fn require_enabled(&self) -> Result<(), SessionError> {
        if self.state == SessionState::Disabled { Err(SessionError::Disabled) } else { Ok(()) }
    }
    /// A request may start: nothing else is in flight, and the session is enabled and ready.
    fn admit(&self) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.require_enabled()?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        Ok(())
    }
    /// The dispatcher the attached helper reported.
    fn dispatcher(&self) -> Result<DispatcherSpec, SessionError> {
        Ok(self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher)
    }
    /// The manager of the offline character the last status admitted.
    fn offline_manager(&self) -> Result<u64, SessionError> {
        Ok(self
            .status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(SessionError::NotConnected)?
            .manager)
    }
    /// Leaves Connecting if this future is dropped: call disconnect before another attachment.
    pub async fn connect(&mut self) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        if self.state == SessionState::Ready {
            return self.refresh().await;
        }
        self.require_enabled()?;
        self.state = SessionState::Connecting;
        self.status = None;
        let result = self.attach().await;
        if result.is_err() && self.state == SessionState::Connecting {
            self.state = SessionState::Disconnected;
        }
        result
    }
    async fn attach(&mut self) -> Result<(), SessionError> {
        self.unload_needed = true;
        self.cleanup = Cleanup::Unconfirmed;
        let payload = self.request(&Request::Discover).await?;
        let discovery = Discovery::decode(&payload, self.target.pid).map_err(SessionError::Protocol)?;
        if !discovery.build_id.eq_ignore_ascii_case(self.profile.helper_build_id()) {
            return Err(SessionError::Identity("loaded helper source build ID differs; restart required".into()));
        }
        self.lease = Some(self.profile.capture_lease(&discovery.dispatcher).map_err(|e| SessionError::Identity(e.to_string()))?);
        self.discovery = Some(discovery);
        // A prior cleanup acknowledgement does not describe a newly attached helper session.
        self.cleanup = Cleanup::Unconfirmed;
        self.read_status().await
    }
    pub async fn refresh(&mut self) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.require_enabled()?;
        if self.discovery.is_none() {
            return Err(SessionError::NotConnected);
        }
        self.status = None;
        self.state = SessionState::Connecting;
        let result = self.read_status().await;
        if result.is_err() && self.state == SessionState::Connecting {
            self.state = SessionState::Disconnected;
        }
        result
    }
    /// Copy the current forge on Unity's main thread. Returned data owns no managed references.
    /// The item address/individual ID are observations scoped to this process identity, never
    /// permission to dereference memory or edit an item. A dropped future leaves Connecting;
    /// explicitly disconnect before another attachment or operation.
    pub async fn read_forge(&mut self) -> Result<Option<ForgeSnapshot>, SessionError> {
        self.read_owned(|dispatcher| Request::ReadForge { dispatcher }, forge::decode).await
    }
    /// Optional presentation. A game formatting refusal leaves the session ready;
    /// transport, protocol, process and offline failures still revoke admission.
    pub async fn describe_implicits(&mut self, expected: &ForgeSnapshot) -> Result<Option<Vec<eg_trainer_wire::mapping::ModifierSnapshot>>, SessionError> {
        let manager = self.offline_manager()?;
        let dispatcher = self.dispatcher()?;
        let query = eg_trainer_wire::mapping::ForgeDescriptionRequest::new(dispatcher, manager, expected.clone()).map_err(SessionError::Protocol)?;
        let result = self.read_owned(|_| Request::DescribeImplicits(query), eg_trainer_wire::mapping::decode_modifiers).await;
        match result {
            Ok(modifiers) => {
                if modifiers.iter().any(|m| {
                    m.index as usize >= eg_trainer_wire::catalog::MAX_IMPLICITS
                        || m.roll_ids.len() > 1
                        || m.roll_ids.iter().any(|id| usize::from(*id) >= expected.implicit_rolls.len() || u32::from(*id) != m.index)
                }) {
                    self.release_without_io();
                    self.state = SessionState::Disconnected;
                    return Err(SessionError::Protocol(ProtocolError::InvalidData));
                }
                Ok(Some(modifiers))
            }
            Err(SessionError::Helper { code, .. }) if code.spares_presentation_session() && self.state == SessionState::Ready => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Observe the two actor-stat fields. Identity tokens and raw float bits convey no
    /// write authority. Feature-local graph unavailability leaves the admitted session ready;
    /// runtime offline/manager refusal revokes admission even though its typed reason is returned.
    pub async fn read_attack_leech(&mut self) -> Result<AttackLeechRead, SessionError> {
        self.admit()?;
        let expected_manager = self.offline_manager()?;
        let dispatcher = self.dispatcher()?;
        let read = match self
            .read_owned_control(
                |dispatcher| Request::ReadAttackLeech { dispatcher, expected_manager },
                |bytes| {
                    let read = fields::decode(bytes)?;
                    if matches!(read, AttackLeechRead::Available(observation) if observation.manager.wrapper_token() != expected_manager) {
                        return Err(ProtocolError::InvalidData);
                    }
                    Ok(read)
                },
                None,
            )
            .await?
        {
            OwnedRead::Complete(read) => read,
            OwnedRead::Stopped => return Err(SessionError::Protocol(ProtocolError::InvalidData)),
        };
        if let Err(error) = self.validate_profile(&dispatcher) {
            self.release_without_io();
            self.state = SessionState::Disconnected;
            return Err(SessionError::Identity(error.to_string()));
        }
        self.require_enabled()?;
        if matches!(
            read,
            AttackLeechRead::Unavailable(AttackLeechUnavailable::OfflineRequired | AttackLeechUnavailable::ManagerAbsent | AttackLeechUnavailable::ManagerChanged)
        ) {
            self.release_without_io();
            self.state = SessionState::Disconnected;
        }
        Ok(read)
    }
    /// Read only an already-completed runtime locale selection and the game application version.
    /// This does not start localization or wait for its asynchronous initialization.
    pub async fn read_locale(&mut self) -> Result<GameLocale, SessionError> {
        self.read_owned(|dispatcher| Request::ReadLocale { dispatcher }, locale::decode).await
    }
    async fn read_owned<R>(&mut self, request: impl FnOnce(DispatcherSpec) -> Request, decode: fn(&[u8]) -> Result<R, ProtocolError>) -> Result<R, SessionError> {
        match self.read_owned_control(request, decode, None).await? {
            OwnedRead::Complete(value) => Ok(value),
            OwnedRead::Stopped => Err(SessionError::Protocol(ProtocolError::InvalidData)),
        }
    }
    async fn read_owned_control<R>(
        &mut self,
        request: impl FnOnce(DispatcherSpec) -> Request,
        decode: impl FnOnce(&[u8]) -> Result<R, ProtocolError>,
        mut observer: Option<&mut dyn ReadObserver>,
    ) -> Result<OwnedRead<R>, SessionError> {
        self.admit()?;
        if observer.as_mut().is_some_and(|observer| observer.control() == ReadControl::Stop) {
            return Ok(OwnedRead::Stopped);
        }
        let dispatcher = self.dispatcher()?;
        if let Err(error) = self.validate_profile(&dispatcher) {
            self.release_without_io();
            self.state = SessionState::Disconnected;
            return Err(SessionError::Identity(error.to_string()));
        }
        if observer.as_mut().is_some_and(|observer| observer.control() == ReadControl::Stop) {
            return Ok(OwnedRead::Stopped);
        }
        let status = self.status.take();
        self.state = SessionState::Connecting;
        let request = request(dispatcher);
        let optional_description = matches!(
            request,
            Request::DescribeImplicits(_) | Request::DescribeUnique(_) | Request::PreviewUnique(_) | Request::MapRoll(_)
        );
        let result = self.request(&request).await.and_then(|bytes| decode(&bytes).map_err(SessionError::Protocol));
        match result {
            Ok(snapshot) => {
                if let Err(error) = self.validate_profile(&dispatcher) {
                    self.release_without_io();
                    self.state = SessionState::Disconnected;
                    return Err(SessionError::Identity(error.to_string()));
                }
                self.status = status;
                self.state = SessionState::Ready;
                if observer.as_mut().is_some_and(|observer| observer.control() == ReadControl::Stop) {
                    return Ok(OwnedRead::Stopped);
                }
                Ok(OwnedRead::Complete(snapshot))
            }
            Err(error) => {
                if matches!(error, SessionError::CharacterLoading) {
                    self.status = None;
                    self.state = SessionState::Ready;
                    return Err(error);
                }
                if optional_description && matches!(&error, SessionError::Helper { code, .. } if code.spares_presentation_session()) {
                    self.status = status;
                    self.state = SessionState::Ready;
                    return Err(error);
                }
                self.release_without_io();
                if self.state == SessionState::Connecting {
                    self.state = SessionState::Disconnected;
                }
                Err(error)
            }
        }
    }
    fn validate_profile(&self, dispatcher: &DispatcherSpec) -> std::io::Result<()> {
        let lease = self.lease.as_ref().ok_or_else(|| std::io::Error::other("session artifact lease missing"))?;
        self.profile.validate_lease(dispatcher, lease)
    }
    async fn read_status(&mut self) -> Result<(), SessionError> {
        let dispatcher = self.dispatcher()?;
        if let Err(error) = self.validate_profile(&dispatcher) {
            self.release_without_io();
            self.state = SessionState::Disconnected;
            return Err(SessionError::Identity(error.to_string()));
        }
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
            self.state = SessionState::Disconnected;
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
        if response.code == ResponseCode::CharacterLoading {
            self.status = None;
            self.state = SessionState::Ready;
            return Err(SessionError::CharacterLoading);
        }
        if response.code == ResponseCode::OfflineRequired {
            self.release_without_io();
            self.state = SessionState::Disconnected;
        }
        Err(SessionError::Helper {
            code: response.code,
            message: String::from_utf8_lossy(&response.payload).into_owned(),
            may_have_run: response.code == ResponseCode::Failed,
        })
    }
    fn release_without_io(&mut self) {
        self.hooks = [None; 4];
        if self.discovery.take().is_some() {
            self.cleanup = Cleanup::Unconfirmed;
        }
        self.status = None;
        self.lease = None;
        // Admission is revoked synchronously. The worker subsequently sends only
        // the owner-only Unload control request and reads departure evidence.
    }
    /// Stop this helper load and wait for native departure proof. This control
    /// request is allowed after loss of offline admission; it cannot install a
    /// hook or perform gameplay work. Dropping Session still performs no I/O.
    pub async fn disconnect(&mut self) -> Cleanup {
        let was_disabled = self.state == SessionState::Disabled;
        self.abandon_pending_write();
        self.status = None;
        self.state = SessionState::Closing;
        // Consume before awaiting: cancellation never causes another Unload send.
        if std::mem::take(&mut self.unload_needed) {
            self.cleanup = Cleanup::Unconfirmed;
            if matches!(tokio::time::timeout(std::time::Duration::from_secs(11), self.transport.unload()).await, Ok(Ok(()))) {
                self.cleanup = Cleanup::ConfirmedOff;
            }
        }
        self.discovery = None;
        self.lease = None;
        self.state = if was_disabled { SessionState::Disabled } else { SessionState::Disconnected };
        self.cleanup
    }
    pub async fn disable(&mut self) -> Cleanup {
        let cleanup = self.disconnect().await;
        self.state = SessionState::Disabled;
        cleanup
    }
}
