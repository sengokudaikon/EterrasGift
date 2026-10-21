//! Serialized control session, owned observations and explicit guarded writes. No automatic retry.
mod mapping;
mod rate_write;
mod unique;
mod write;
use crate::profile::{BuildProfile, ProfileLease};
use eg_core::game_log::GameReadiness;
use eg_trainer_wire::{
    catalog::{self, CatalogKind, CatalogPage},
    definitions::{self, ItemLimits},
    fields::{self, AttackLeechRead, AttackLeechUnavailable},
    forge::{self, ForgeSnapshot},
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, HookState, ProtocolError, Request, ResponseCode, RuntimeStatus},
    transport::{Transport, TransportError},
};
use jiff::Timestamp;

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
    ConfirmedOff,
    Unconfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureControl {
    Continue,
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureProgress {
    Starting,
    LeaseVerified {
        bytes_read: Option<u64>,
    },
    LocaleRead,
    LimitsRead,
    Page {
        kind: CatalogKind,
        cursor: u32,
        total: u32,
        count: u32,
        pages: u32,
        bytes: u64,
    },
    VerifyingLocale {
        pages: u32,
        bytes: u64,
    },
    Complete {
        pages: u32,
        bytes: u64,
    },
}
/// Called only between completed synchronous work/RPCs, never to abort an
/// exchange. Returning Stop leaves Ready and discovery available for cleanup.
pub trait CaptureObserver: Send {
    fn control(&mut self) -> CaptureControl {
        CaptureControl::Continue
    }
    fn progress(&mut self, _: CaptureProgress) {}
}
struct ContinueCapture;
impl CaptureObserver for ContinueCapture {}
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureOutcome {
    Complete(Box<crate::catalog::GameCatalog>),
    Stopped,
}
enum OwnedRead<R> {
    Complete(R),
    Stopped,
}
#[derive(Debug)]
pub enum SessionError {
    Disabled,
    Busy,
    NotConnected,
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
    last_write: Option<crate::writes::WriteRecord>,
    last_rate_write: Option<crate::field_writes::RateWriteRecord>,
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
            state: SessionState::Disabled,
            discovery: None,
            status: None,
            cleanup: Cleanup::NotNeeded,
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
    /// Leaves Connecting if this future is dropped: call disconnect before another attachment.
    pub async fn connect(&mut self, now: Timestamp) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        if self.state == SessionState::Ready {
            return self.refresh(now).await;
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
    pub async fn refresh(&mut self, _now: Timestamp) -> Result<(), SessionError> {
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
    pub async fn read_forge(&mut self, now: Timestamp) -> Result<Option<ForgeSnapshot>, SessionError> {
        self.read_owned(now, |dispatcher| Request::ReadForge { dispatcher }, forge::decode).await
    }
    /// Optional presentation. A game formatting refusal leaves the session ready;
    /// transport, protocol, process and offline failures still revoke admission.
    pub async fn describe_implicits(&mut self, now: Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<eg_trainer_wire::mapping::ModifierSnapshot>>, SessionError> {
        let manager = self.status.filter(|s| !s.online && s.manager != 0).ok_or(SessionError::NotConnected)?.manager;
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let query = eg_trainer_wire::mapping::ForgeDescriptionRequest::new(dispatcher, manager, expected.clone()).map_err(SessionError::Protocol)?;
        let result = self
            .read_owned(now, |_| Request::DescribeImplicits(query), eg_trainer_wire::mapping::decode_modifiers)
            .await;
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
            Err(SessionError::Helper {
                code: ResponseCode::Refused | ResponseCode::Failed | ResponseCode::NotReady | ResponseCode::StaleItem,
                ..
            }) if self.state == SessionState::Ready => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Observe the two actor-stat fields. Identity tokens and raw float bits convey no
    /// write authority. Feature-local graph unavailability leaves the admitted session ready;
    /// runtime offline/manager refusal revokes admission even though its typed reason is returned.
    pub async fn read_attack_leech(&mut self, now: Timestamp) -> Result<AttackLeechRead, SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.require_enabled()?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        let expected_manager = self
            .status
            .filter(|status| !status.online && status.manager != 0)
            .ok_or(SessionError::NotConnected)?
            .manager;
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let read = match self
            .read_owned_control(
                now,
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
    /// Copy the game's literal item ranges. This carries no locale, rarity names or
    /// claim that the rest of the definitions catalog has been captured. The build
    /// profile lease is checked on every read, and the helper independently gates it.
    pub async fn read_item_limits(&mut self, now: Timestamp) -> Result<ItemLimits, SessionError> {
        self.read_owned(now, |dispatcher| Request::ReadItemLimits { dispatcher }, definitions::decode).await
    }
    /// Read only an already-completed runtime locale selection and the game application version.
    /// This does not start localization or wait for its asynchronous initialization.
    pub async fn read_locale(&mut self, now: Timestamp) -> Result<GameLocale, SessionError> {
        self.read_owned(now, |dispatcher| Request::ReadLocale { dispatcher }, locale::decode).await
    }
    pub async fn read_catalog_page(&mut self, now: Timestamp, kind: CatalogKind, cursor: u32) -> Result<CatalogPage, SessionError> {
        let page = self
            .read_owned(now, |dispatcher| Request::ReadCatalogPage { dispatcher, kind, cursor }, catalog::decode)
            .await?;
        if page.kind != kind || page.cursor != cursor {
            self.release_without_io();
            self.state = SessionState::Disconnected;
            return Err(SessionError::Protocol(ProtocolError::InvalidData));
        }
        Ok(page)
    }
    /// Capture all definition kinds without publishing partial results. Locale/version and each
    /// population are pinned. This relies on immutable game definition assets, not a snapshot lock.
    pub async fn capture_catalog(&mut self, now: Timestamp) -> Result<crate::catalog::GameCatalog, SessionError> {
        match self.capture_catalog_observed(now, &mut ContinueCapture).await? {
            CaptureOutcome::Complete(catalog) => Ok(*catalog),
            CaptureOutcome::Stopped => Err(SessionError::Protocol(ProtocolError::InvalidData)),
        }
    }
    pub async fn capture_catalog_observed(&mut self, now: Timestamp, observer: &mut dyn CaptureObserver) -> Result<CaptureOutcome, SessionError> {
        let result = self.capture_catalog_inner(now, observer).await;
        if result.is_err() && self.state == SessionState::Ready {
            self.release_without_io();
            self.state = SessionState::Disconnected;
        }
        result
    }
    async fn capture_catalog_inner(&mut self, now: Timestamp, observer: &mut dyn CaptureObserver) -> Result<CaptureOutcome, SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.require_enabled()?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        if observer.control() == CaptureControl::Stop {
            return Ok(CaptureOutcome::Stopped);
        }
        observer.progress(CaptureProgress::Starting);
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        self.validate_profile(&dispatcher).map_err(|error| SessionError::Identity(error.to_string()))?;
        observer.progress(CaptureProgress::LeaseVerified {
            bytes_read: self.artifact_bytes_read(),
        });
        macro_rules! copied {
            ($future:expr) => {
                match $future.await? {
                    OwnedRead::Complete(value) => value,
                    OwnedRead::Stopped => return Ok(CaptureOutcome::Stopped),
                }
            };
        }
        let locale = copied!(self.read_owned_control(now, |dispatcher| Request::ReadLocale { dispatcher }, locale::decode, Some(observer)));
        observer.progress(CaptureProgress::LocaleRead);
        let limits = copied!(self.read_owned_control(now, |dispatcher| Request::ReadItemLimits { dispatcher }, definitions::decode, Some(observer),));
        observer.progress(CaptureProgress::LimitsRead);
        let identity = crate::catalog::CatalogIdentity {
            binary_hash: self.profile.binary_hash().into(),
            metadata_hash: self.profile.metadata_hash().into(),
            locale: locale.clone(),
            extraction_version: crate::catalog::EXTRACTION_VERSION,
        };
        let mut builder = crate::catalog::CatalogBuilder::new(identity, limits).map_err(SessionError::Protocol)?;
        let mut pages = 0;
        let mut bytes = 0u64;
        for kind in [CatalogKind::Bases, CatalogKind::Affixes, CatalogKind::Uniques, CatalogKind::Rarities] {
            while let Some(cursor) = builder.next_cursor(kind) {
                let (page, length) = copied!(self.read_owned_control(
                    now,
                    |dispatcher| Request::ReadCatalogPage { dispatcher, kind, cursor },
                    |payload| Ok((catalog::decode(payload)?, payload.len())),
                    Some(observer)
                ));
                if page.kind != kind || page.cursor != cursor {
                    return Err(SessionError::Protocol(ProtocolError::InvalidData));
                }
                let count = page.entries.len() as u32;
                let total = page.total;
                builder.push(page).map_err(SessionError::Protocol)?;
                pages += 1;
                bytes += length as u64;
                observer.progress(CaptureProgress::Page {
                    kind,
                    cursor,
                    total,
                    count,
                    pages,
                    bytes,
                });
            }
        }
        observer.progress(CaptureProgress::VerifyingLocale { pages, bytes });
        if copied!(self.read_owned_control(now, |dispatcher| Request::ReadLocale { dispatcher }, locale::decode, Some(observer))) != locale {
            return Err(SessionError::Protocol(ProtocolError::InvalidData));
        }
        self.validate_profile(&dispatcher).map_err(|error| SessionError::Identity(error.to_string()))?;
        let catalog = builder.finish().map_err(SessionError::Protocol)?;
        observer.progress(CaptureProgress::Complete { pages, bytes });
        if observer.control() == CaptureControl::Stop {
            return Ok(CaptureOutcome::Stopped);
        }
        Ok(CaptureOutcome::Complete(Box::new(catalog)))
    }
    async fn read_owned<R>(&mut self, now: Timestamp, request: impl FnOnce(DispatcherSpec) -> Request, decode: fn(&[u8]) -> Result<R, ProtocolError>) -> Result<R, SessionError> {
        match self.read_owned_control(now, request, decode, None).await? {
            OwnedRead::Complete(value) => Ok(value),
            OwnedRead::Stopped => Err(SessionError::Protocol(ProtocolError::InvalidData)),
        }
    }
    async fn read_owned_control<R>(
        &mut self,
        _now: Timestamp,
        request: impl FnOnce(DispatcherSpec) -> Request,
        decode: impl FnOnce(&[u8]) -> Result<R, ProtocolError>,
        mut observer: Option<&mut dyn CaptureObserver>,
    ) -> Result<OwnedRead<R>, SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.require_enabled()?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        if observer.as_mut().is_some_and(|observer| observer.control() == CaptureControl::Stop) {
            return Ok(OwnedRead::Stopped);
        }
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        if let Err(error) = self.validate_profile(&dispatcher) {
            self.release_without_io();
            self.state = SessionState::Disconnected;
            return Err(SessionError::Identity(error.to_string()));
        }
        if observer.as_mut().is_some_and(|observer| observer.control() == CaptureControl::Stop) {
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
                if observer.as_mut().is_some_and(|observer| observer.control() == CaptureControl::Stop) {
                    return Ok(OwnedRead::Stopped);
                }
                Ok(OwnedRead::Complete(snapshot))
            }
            Err(error) => {
                if optional_description
                    && matches!(
                        &error,
                        SessionError::Helper {
                            code: ResponseCode::Refused | ResponseCode::Failed | ResponseCode::NotReady | ResponseCode::StaleItem,
                            ..
                        }
                    )
                {
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
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
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
        if self.discovery.take().is_some() {
            self.cleanup = Cleanup::Unconfirmed;
        }
        self.status = None;
        self.lease = None;
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
        let was_disabled = self.state == SessionState::Disabled;
        self.abandon_pending_write();
        self.status = None;
        self.state = SessionState::Closing;
        // Best effort, as in HookSync.TurnOffAsync. Cancellation never proves a submitted clear ran.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), self.clear_hooks()).await;
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
