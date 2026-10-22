//! Game-specific retained bootstrap and verified transport. Construction is inert.
//! Native work is serialized by the trainer worker; no main-thread nomination or
//! pre-bootstrap runtime certification is required. Drop never stops/unloads code.

mod version;

use crate::lifecycle::{RESTART_REQUIRED, Residency};

use eg_game_access::{
    bootstrap::{
        BootstrapError, HelperArtifact,
        windows::{DispatcherProof, ModuleIdentity, RemoteDelivery, RemoteFailure, RemoteHelper},
    },
    pipe::PipeClient,
};
use eg_trainer::{
    backend::{BackendError, ErrorKind},
    connection::{AdmissionCheck, HelperPreparer, PreparationFailure, PreparationResidency, PreparedHelper},
    profile::BuildProfile,
};
use eg_trainer_wire::{
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode},
    transport::{Delivery, Transport, TransportError, TransportFailure},
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct RuntimeRecord {
    helper: RemoteHelper,
    game: ModuleIdentity,
    dispatcher: Option<DispatcherSpec>,
    proof: Option<DispatcherProof>,
    rejected: Option<String>,
    residency: Residency,
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
/// A new load requires proven departure; a failed departure fences this process.
pub struct NativePreparer {
    artifact: Arc<HelperArtifact>,
    retained: Vec<Retained>,
    timeout: Duration,
    successful_loads: usize,
}
impl NativePreparer {
    /// Compose the supported-game Windows adapter without opening a process.
    ///
    /// Runtime compatibility is checked during preparation and by the helper.
    pub fn new(artifact: Arc<HelperArtifact>) -> Self {
        Self {
            artifact,
            retained: Vec::new(),
            timeout: Duration::from_secs(10),
            successful_loads: 0,
        }
    }
    /// Read-only departure evidence for an already retained exact process.
    /// This remains permitted after the offline log gate has closed.
    pub fn departure_proven(&self, target: &ProcessIdentity) -> Result<bool, BootstrapError> {
        let entry = self
            .retained
            .iter()
            .find(|entry| entry.target == *target)
            .ok_or(BootstrapError::Invalid("no retained helper load"))?;
        match &entry.state {
            RetainedState::Loaded(record) => {
                let record = record.lock().unwrap_or_else(|p| p.into_inner());
                record.helper.departed(record.proof.as_ref())
            }
            _ => Err(BootstrapError::Invalid("helper startup was not verified")),
        }
    }
    pub fn successful_loads(&self) -> usize {
        self.successful_loads
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
        if let Some(index) = self.retained.iter().position(|entry| entry.target == *target) {
            if let RetainedState::Loaded(record) = &self.retained[index].state {
                let mut record = record.lock().unwrap_or_else(|p| p.into_inner());
                let gone = record.helper.departed(record.proof.as_ref()).unwrap_or(false);
                record.residency.observe(Instant::now(), gone, gone);
            }
            if matches!(&self.retained[index].state, RetainedState::Loaded(record)
                if record.lock().unwrap_or_else(|p| p.into_inner()).residency == Residency::Gone)
            {
                self.retained.remove(index);
            }
        }
        if let Some(entry) = self.retained.iter().find(|entry| entry.target == *target) {
            if entry.profile != *profile {
                return Err(failed(
                    BackendError::blocked(ErrorKind::Identity, "retained load belongs to another artifact profile"),
                    entry.residency(),
                ));
            }
            return match &entry.state {
                RetainedState::Loaded(record) => match record.lock().unwrap_or_else(|p| p.into_inner()).residency {
                    Residency::Loaded => Ok(record.clone()),
                    // Unloading is under way and bounded by its deadline: it ends Gone (a fresh load may follow) or Stuck.
                    Residency::Leaving { .. } => Err(failed(
                        BackendError::waiting(ErrorKind::NotReady, "The helper is still leaving Last Epoch. Connecting again once it has gone."),
                        entry.residency(),
                    )),
                    Residency::Gone | Residency::Stuck => Err(failed(BackendError::blocked(ErrorKind::RestartRequired, RESTART_REQUIRED), entry.residency())),
                },
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
            let game = match profile.binary_lease() {
                Some(lease) => ModuleIdentity::from_lease(lease),
                None => ModuleIdentity::capture(&folder.join("GameAssembly.dll"), profile.binary_hash()),
            }
            .map_err(game_build_error)?;
            let unity_path = folder.join("UnityPlayer.dll");
            let unity = ModuleIdentity::open(&unity_path).map_err(identity_error)?;
            version::check_unity(&unity_path).map_err(identity_error)?;
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
        let result = RemoteHelper::start(
            target.clone(),
            self.artifact.clone(),
            &[modules.0.clone(), modules.1],
            &mut || admission.check().map_err(bootstrap_error),
            self.timeout,
        );
        match result {
            Ok(helper) => {
                self.successful_loads = self.successful_loads.saturating_add(1);
                let record = Arc::new(Mutex::new(RuntimeRecord {
                    helper,
                    game: modules.0,
                    dispatcher: None,
                    proof: None,
                    rejected: None,
                    residency: Residency::Loaded,
                }));
                self.retained[index].state = RetainedState::Loaded(record.clone());
                Ok(record)
            }
            Err(failure) => {
                let error = match &failure.error {
                    BootstrapError::Admission { message, retry_when_waiting } => BackendError {
                        kind: ErrorKind::Gate,
                        message: message.clone(),
                        delivery: Some(if failure.delivery == RemoteDelivery::NotSent {
                            Delivery::NotSent
                        } else {
                            Delivery::MayHaveRun
                        }),
                        retry_when_waiting: *retry_when_waiting,
                    },
                    _ => BackendError::blocked(ErrorKind::Transport, failure.to_string()),
                };
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
impl HelperPreparer for NativePreparer {
    type Transport = VerifiedTransport;
    fn reap_exited(&mut self) -> Vec<ProcessIdentity> {
        let mut exited = Vec::new();
        self.retained.retain(|entry| {
            let dead = match &entry.state {
                RetainedState::Loaded(record) => record.lock().unwrap_or_else(|p| p.into_inner()).helper.process_exited(),
                RetainedState::Uncertain(failure) => failure.process_exited(),
                RetainedState::Attempting => false,
            };
            if dead {
                exited.push(entry.target.clone());
            }
            !dead
        });
        exited
    }
    async fn prepare(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        admission: &mut dyn AdmissionCheck,
    ) -> Result<PreparedHelper<Self::Transport>, PreparationFailure> {
        let runtime = self.prepare_once(target, profile, admission)?;
        Ok(PreparedHelper {
            target: target.clone(),
            helper_build_id: self.artifact.source_id().into(),
            residency: PreparationResidency::Loaded,
            transport: VerifiedTransport {
                pipe: PipeClient::new(target.clone()),
                runtime,
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
    if error.kind == ErrorKind::Gate {
        BootstrapError::Admission {
            message: error.message,
            retry_when_waiting: error.retry_when_waiting,
        }
    } else {
        io::Error::other(error.to_string()).into()
    }
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

/// Session transport keeps native residency/evidence alive independently of its
/// preparer, and checks full pristine code before its first scheduler install.
pub struct VerifiedTransport {
    pipe: PipeClient,
    runtime: Arc<Mutex<RuntimeRecord>>,
}
impl Transport for VerifiedTransport {
    async fn unload(&mut self) -> Result<(), TransportError> {
        let send = {
            let mut record = self.runtime.lock().unwrap_or_else(|p| p.into_inner());
            let gone = record.helper.departed(record.proof.as_ref()).unwrap_or(false);
            record.residency.observe(Instant::now(), gone, gone);
            let send = record.residency == Residency::Loaded;
            record.residency.leaving(Instant::now(), Duration::from_secs(10));
            send
        };
        // Missing ACK can mean self-unload. A cancelled prior unload may only be
        // observed, never sent again; module absence alone is also insufficient.
        if send {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.pipe.send(&Request::Unload)).await;
        }
        loop {
            let state = {
                let mut record = self.runtime.lock().unwrap_or_else(|p| p.into_inner());
                let gone = record.helper.departed(record.proof.as_ref()).unwrap_or(false);
                record.residency.observe(Instant::now(), gone, gone);
                record.residency
            };
            match state {
                Residency::Gone => return Ok(()),
                Residency::Stuck => return Err(transport_error(RESTART_REQUIRED, Delivery::MayHaveRun)),
                _ => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    }
    fn target(&self) -> &ProcessIdentity {
        self.pipe.target()
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        {
            let record = self.runtime.lock().unwrap_or_else(|poison| poison.into_inner());
            record.helper.artifact().verify_unchanged().map_err(|error| transport_error(error, Delivery::NotSent))?;
            match record.residency {
                Residency::Loaded => {}
                // A helper that is leaving or gone takes no more requests; the session ends as a departure.
                Residency::Leaving { .. } | Residency::Gone => {
                    return Err(TransportError {
                        delivery: Delivery::NotSent,
                        failure: TransportFailure::Departed,
                    });
                }
                Residency::Stuck => return Err(transport_error(RESTART_REQUIRED, Delivery::NotSent)),
            }
            if let Some(reason) = &record.rejected {
                return Err(transport_error(reason, Delivery::NotSent));
            }
            require_request_proof(request, record.dispatcher, record.proof.is_some()).map_err(|error| transport_error(error, Delivery::NotSent))?;
        }
        let response = match self.pipe.send(request).await {
            Ok(response) => response,
            Err(error) => {
                let mut record = self.runtime.lock().unwrap_or_else(|p| p.into_inner());
                let gone = record.helper.departed(record.proof.as_ref()).unwrap_or(false);
                record.residency.observe(Instant::now(), gone, gone);
                return Err(if record.residency == Residency::Gone {
                    TransportError {
                        delivery: error.delivery,
                        failure: TransportFailure::Departed,
                    }
                } else {
                    error
                });
            }
        };
        if matches!(request, Request::Discover) && response.code == ResponseCode::Done {
            let proof_result = (|| {
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
                    record.helper.verify_pristine(&proof, &mut || Ok(()))?;
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
            let mut native = NativePreparer::new(fixture.artifact.clone());
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
            let mut native = NativePreparer::new(fixture.artifact.clone());
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
