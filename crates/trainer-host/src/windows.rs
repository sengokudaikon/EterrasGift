//! Game-specific retained bootstrap and verified transport. Construction is inert.
//! Native work is serialized by the trainer worker; no main-thread nomination or
//! pre-bootstrap runtime certification is required. Drop never stops/unloads code.

mod version;

use eg_game_access::{
    bootstrap::{
        BootstrapError, HelperArtifact,
        windows::{DispatcherProof, ModuleIdentity, RemoteDelivery, RemoteFailure, RemoteHelper},
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
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

struct RuntimeRecord {
    helper: RemoteHelper,
    game: ModuleIdentity,
    dispatcher: Option<DispatcherSpec>,
    proof: Option<DispatcherProof>,
    rejected: Option<String>,
}
struct Retained {
    target: ProcessIdentity,
    profile: BuildProfile,
    state: RetainedState,
}
enum RetainedState {
    Attempting,
    Loaded(Arc<Mutex<RuntimeRecord>>),
    // RemoteFailure owns any unfinished remote call and its input allocation.
    Uncertain(RemoteFailure),
}
impl Retained {
    fn residency(&self) -> PreparationResidency {
        match self.state {
            RetainedState::Loaded(_) => PreparationResidency::Loaded,
            _ => PreparationResidency::MayBeLoaded,
        }
    }
}

/// One registry per owning trainer worker, retained across connect/disconnect.
/// A possibly submitted load is never repeated in the same process generation.
/// Process birth/path are part of the key; there is no arbitrary eight-game limit.
/// Published helper code stays resident until game exit, independently of this owner.
pub struct NativePreparer<F> {
    artifact: Arc<HelperArtifact>,
    logs: F,
    retained: Vec<Retained>,
    timeout: Duration,
}
impl<F: LogFactory> NativePreparer<F> {
    /// Compose the supported-game Windows adapter without opening a process.
    ///
    /// # Safety
    /// Targets later supplied to prepare must be the supported Unity6000.4 Windows
    /// x64 game (or an owned matching fixture), with its modern nonmoving IL2CPP
    /// runtime. No competing patcher may modify the scheduler during installation.
    /// Game runtime/module shutdown is excluded while callbacks access the runtime.
    /// Each synchronous graph operation assumes no concurrent graph/scalar mutation,
    /// reentrancy or native-object destruction; edits assume exclusive write access.
    /// The scheduler's Win32 wrappers must not enter instrumentation that waits on
    /// locks owned by suspended threads. No startup check establishes these premises.
    /// These are game-specific interoperability assumptions, not facts certified by
    /// a host-side authority object. prepare checks UnityPlayer's fixed file version
    /// (6000.4), then binds the same disk bytes to the loaded module by hash.
    /// Offline/identity/artifact checks run in prepare;
    /// the helper checks callback thread and each operation's metadata/current graph.
    pub unsafe fn new(artifact: Arc<HelperArtifact>, logs: F) -> Self {
        Self {
            artifact,
            logs,
            retained: Vec::new(),
            timeout: Duration::from_secs(10),
        }
    }
    pub fn retained_targets(&self) -> usize {
        self.retained.len()
    }
    pub fn uncertain_targets(&self) -> usize {
        self.retained.iter().filter(|entry| !matches!(entry.state, RetainedState::Loaded(_))).count()
    }
    pub fn verified_targets(&self) -> usize {
        self.retained
            .iter()
            .filter(|entry| match &entry.state {
                RetainedState::Loaded(record) => record.lock().unwrap_or_else(|p| p.into_inner()).proof.is_some(),
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
    fn prepare_once(&mut self, target: &ProcessIdentity, profile: &BuildProfile, admission: &mut dyn AdmissionCheck) -> Result<Arc<Mutex<RuntimeRecord>>, PreparationFailure> {
        let known = self.known_residency(target);
        admission.check().map_err(|error| failed(error, known))?;
        if !profile.helper_build_id().eq_ignore_ascii_case(self.artifact.source_id()) {
            return Err(failed(
                BackendError::blocked(ErrorKind::Identity, "helper artifact source differs from session profile"),
                known,
            ));
        }
        if let Some(entry) = self.retained.iter().find(|entry| entry.target == *target) {
            if entry.profile != *profile {
                return Err(failed(
                    BackendError::blocked(ErrorKind::Identity, "retained load belongs to another artifact profile"),
                    entry.residency(),
                ));
            }
            return match &entry.state {
                RetainedState::Loaded(record) => Ok(record.clone()),
                RetainedState::Uncertain(failure) => Err(failed(BackendError::blocked(ErrorKind::RestartRequired, failure.to_string()), entry.residency())),
                RetainedState::Attempting => Err(failed(
                    BackendError::blocked(ErrorKind::RestartRequired, "prior helper startup did not complete; do not replay"),
                    entry.residency(),
                )),
            };
        }
        let modules = (|| {
            profile.validate_target(&target.executable).map_err(identity_error)?;
            let folder = target.executable.parent().ok_or_else(|| identity_error("game directory missing"))?;
            let game = ModuleIdentity::capture(&folder.join("GameAssembly.dll"), profile.binary_hash()).map_err(game_build_error)?;
            let unity_path = folder.join("UnityPlayer.dll");
            let unity_hash = eg_core::hashes::of_bytes(&std::fs::read(&unity_path).map_err(identity_error)?);
            version::check_unity(&unity_path).map_err(identity_error)?;
            let unity = ModuleIdentity::capture(&unity_path, &unity_hash).map_err(identity_error)?;
            Ok::<_, BackendError>((game, unity))
        })()
        .map_err(|error| failed(error, PreparationResidency::NotLoaded))?;
        admission.check().map_err(|error| failed(error, PreparationResidency::NotLoaded))?;
        let index = self.retained.len();
        // Reserve before the synchronous native boundary. An unwind leaves the
        // replay fence; PendingCall independently retains any submitted allocation.
        self.retained.push(Retained {
            target: target.clone(),
            profile: profile.clone(),
            state: RetainedState::Attempting,
        });
        // SAFETY: new binds this adapter to the supported game's runtime assumptions.
        // RemoteHelper performs process/artifact checks and owns all remote calls.
        let result = unsafe {
            RemoteHelper::start(
                target.clone(),
                self.artifact.clone(),
                &[modules.0.clone(), modules.1],
                &mut || admission.check().map_err(bootstrap_error),
                self.timeout,
            )
        };
        match result {
            Ok(helper) => {
                let record = Arc::new(Mutex::new(RuntimeRecord {
                    helper,
                    game: modules.0,
                    dispatcher: None,
                    proof: None,
                    rejected: None,
                }));
                self.retained[index].state = RetainedState::Loaded(record.clone());
                Ok(record)
            }
            Err(failure) => {
                let error = BackendError::blocked(ErrorKind::Transport, failure.to_string());
                if failure.delivery == RemoteDelivery::NotSent {
                    self.retained.remove(index);
                    Err(failed(error, PreparationResidency::NotLoaded))
                } else {
                    self.retained[index].state = RetainedState::Uncertain(failure);
                    Err(failed(error, PreparationResidency::MayBeLoaded))
                }
            }
        }
    }
}
impl<F: LogFactory> HelperPreparer for NativePreparer<F> {
    type Transport = VerifiedTransport<F::Log>;
    async fn prepare(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        admission: &mut dyn AdmissionCheck,
    ) -> Result<PreparedHelper<Self::Transport>, PreparationFailure> {
        let runtime = self.prepare_once(target, profile, admission)?;
        let log = self.logs.create(target).map_err(|error| failed(error, PreparationResidency::Loaded))?;
        let admission = FreshAdmission::new(target, log).map_err(|error| failed(error, PreparationResidency::Loaded))?;
        Ok(PreparedHelper {
            target: target.clone(),
            helper_build_id: self.artifact.source_id().into(),
            residency: PreparationResidency::Loaded,
            transport: VerifiedTransport {
                pipe: PipeClient::new(target.clone()),
                runtime,
                admission,
            },
        })
    }
}

fn identity_error(error: impl std::fmt::Display) -> BackendError {
    BackendError::blocked(ErrorKind::Identity, error.to_string())
}
fn game_build_error(error: BootstrapError) -> BackendError {
    match error {
        BootstrapError::Invalid("module hash") => identity_error("This Last Epoch version isn't supported yet."),
        error => identity_error(error),
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
                    let proof = DispatcherProof::capture(record.game.clone(), &discovery.dispatcher)?;
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
    #[allow(dead_code)]
    mod support {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mod.rs"));
    }

    #[test]
    fn unfinished_attempts_preserve_process_birth_profile_and_replay_fence_without_a_slot_limit() {
        support::run(async {
            let fixture = support::Fixture::new();
            // SAFETY: only authored replay records are inspected; no target is opened.
            let mut native = unsafe { NativePreparer::new(fixture.artifact.clone(), support::Logs::default()) };
            for pid in 1..=12 {
                native.retained.push(Retained {
                    target: ProcessIdentity { pid, ..fixture.target.clone() },
                    profile: fixture.profile.clone(),
                    state: RetainedState::Attempting,
                });
            }
            let target = native.retained[0].target.clone();
            let mut admission = support::Admission::new(true);
            let failure = native.prepare(&target, &fixture.profile, &mut admission).await.err().unwrap();
            assert_eq!(failure.residency, PreparationResidency::MayBeLoaded);
            assert_eq!(failure.error.kind, ErrorKind::RestartRequired);
            assert_eq!(native.retained_targets(), 12);
            let changed_birth = ProcessIdentity {
                created: target.created + 1,
                ..target.clone()
            };
            assert_eq!(native.known_residency(&changed_birth), PreparationResidency::NotLoaded);
            admission.allow = false;
            let denied = native.prepare(&target, &fixture.profile, &mut admission).await.err().unwrap();
            assert_eq!(denied.error.kind, ErrorKind::Gate);
            assert_eq!(denied.residency, PreparationResidency::MayBeLoaded);
        });
    }

    #[test]
    fn changed_game_build_is_readable_and_refused_before_any_remote_work() {
        support::run(async {
            let fixture = support::Fixture::new();
            std::fs::write(fixture.temp.path().join("GameAssembly.dll"), b"new game patch").unwrap();
            // SAFETY: preflight rejects authored files; no process is opened.
            let mut native = unsafe { NativePreparer::new(fixture.artifact.clone(), support::Logs::default()) };
            let mut admission = support::Admission::new(true);
            let failure = native.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
            assert_eq!(failure.error.message, "This Last Epoch version isn't supported yet.");
            assert_eq!(failure.error.kind, ErrorKind::Identity);
            assert_eq!(failure.residency, PreparationResidency::NotLoaded);
            assert_eq!(native.retained_targets(), 0);
            assert!(game_build_error(BootstrapError::Invalid("module file changed")).message.contains("module file changed"));
        });
    }

    #[test]
    fn every_scheduler_request_requires_the_recorded_pristine_dispatcher() {
        let dispatcher = DispatcherSpec { rva: 4096, bytes: [1; 16] };
        let request = Request::Status { dispatcher };
        assert!(require_request_proof(&Request::Discover, None, false).is_ok());
        assert!(require_request_proof(&request, Some(dispatcher), false).is_err());
        assert!(require_request_proof(&request, Some(DispatcherSpec { rva: 8192, ..dispatcher }), true).is_err());
        assert!(require_request_proof(&request, Some(dispatcher), true).is_ok());
    }
}
