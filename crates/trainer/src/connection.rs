//! Safe orchestration around the explicitly injected helper-preparation boundary.
//! A preparer supplies remote-process operations; this module supplies exact local
//! artifact/identity binding and repeated process-bound offline admission.
//! No preparation or pipe exchange is retried. Once preparation may have loaded
//! code, a failure retains that fact and conservatively reports MayHaveRun.

use std::{future::Future, path::PathBuf, time::Instant};

use eg_core::hashes;
use eg_trainer_wire::{
    process::ProcessIdentity,
    transport::{Delivery, Transport},
};
use jiff::Timestamp;

use crate::{
    backend::{BackendError, Connector, ErrorKind},
    gate::{self, GatePolicy, GateVerdict, LogSource},
    profile::BuildProfile,
    session::Session,
};

/// Every call rereads admission. A remote preparer must call this immediately
/// before each process-query, allocation, module-load or bootstrap stage, and
/// revalidate the target process identity at its own process boundary.
pub trait AdmissionCheck: Send {
    fn check(&mut self) -> Result<(), BackendError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationResidency {
    NotLoaded,
    MayBeLoaded,
    Loaded,
}

pub struct PreparedHelper<T> {
    pub target: ProcessIdentity,
    pub helper_build_id: String,
    pub residency: PreparationResidency,
    pub transport: T,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparationFailure {
    pub error: BackendError,
    pub residency: PreparationResidency,
}
impl PreparationFailure {
    fn into_error(mut self) -> BackendError {
        if self.residency != PreparationResidency::NotLoaded && (self.error.kind != ErrorKind::Gate || !self.error.retry_when_waiting) {
            self.error.delivery = Some(Delivery::MayHaveRun);
            self.error.retry_when_waiting = false;
        }
        self.error
    }
}

pub trait HelperPreparer: Send + 'static {
    type Transport: Transport + Send + 'static;
    fn reap_exited(&mut self) -> Vec<ProcessIdentity> {
        Vec::new()
    }
    /// A successful Loaded result asserts only helper preparation, not runtime
    /// offline readiness. Reusing an existing matching server is permitted;
    /// reinjecting a loaded generation is not. This future must bound its remote
    /// operations and keep retained resources alive after uncertain delivery.
    fn prepare(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        admission: &mut dyn AdmissionCheck,
    ) -> impl Future<Output = Result<PreparedHelper<Self::Transport>, PreparationFailure>> + Send;
}

pub trait LogFactory: Send + 'static {
    type Log: LogSource + Send + 'static;
    /// Produce an independent reader/cursor for this exact process identity.
    fn create(&mut self, target: &ProcessIdentity) -> Result<Self::Log, BackendError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptOutcome {
    InProgress,
    SessionConnected,
    Failed(BackendError),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapAttempt {
    pub target: ProcessIdentity,
    pub helper_build_id: String,
    pub residency: PreparationResidency,
    pub outcome: AttemptOutcome,
}

pub struct SessionConnector<P, F> {
    preparer: P,
    logs: F,
    helper_build_id: String,
    policy: GatePolicy,
    last_attempt: Option<BootstrapAttempt>,
}
impl<P: HelperPreparer, F: LogFactory> SessionConnector<P, F> {
    /// No files, processes, logs or remote operations are accessed here.
    pub fn new(preparer: P, logs: F, helper_build_id: &str, policy: GatePolicy) -> Result<Self, BackendError> {
        if !hashes::is_sha256(helper_build_id) {
            return Err(BackendError::blocked(ErrorKind::Identity, "invalid expected helper source build ID"));
        }
        Ok(Self {
            preparer,
            logs,
            helper_build_id: helper_build_id.to_ascii_uppercase(),
            policy,
            last_attempt: None,
        })
    }
    pub fn last_attempt(&self) -> Option<&BootstrapAttempt> {
        self.last_attempt.as_ref()
    }

    async fn connect_once(&mut self, target: &ProcessIdentity, now: Timestamp) -> Result<Session<P::Transport>, BackendError> {
        let started = process_start(target)?;
        let log = self.logs.create(target)?;
        let mut admission = Admission {
            log,
            started,
            now,
            elapsed: Instant::now(),
            policy: self.policy,
        };
        admission.check()?;
        let (profile, _files) = capture_profile(target, &self.helper_build_id)?;
        admission.check()?;
        // Mark uncertainty before awaiting: even a caller that drops this future
        // cannot mistake a partially executed preparer for an unloaded helper.
        self.last_attempt.as_mut().unwrap().residency = PreparationResidency::MayBeLoaded;
        let prepared = match self.preparer.prepare(target, &profile, &mut admission).await {
            Ok(prepared) => prepared,
            Err(failure) => {
                self.last_attempt.as_mut().unwrap().residency = failure.residency;
                return Err(failure.into_error());
            }
        };
        self.last_attempt.as_mut().unwrap().residency = prepared.residency;
        admission.check()?;
        if prepared.target != *target || prepared.transport.target() != target {
            return Err(BackendError::blocked(
                ErrorKind::Identity,
                "prepared helper or transport belongs to another process identity",
            ));
        }
        if prepared.residency != PreparationResidency::Loaded || !prepared.helper_build_id.eq_ignore_ascii_case(profile.helper_build_id()) {
            return Err(BackendError::blocked(ErrorKind::Identity, "prepared helper source build or residency mismatch"));
        }
        let mut session = Session::new(target.clone(), prepared.transport, profile)?;
        session.enable();
        session.connect(admission.time()?).await?;
        Ok(session)
    }
}

impl<P: HelperPreparer, F: LogFactory> Connector for SessionConnector<P, F> {
    type Connection = Session<P::Transport>;
    fn reap_exited(&mut self) -> Vec<ProcessIdentity> {
        self.preparer.reap_exited()
    }
    async fn connect(&mut self, target: ProcessIdentity, now: Timestamp) -> Result<Self::Connection, BackendError> {
        self.last_attempt = Some(BootstrapAttempt {
            target: target.clone(),
            helper_build_id: self.helper_build_id.clone(),
            residency: PreparationResidency::NotLoaded,
            outcome: AttemptOutcome::InProgress,
        });
        let result = self.connect_once(&target, now).await;
        let attempt = self.last_attempt.as_mut().unwrap();
        match result {
            Ok(session) => {
                attempt.outcome = AttemptOutcome::SessionConnected;
                Ok(session)
            }
            Err(error) => {
                let mut error = PreparationFailure {
                    error,
                    residency: attempt.residency,
                }
                .into_error();
                // No automatic return to preparation after a handshake failure.
                // The retained server remains loaded until game exit.
                if attempt.residency != PreparationResidency::NotLoaded && error.kind != ErrorKind::Gate {
                    error.retry_when_waiting = false;
                }
                attempt.outcome = AttemptOutcome::Failed(error.clone());
                Err(error)
            }
        }
    }
}

struct Admission<L> {
    log: L,
    started: Timestamp,
    now: Timestamp,
    elapsed: Instant,
    policy: GatePolicy,
}
impl<L> Admission<L> {
    fn time(&self) -> Result<Timestamp, BackendError> {
        Timestamp::from_nanosecond(self.now.as_nanosecond() + self.elapsed.elapsed().as_nanos() as i128)
            .map_err(|_| BackendError::blocked(ErrorKind::Identity, "admission clock overflow"))
    }
}
impl<L: LogSource + Send> AdmissionCheck for Admission<L> {
    fn check(&mut self) -> Result<(), BackendError> {
        let decision = gate::evaluate(&self.log.read(self.started), self.started, self.time()?, self.policy);
        match decision.verdict {
            GateVerdict::Allow => Ok(()),
            GateVerdict::Wait | GateVerdict::Refuse => Err(BackendError {
                kind: ErrorKind::Gate,
                message: format!("offline admission before preparation: {:?} ({:?})", decision.readiness, decision.log_status),
                delivery: Some(Delivery::NotSent),
                retry_when_waiting: decision.verdict == GateVerdict::Wait,
            }),
        }
    }
}
fn process_start(target: &ProcessIdentity) -> Result<Timestamp, BackendError> {
    if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
        return Err(BackendError::blocked(ErrorKind::Identity, "invalid process identity"));
    }
    eg_core::time::from_filetime(target.created).map_err(|_| BackendError::blocked(ErrorKind::Identity, "invalid process creation time"))
}
fn capture_profile(target: &ProcessIdentity, helper_build_id: &str) -> Result<(BuildProfile, Option<std::sync::Arc<crate::profile::ConnectionFiles>>), BackendError> {
    let executable = std::fs::canonicalize(&target.executable).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
    let parent = executable
        .parent()
        .ok_or_else(|| BackendError::blocked(ErrorKind::Identity, "missing executable directory"))?;
    let mut data = executable
        .file_stem()
        .ok_or_else(|| BackendError::blocked(ErrorKind::Identity, "missing executable name"))?
        .to_os_string();
    data.push("_Data");
    let binary = parent.join("GameAssembly.dll");
    let metadata: PathBuf = parent.join(data).join("il2cpp_data/Metadata/global-metadata.dat");
    let (profile, files) = BuildProfile::capture_connection(&binary, &metadata, helper_build_id).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
    profile
        .validate_target(&target.executable)
        .map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
    Ok((profile, files))
}
