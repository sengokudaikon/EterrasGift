//! Retained Windows bootstrap preparation and a proof-checking pipe transport.
//! Drop submits no RPC, stop or unload. A session disconnect clears features;
//! it does not unload or stop its retained helper. Uncertain starts are not retried.

use eg_game_access::{
    bootstrap::{
        BootstrapError, HelperArtifact,
        windows::{DispatcherProof, MainThreadIdentity, ModuleIdentity, RemoteDelivery, RemoteFailure, RemoteHelper},
    },
    pipe::PipeClient,
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode},
    transport::{Delivery, Transport, TransportError, TransportFailure},
};
use eg_trainer::{
    backend::{BackendError, ErrorKind},
    connection::{AdmissionCheck, HelperPreparer, LogFactory, PreparationFailure, PreparationResidency, PreparedHelper},
    gate::{self, GatePolicy, GateVerdict, LogSource},
    profile::BuildProfile,
};
use std::{
    any::Any,
    future::Future,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Runtime-specific evidence and any ownership guards it needs. This is not
/// constructible from a DLL hash or an OS thread name through a safe API.
pub struct RuntimeAttestation {
    target: ProcessIdentity,
    profile: BuildProfile,
    main: MainThreadIdentity,
    game: ModuleIdentity,
    unity: ModuleIdentity,
    keep_alive: PinnedHostGuard,
}

/// Local ownership may end while the remote helper or an uncertain call still
/// lives. Once a start is submitted, release requires a future explicit target
/// exit proof; ordinary Drop therefore keeps this one ownership reference alive.
struct PinnedHostGuard {
    owner: Option<Arc<dyn Any + Send + Sync>>,
    armed: bool,
}
impl PinnedHostGuard {
    fn new(owner: Arc<dyn Any + Send + Sync>) -> Self {
        Self { owner: Some(owner), armed: false }
    }
}
impl Drop for PinnedHostGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(owner) = self.owner.take()
        {
            std::mem::forget(owner);
        }
    }
}
impl RuntimeAttestation {
    /// # Safety
    /// The caller establishes every runtime, metadata, GC, method-body, real-main-thread,
    /// exclusive patch ownership and residency lifetime obligation of RemoteHelper::start
    /// for this exact process/profile and helper generation. Those obligations continue
    /// through explicitly requested writes, including the narrow mutable Int32 level
    /// backing-field store and exclusive rooted graph/readback ownership contract,
    /// and through pending calls and helper residency, including when this guard is dropped.
    /// Attack/leech reads additionally require populated static caches and exclusive
    /// reference, scalar and Unity native-object lifetime access for the rooted copy.
    /// Rate writes additionally authorize the selected CURRENT-player mutable Single field
    /// under that same exclusive lifetime through guarded comparison, one store and readback.
    /// Equality tokens provide no historical selection, replay or native-lifetime authority.
    /// After a potentially submitted start, `keep_alive` remains pinned even when
    /// the last preparer/transport drops. Pre-start and factual NotSent failures
    /// release it normally. Reclaiming after confirmed target exit is not implemented.
    /// `keep_alive` retains required host-side ownership through all prepared transports;
    /// it cannot by itself prove an external runtime healthy or prevent outside patches.
    pub unsafe fn new(
        target: ProcessIdentity,
        profile: BuildProfile,
        main: MainThreadIdentity,
        game: ModuleIdentity,
        unity: ModuleIdentity,
        keep_alive: Arc<dyn Any + Send + Sync>,
    ) -> Self {
        Self {
            target,
            profile,
            main,
            game,
            unity,
            keep_alive: PinnedHostGuard::new(keep_alive),
        }
    }
}

/// Resources registered before an authority may submit target work. Registration is
/// irreversible: owners stay pinned even if authority/preparation is canceled, unwinds
/// or returns an error. Reclamation after proved target exit is intentionally deferred.
/// This journal supplies ownership only; it supplies no runtime or thread authority.
pub struct AuthorityJournal {
    owners: Vec<PinnedHostGuard>,
}
impl AuthorityJournal {
    fn new() -> Self {
        Self { owners: Vec::new() }
    }
    /// Register the real resource owner BEFORE any possibly submitted target operation.
    /// The owner must cover pending calls, remote allocations, module leases and all
    /// required residency guards. Retaining an unrelated Arc does not satisfy that
    /// caller obligation. There is no release or factual-NotSent override API.
    pub fn retain_owner(&mut self, owner: Arc<dyn Any + Send + Sync>) {
        let mut guard = PinnedHostGuard::new(owner);
        guard.armed = true;
        self.owners.push(guard);
    }
}

pub trait RuntimeAuthority: Send + 'static {
    /// Called only after fresh offline admission. Implementations must use the supplied
    /// callback before each process access and bind evidence to this exact profile.
    /// Before possibly submitting any native work, register its actual resource owner
    /// in `journal`. The owner must already cover provisional allocations/calls and
    /// survive cancellation or unwind at every submission boundary. Keeping the only
    /// owner inside this future or a not-yet-returned attestation is insufficient.
    /// Guards returned with an attestation must already be registered here if the
    /// authority's own target work requires their retained lifetime.
    /// Query-only authorities register nothing. Journal ownership does not authorize
    /// a probe, bootstrap, runtime access or construction of RuntimeAttestation.
    fn attest(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        admission: &mut dyn AdmissionCheck,
        journal: &mut AuthorityJournal,
    ) -> impl Future<Output = Result<RuntimeAttestation, PreparationFailure>> + Send;
}

struct RuntimeRecord {
    helper: RemoteHelper,
    attestation: RuntimeAttestation,
    dispatcher: Option<DispatcherSpec>,
    proof: Option<DispatcherProof>,
    rejected: Option<String>,
}
struct Retained {
    target: ProcessIdentity,
    profile: BuildProfile,
    journal: AuthorityJournal,
    state: RetainedState,
}
enum RetainedState {
    Attesting,
    Attempting,
    Loaded { runtime: Arc<Mutex<RuntimeRecord>> },
    Uncertain { _failure: RemoteFailure, _attestation: Box<RuntimeAttestation> },
    Rejected { failure: PreparationFailure },
}
impl Retained {
    fn residency(&self) -> PreparationResidency {
        match &self.state {
            RetainedState::Loaded { .. } => PreparationResidency::Loaded,
            RetainedState::Rejected { failure } => failure.residency,
            RetainedState::Attesting | RetainedState::Attempting | RetainedState::Uncertain { .. } => PreparationResidency::MayBeLoaded,
        }
    }
}

/// The owner retains this preparer across sessions. Every failed load that may
/// have run stays registered; a subsequent prepare never loads that target again.
/// No slots are automatically evicted while target residency may still exist.
pub struct NativePreparer<A, F> {
    artifact: Arc<HelperArtifact>,
    authority: A,
    logs: F,
    retained: Vec<Retained>,
    timeout: Duration,
}
impl<A: RuntimeAuthority, F: LogFactory> NativePreparer<A, F> {
    /// Inert composition only. All target access happens in prepare, on the worker.
    pub fn new(artifact: Arc<HelperArtifact>, authority: A, logs: F) -> Self {
        Self {
            artifact,
            authority,
            logs,
            retained: Vec::new(),
            timeout: Duration::from_secs(10),
        }
    }
    pub fn retained_targets(&self) -> usize {
        self.retained.len()
    }
    pub fn uncertain_targets(&self) -> usize {
        self.retained.iter().filter(|value| !matches!(value.state, RetainedState::Loaded { .. })).count()
    }
    /// Full pristine proof has been checked at least once. This is historical
    /// preparation evidence, not a claim that the retained server is running.
    pub fn verified_targets(&self) -> usize {
        self.retained
            .iter()
            .filter(|value| match &value.state {
                RetainedState::Loaded { runtime } => runtime.lock().unwrap_or_else(|poison| poison.into_inner()).proof.is_some(),
                _ => false,
            })
            .count()
    }
    fn known_residency(&self, target: &ProcessIdentity) -> PreparationResidency {
        self.retained
            .iter()
            .find(|entry| entry.target == *target)
            .map_or(PreparationResidency::NotLoaded, Retained::residency)
    }
    fn reject_authority_attempt(&mut self, index: usize, mut failure: PreparationFailure) -> PreparationFailure {
        if failure.residency == PreparationResidency::NotLoaded && !self.retained[index].journal.owners.is_empty() {
            failure.residency = PreparationResidency::MayBeLoaded;
            failure.error.delivery = Some(Delivery::MayHaveRun);
            failure.error.retry_when_waiting = false;
        }
        if failure.residency == PreparationResidency::NotLoaded {
            self.retained.remove(index);
        } else {
            self.retained[index].state = RetainedState::Rejected { failure: failure.clone() };
        }
        failure
    }
    fn transport(&mut self, target: &ProcessIdentity, runtime: Arc<Mutex<RuntimeRecord>>) -> Result<VerifiedTransport<F::Log>, PreparationFailure> {
        let log = self.logs.create(target).map_err(|error| failed(error, PreparationResidency::Loaded))?;
        let admission = FreshAdmission::new(target, log).map_err(|error| failed(error, PreparationResidency::Loaded))?;
        Ok(VerifiedTransport {
            pipe: PipeClient::new(target.clone()),
            runtime,
            admission,
        })
    }
}

impl<A: RuntimeAuthority, F: LogFactory> HelperPreparer for NativePreparer<A, F> {
    type Transport = VerifiedTransport<F::Log>;
    async fn prepare(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        admission: &mut dyn AdmissionCheck,
    ) -> Result<PreparedHelper<Self::Transport>, PreparationFailure> {
        let known_residency = self.known_residency(target);
        admission.check().map_err(|error| failed(error, known_residency))?;
        if !profile.helper_build_id().eq_ignore_ascii_case(self.artifact.source_id()) {
            return Err(failed(
                BackendError::blocked(ErrorKind::Identity, "helper artifact source differs from session profile"),
                known_residency,
            ));
        }
        let existing = self.retained.iter().find(|value| value.target == *target);
        let runtime = if let Some(existing) = existing {
            if existing.profile != *profile {
                return Err(failed(
                    BackendError::blocked(ErrorKind::Identity, "retained authority attempt belongs to another artifact profile"),
                    existing.residency(),
                ));
            }
            match &existing.state {
                RetainedState::Rejected { failure } => return Err(failure.clone()),
                RetainedState::Attesting | RetainedState::Attempting | RetainedState::Uncertain { .. } => {
                    return Err(failed(
                        BackendError::blocked(ErrorKind::RestartRequired, "a prior authority or helper attempt may have run; replay is refused"),
                        PreparationResidency::MayBeLoaded,
                    ));
                }
                RetainedState::Loaded { runtime } => {
                    let record = runtime.lock().unwrap_or_else(|poison| poison.into_inner());
                    if record.attestation.profile != *profile || record.rejected.is_some() {
                        return Err(failed(
                            BackendError::blocked(
                                ErrorKind::RestartRequired,
                                record.rejected.clone().unwrap_or_else(|| "retained runtime profile changed".into()),
                            ),
                            PreparationResidency::Loaded,
                        ));
                    }
                    drop(record);
                    Arc::clone(runtime)
                }
            }
        } else {
            if self.retained.len() >= 8 {
                return Err(failed(
                    BackendError::blocked(ErrorKind::RestartRequired, "retained native runtime limit reached"),
                    PreparationResidency::NotLoaded,
                ));
            }
            // Reserve before polling authority. Cancellation/unwind leaves this exact
            // target/profile and its already registered owners retained without replay.
            let retained_index = self.retained.len();
            self.retained.push(Retained {
                target: target.clone(),
                profile: profile.clone(),
                journal: AuthorityJournal::new(),
                state: RetainedState::Attesting,
            });
            let mut attestation = match self.authority.attest(target, profile, admission, &mut self.retained[retained_index].journal).await {
                Ok(value) => value,
                Err(failure) => return Err(self.reject_authority_attempt(retained_index, failure)),
            };
            if let Err(error) = validate_authority_identity(target, profile, &attestation.target, &attestation.profile, attestation.game.sha256()) {
                return Err(self.reject_authority_attempt(retained_index, failed(error, PreparationResidency::NotLoaded)));
            }
            if let Err(error) = admission.check() {
                return Err(self.reject_authority_attempt(retained_index, failed(error, PreparationResidency::NotLoaded)));
            }
            self.retained[retained_index].state = RetainedState::Attempting;
            // Arm before entering the unsafe operation, including possible unwind.
            // Authority journal owners remain independently pinned even for NotSent.
            attestation.keep_alive.armed = true;
            // SAFETY: only unsafe construction of this exact identity-bound attestation can
            // supply the caller contract. Its ownership guards are retained in RuntimeRecord.
            let result = unsafe {
                RemoteHelper::start(
                    target.clone(),
                    attestation.main.clone(),
                    Arc::clone(&self.artifact),
                    &[attestation.game.clone(), attestation.unity.clone()],
                    &mut || admission.check().map_err(bootstrap_error),
                    self.timeout,
                )
            };
            let helper = match result {
                Ok(helper) => helper,
                Err(failure) => {
                    let error = BackendError::blocked(ErrorKind::Transport, failure.to_string());
                    if failure.delivery == RemoteDelivery::NotSent {
                        attestation.keep_alive.armed = false;
                        return Err(self.reject_authority_attempt(retained_index, failed(error, PreparationResidency::NotLoaded)));
                    }
                    self.retained[retained_index].state = RetainedState::Uncertain {
                        _failure: failure,
                        _attestation: Box::new(attestation),
                    };
                    return Err(failed(error, PreparationResidency::MayBeLoaded));
                }
            };
            let runtime = Arc::new(Mutex::new(RuntimeRecord {
                helper,
                attestation,
                dispatcher: None,
                proof: None,
                rejected: None,
            }));
            self.retained[retained_index].state = RetainedState::Loaded { runtime: Arc::clone(&runtime) };
            runtime
        };
        let transport = self.transport(target, runtime)?;
        Ok(PreparedHelper {
            target: target.clone(),
            helper_build_id: self.artifact.source_id().into(),
            residency: PreparationResidency::Loaded,
            transport,
        })
    }
}

fn validate_authority_identity(
    target: &ProcessIdentity,
    profile: &BuildProfile,
    actual_target: &ProcessIdentity,
    actual_profile: &BuildProfile,
    game_sha256: &str,
) -> Result<(), BackendError> {
    if actual_target != target || actual_profile != profile || game_sha256 != profile.binary_hash() {
        return Err(BackendError::blocked(
            ErrorKind::Identity,
            "runtime attestation belongs to another process or artifact profile",
        ));
    }
    Ok(())
}

fn failed(error: BackendError, residency: PreparationResidency) -> PreparationFailure {
    PreparationFailure { error, residency }
}
fn bootstrap_error(error: BackendError) -> BootstrapError {
    io::Error::other(error.to_string()).into()
}
fn transport_error(error: impl std::fmt::Display, delivery: Delivery) -> TransportError {
    TransportError {
        delivery,
        failure: TransportFailure::Io(io::Error::other(error.to_string())),
    }
}

fn require_request_proof(request: &Request, known: Option<DispatcherSpec>, has_proof: bool) -> Result<(), BootstrapError> {
    let Some(requested) = request.dispatcher() else { return Ok(()) };
    if !has_proof {
        return Err(BootstrapError::Invalid("request before full pristine dispatcher proof"));
    }
    if known != Some(requested) {
        return Err(BootstrapError::Invalid("request dispatcher differs from retained pristine proof"));
    }
    Ok(())
}

struct FreshAdmission<L> {
    log: L,
    started: jiff::Timestamp,
}
impl<L: LogSource> FreshAdmission<L> {
    fn new(target: &ProcessIdentity, log: L) -> Result<Self, BackendError> {
        let started = jiff::Timestamp::from_nanosecond((i128::from(target.created) - 116_444_736_000_000_000) * 100)
            .map_err(|error| BackendError::blocked(ErrorKind::Identity, error.to_string()))?;
        Ok(Self { log, started })
    }
    fn check(&mut self) -> Result<(), BootstrapError> {
        let decision = gate::evaluate(&self.log.read(self.started), self.started, jiff::Timestamp::now(), GatePolicy::Release);
        if decision.verdict == GateVerdict::Allow {
            Ok(())
        } else {
            Err(BootstrapError::Invalid("fresh Release offline admission refused native transport access"))
        }
    }
}

/// Session transport keeps native residency/evidence alive independently of its
/// preparer, and checks full pristine code before its first scheduler install.
pub struct VerifiedTransport<L> {
    pipe: PipeClient,
    runtime: Arc<Mutex<RuntimeRecord>>,
    admission: FreshAdmission<L>,
}
impl<L: LogSource + Send> Transport for VerifiedTransport<L> {
    fn target(&self) -> &ProcessIdentity {
        self.pipe.target()
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        self.admission.check().map_err(|error| transport_error(error, Delivery::NotSent))?;
        {
            let record = self.runtime.lock().unwrap_or_else(|poison| poison.into_inner());
            record.helper.artifact().verify_unchanged().map_err(|error| transport_error(error, Delivery::NotSent))?;
            if let Some(reason) = &record.rejected {
                return Err(transport_error(reason, Delivery::NotSent));
            }
            require_request_proof(request, record.dispatcher, record.proof.is_some()).map_err(|error| transport_error(error, Delivery::NotSent))?;
        }
        // Artifact verification may have taken time. Recheck policy immediately
        // before the pipe client performs its process/server checks and exchange.
        self.admission.check().map_err(|error| transport_error(error, Delivery::NotSent))?;
        let response = self.pipe.send(request).await?;
        if matches!(request, Request::Discover) && response.code == ResponseCode::Done {
            let proof_result = (|| {
                self.admission.check()?;
                let mut record = self.runtime.lock().unwrap_or_else(|poison| poison.into_inner());
                let discovery = Discovery::decode(&response.payload, self.pipe.target().pid).map_err(|error| io::Error::other(error.to_string()))?;
                if !discovery.build_id.eq_ignore_ascii_case(record.helper.artifact().source_id()) {
                    return Err(BootstrapError::Invalid("loaded helper source differs from retained artifact"));
                }
                if let Some(prior) = record.dispatcher {
                    if prior != discovery.dispatcher {
                        return Err(BootstrapError::Invalid("retained dispatcher identity changed"));
                    }
                } else {
                    let proof = DispatcherProof::capture(record.attestation.game.clone(), &discovery.dispatcher)?;
                    record.helper.verify_pristine(&proof, &mut || self.admission.check())?;
                    record.dispatcher = Some(discovery.dispatcher);
                    record.proof = Some(proof);
                }
                Ok::<_, BootstrapError>(())
            })();
            if let Err(error) = proof_result {
                self.runtime.lock().unwrap_or_else(|poison| poison.into_inner()).rejected = Some(error.to_string());
                return Err(transport_error(error, Delivery::MayHaveRun));
            }
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_game_access::catalog::CatalogKind;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    #[allow(dead_code)]
    mod support {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mod.rs"));
    }
    struct CountDrop(Arc<AtomicUsize>);
    impl Drop for CountDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, SeqCst);
        }
    }
    #[test]
    fn factual_not_submitted_guard_releases_owner_normally() {
        for armed_then_disarmed in [false, true] {
            let drops = Arc::new(AtomicUsize::new(0));
            let owner = Arc::new(CountDrop(drops.clone()));
            let mut guard = PinnedHostGuard::new(owner.clone());
            guard.armed = armed_then_disarmed;
            if armed_then_disarmed {
                guard.armed = false;
            }
            drop(owner);
            assert_eq!(drops.load(SeqCst), 0);
            drop(guard);
            assert_eq!(drops.load(SeqCst), 1);
        }
    }
    fn reclaim_one_authored_pin(owner: &Arc<CountDrop>) {
        assert_eq!(Arc::strong_count(owner), 2);
        // SAFETY: an authored guard just deliberately forgot exactly one clone of this
        // allocation. No native target exists. Reclaim that clone after testing retention.
        unsafe {
            Arc::decrement_strong_count(Arc::as_ptr(owner));
        }
    }
    #[test]
    fn submitted_owner_survives_last_guard_drop_without_running_owner_destructor() {
        let drops = Arc::new(AtomicUsize::new(0));
        let owner = Arc::new(CountDrop(drops.clone()));
        let mut guard = PinnedHostGuard::new(owner.clone());
        guard.armed = true;
        drop(guard);
        assert_eq!(drops.load(SeqCst), 0);
        reclaim_one_authored_pin(&owner);
        drop(owner);
        assert_eq!(drops.load(SeqCst), 1);
    }
    struct NeverAuthority(Arc<AtomicUsize>);
    impl RuntimeAuthority for NeverAuthority {
        async fn attest(&mut self, _: &ProcessIdentity, _: &BuildProfile, _: &mut dyn AdmissionCheck, _: &mut AuthorityJournal) -> Result<RuntimeAttestation, PreparationFailure> {
            self.0.fetch_add(1, SeqCst);
            panic!("Attempting target reached authority");
        }
    }
    #[test]
    fn caught_start_panic_retains_guard_and_attempting_marker_refuses_replay() {
        let fixture = support::Fixture::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let logs = support::Logs::default();
        let log_calls = logs.calls.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), NeverAuthority(calls.clone()), logs);
        // This is the stored state immediately before the unsafe start boundary.
        // Exercise unwind and later public prepare without opening any process.
        preparer.retained.push(Retained {
            target: fixture.target.clone(),
            profile: fixture.profile.clone(),
            journal: AuthorityJournal::new(),
            state: RetainedState::Attempting,
        });
        let drops = Arc::new(AtomicUsize::new(0));
        let owner = Arc::new(CountDrop(drops.clone()));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut guard = PinnedHostGuard::new(owner.clone());
            guard.armed = true;
            panic!("authored admission callback panic");
        }));
        assert!(result.is_err());
        assert_eq!(drops.load(SeqCst), 0);
        support::run(async {
            let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut support::Admission::new(true)).await.err().unwrap();
            assert_eq!(failure.residency, PreparationResidency::MayBeLoaded);
            assert_eq!(failure.error.kind, ErrorKind::RestartRequired);
        });
        assert_eq!(preparer.retained_targets(), 1);
        assert_eq!(preparer.uncertain_targets(), 1);
        assert_eq!(calls.load(SeqCst), 0);
        assert_eq!(log_calls.load(SeqCst), 0);
        drop(preparer);
        assert_eq!(drops.load(SeqCst), 0);
        reclaim_one_authored_pin(&owner);
        drop(owner);
        assert_eq!(drops.load(SeqCst), 1);
    }
    #[test]
    fn every_scheduler_request_requires_exact_recorded_proof_before_pipe() {
        let spec = DispatcherSpec { rva: 0x1000, bytes: [7; 16] };
        let requests = [
            Request::Status { dispatcher: spec },
            Request::ClearHooks { dispatcher: spec },
            Request::ReadForge { dispatcher: spec },
            Request::ReadItemLimits { dispatcher: spec },
            Request::ReadLocale { dispatcher: spec },
            Request::ReadCatalogPage {
                dispatcher: spec,
                kind: CatalogKind::Bases,
                cursor: 0,
            },
        ];
        let mut wrong_bytes = spec;
        wrong_bytes.bytes[15] ^= 1;
        let mut wrong_rva = spec;
        wrong_rva.rva += 1;
        for request in requests {
            assert!(require_request_proof(&request, Some(spec), true).is_ok());
            assert!(require_request_proof(&request, Some(spec), false).is_err());
            assert!(require_request_proof(&request, None, true).is_err());
            assert!(require_request_proof(&request, Some(wrong_bytes), true).is_err());
            assert!(require_request_proof(&request, Some(wrong_rva), true).is_err());
        }
        assert!(require_request_proof(&Request::Discover, None, false).is_ok());
        assert!(require_request_proof(&Request::Discover, Some(spec), true).is_ok());
    }
    fn reserved(preparer: &mut NativePreparer<NeverAuthority, support::Logs>, fixture: &support::Fixture, registered: bool) -> Option<(Arc<CountDrop>, Arc<AtomicUsize>)> {
        let mut journal = AuthorityJournal::new();
        let owner = registered.then(|| {
            let drops = Arc::new(AtomicUsize::new(0));
            let owner = Arc::new(CountDrop(drops.clone()));
            journal.retain_owner(owner.clone());
            (owner, drops)
        });
        preparer.retained.push(Retained {
            target: fixture.target.clone(),
            profile: fixture.profile.clone(),
            journal,
            state: RetainedState::Attesting,
        });
        owner
    }
    #[test]
    fn wrong_success_identity_facts_are_rejected_and_registered_owners_preserved() {
        for registered in [false, true] {
            for mismatch in 0..3 {
                let fixture = support::Fixture::new();
                let mut preparer = NativePreparer::new(fixture.artifact.clone(), NeverAuthority(Arc::new(AtomicUsize::new(0))), support::Logs::default());
                let owner = reserved(&mut preparer, &fixture, registered);
                let different = support::other_target(&fixture.target, 999);
                let profile = fixture.profile_with_source(&"a".repeat(64));
                let (target, actual_profile, hash) = match mismatch {
                    0 => (&different, &fixture.profile, fixture.profile.binary_hash()),
                    1 => (&fixture.target, &profile, fixture.profile.binary_hash()),
                    _ => (&fixture.target, &fixture.profile, "wrong module hash"),
                };
                let error = validate_authority_identity(&fixture.target, &fixture.profile, target, actual_profile, hash).unwrap_err();
                let failure = preparer.reject_authority_attempt(0, failed(error, PreparationResidency::NotLoaded));
                assert_eq!(failure.error.kind, ErrorKind::Identity);
                assert_eq!(
                    failure.residency,
                    if registered {
                        PreparationResidency::MayBeLoaded
                    } else {
                        PreparationResidency::NotLoaded
                    }
                );
                assert_eq!(preparer.retained_targets(), usize::from(registered));
                drop(preparer);
                if let Some((owner, drops)) = owner {
                    assert_eq!(drops.load(SeqCst), 0);
                    reclaim_one_authored_pin(&owner);
                    drop(owner);
                    assert_eq!(drops.load(SeqCst), 1);
                }
            }
        }
        let fixture = support::Fixture::new();
        assert!(validate_authority_identity(&fixture.target, &fixture.profile, &fixture.target, &fixture.profile, fixture.profile.binary_hash()).is_ok());
    }
    #[test]
    fn post_authority_gate_and_helper_not_sent_preserve_prior_residency_in_exact_slot() {
        for registered in [false, true] {
            for helper_not_sent in [false, true] {
                let fixture = support::Fixture::new();
                let mut preparer = NativePreparer::new(fixture.artifact.clone(), NeverAuthority(Arc::new(AtomicUsize::new(0))), support::Logs::default());
                // A previous canceled target must remain unchanged when the following
                // target succeeds authority but is refused before any helper submission.
                reserved(&mut preparer, &fixture, false);
                preparer.retained[0].target.pid = 123;
                let owner = reserved(&mut preparer, &fixture, registered);
                let journal_address = &preparer.retained[1].journal as *const AuthorityJournal;
                if helper_not_sent {
                    preparer.retained[1].state = RetainedState::Attempting;
                }
                let error = if helper_not_sent {
                    BackendError::blocked(ErrorKind::Transport, "authored factual helper NotSent; no loader called")
                } else {
                    BackendError::blocked(ErrorKind::Gate, "authored post-authority gate loss")
                };
                let failure = preparer.reject_authority_attempt(1, failed(error, PreparationResidency::NotLoaded));
                assert_eq!(
                    failure.residency,
                    if registered {
                        PreparationResidency::MayBeLoaded
                    } else {
                        PreparationResidency::NotLoaded
                    }
                );
                assert_eq!(preparer.retained_targets(), if registered { 2 } else { 1 });
                assert_eq!(preparer.retained[0].target.pid, 123);
                assert!(matches!(preparer.retained[0].state, RetainedState::Attesting));
                if registered {
                    assert_eq!(&preparer.retained[1].journal as *const AuthorityJournal, journal_address);
                    assert_eq!(preparer.retained[1].target, fixture.target);
                    assert_eq!(preparer.retained[1].profile, fixture.profile);
                    assert!(matches!(preparer.retained[1].state, RetainedState::Rejected { .. }));
                }
                drop(preparer);
                if let Some((owner, drops)) = owner {
                    assert_eq!(drops.load(SeqCst), 0);
                    reclaim_one_authored_pin(&owner);
                    drop(owner);
                    assert_eq!(drops.load(SeqCst), 1);
                }
            }
        }
    }
    #[test]
    fn journal_registration_pins_every_owner_without_any_future_or_native_call() {
        let drops = Arc::new(AtomicUsize::new(0));
        let first = Arc::new(CountDrop(drops.clone()));
        let second = Arc::new(CountDrop(drops.clone()));
        let mut journal = AuthorityJournal::new();
        journal.retain_owner(first.clone());
        journal.retain_owner(second.clone());
        drop(journal);
        assert_eq!(drops.load(SeqCst), 0);
        reclaim_one_authored_pin(&first);
        reclaim_one_authored_pin(&second);
        drop(first);
        drop(second);
        assert_eq!(drops.load(SeqCst), 2);
    }
}
