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
    /// through pending calls and helper residency, including when this guard is dropped.
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

pub trait RuntimeAuthority: Send + 'static {
    /// Called only after fresh offline admission. Implementations must use the supplied
    /// callback before each process access and bind evidence to this exact profile.
    fn attest(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        admission: &mut dyn AdmissionCheck,
    ) -> impl Future<Output = Result<RuntimeAttestation, PreparationFailure>> + Send;
}

struct RuntimeRecord {
    helper: RemoteHelper,
    attestation: RuntimeAttestation,
    dispatcher: Option<DispatcherSpec>,
    proof: Option<DispatcherProof>,
    rejected: Option<String>,
}
enum Retained {
    Attempting {
        target: ProcessIdentity,
    },
    Loaded {
        target: ProcessIdentity,
        runtime: Arc<Mutex<RuntimeRecord>>,
    },
    Uncertain {
        target: ProcessIdentity,
        _failure: RemoteFailure,
        _attestation: Box<RuntimeAttestation>,
    },
    Rejected {
        target: ProcessIdentity,
        failure: PreparationFailure,
    },
}
impl Retained {
    fn target(&self) -> &ProcessIdentity {
        match self {
            Self::Attempting { target } | Self::Loaded { target, .. } | Self::Uncertain { target, .. } | Self::Rejected { target, .. } => target,
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
        self.retained
            .iter()
            .filter(|value| matches!(value, Retained::Attempting { .. } | Retained::Uncertain { .. } | Retained::Rejected { .. }))
            .count()
    }
    /// Full pristine proof has been checked at least once. This is historical
    /// preparation evidence, not a claim that the retained server is running.
    pub fn verified_targets(&self) -> usize {
        self.retained
            .iter()
            .filter(|value| match value {
                Retained::Loaded { runtime, .. } => runtime.lock().unwrap_or_else(|poison| poison.into_inner()).proof.is_some(),
                _ => false,
            })
            .count()
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
        admission.check().map_err(|error| failed(error, PreparationResidency::NotLoaded))?;
        if !profile.helper_build_id().eq_ignore_ascii_case(self.artifact.source_id()) {
            return Err(failed(
                BackendError::blocked(ErrorKind::Identity, "helper artifact source differs from session profile"),
                PreparationResidency::NotLoaded,
            ));
        }
        let existing = self.retained.iter().find(|value| value.target() == target);
        let runtime = if let Some(existing) = existing {
            match existing {
                Retained::Rejected { failure, .. } => return Err(failure.clone()),
                Retained::Attempting { .. } | Retained::Uncertain { .. } => {
                    return Err(failed(
                        BackendError::blocked(ErrorKind::RestartRequired, "a prior helper load may have run; reinjection is refused"),
                        PreparationResidency::MayBeLoaded,
                    ));
                }
                Retained::Loaded { runtime, .. } => {
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
            let mut attestation = match self.authority.attest(target, profile, admission).await {
                Ok(value) => value,
                Err(failure) => {
                    if failure.residency != PreparationResidency::NotLoaded {
                        self.retained.push(Retained::Rejected {
                            target: target.clone(),
                            failure: failure.clone(),
                        });
                    }
                    return Err(failure);
                }
            };
            if attestation.target != *target || attestation.profile != *profile || attestation.game.sha256() != profile.binary_hash() {
                return Err(failed(
                    BackendError::blocked(ErrorKind::Identity, "runtime attestation belongs to another process or artifact profile"),
                    PreparationResidency::NotLoaded,
                ));
            }
            admission.check().map_err(|error| failed(error, PreparationResidency::NotLoaded))?;
            // Reserve the no-replay fence before the synchronous unsafe call:
            // even a caught admission-callback panic leaves this target blocked.
            let retained_index = self.retained.len();
            self.retained.push(Retained::Attempting { target: target.clone() });
            // Arm before entering the unsafe operation, including possible unwind.
            // Only a factual NotSent result can make releasing this guard safe.
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
                    let residency = if failure.delivery == RemoteDelivery::MayHaveRun {
                        PreparationResidency::MayBeLoaded
                    } else {
                        attestation.keep_alive.armed = false;
                        self.retained.remove(retained_index);
                        PreparationResidency::NotLoaded
                    };
                    let error = BackendError::blocked(ErrorKind::Transport, failure.to_string());
                    if residency == PreparationResidency::MayBeLoaded {
                        self.retained[retained_index] = Retained::Uncertain {
                            target: target.clone(),
                            _failure: failure,
                            _attestation: Box::new(attestation),
                        };
                    }
                    return Err(failed(error, residency));
                }
            };
            let runtime = Arc::new(Mutex::new(RuntimeRecord {
                helper,
                attestation,
                dispatcher: None,
                proof: None,
                rejected: None,
            }));
            self.retained[retained_index] = Retained::Loaded {
                target: target.clone(),
                runtime: Arc::clone(&runtime),
            };
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
    let requested = match request {
        Request::Discover => return Ok(()),
        Request::Status { dispatcher }
        | Request::ClearHooks { dispatcher }
        | Request::ReadForge { dispatcher }
        | Request::ReadItemLimits { dispatcher }
        | Request::ReadLocale { dispatcher }
        | Request::ReadCatalogPage { dispatcher, .. } => dispatcher,
    };
    if !has_proof {
        return Err(BootstrapError::Invalid("request before full pristine dispatcher proof"));
    }
    if known.as_ref() != Some(requested) {
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
        async fn attest(&mut self, _: &ProcessIdentity, _: &BuildProfile, _: &mut dyn AdmissionCheck) -> Result<RuntimeAttestation, PreparationFailure> {
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
        preparer.retained.push(Retained::Attempting { target: fixture.target.clone() });
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
}
