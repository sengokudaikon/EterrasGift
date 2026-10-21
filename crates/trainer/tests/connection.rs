//! Authored files and scripted transports only: no process is opened or injected.
use std::{
    collections::VecDeque,
    fs,
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness};
use eg_trainer::{
    backend::{BackendError, Connector, ErrorKind},
    connection::{AdmissionCheck, AttemptOutcome, HelperPreparer, LogFactory, PreparationFailure, PreparationResidency, PreparedHelper, SessionConnector},
    gate::{GatePolicy, LogSource},
    profile::BuildProfile,
    session::SessionState,
};
use eg_trainer_wire::{
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode, RuntimeStatus},
    transport::{Delivery, Transport, TransportError},
};
use jiff::Timestamp;
use tokio::sync::Notify;

const HELPER: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
fn now() -> Timestamp {
    "2026-10-19T10:00:00Z".parse().unwrap()
}
fn started() -> Timestamp {
    "2026-10-19T09:58:00Z".parse().unwrap()
}
fn run(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), future).await.unwrap() });
}
fn log(readiness: GameReadiness) -> GameLogState {
    GameLogState {
        status: GameLogStatus::Current,
        readiness,
        character: None,
        shell_state: Some("None".into()),
        offline_mode: Some(readiness == GameReadiness::Offline),
        problem: None,
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    target: ProcessIdentity,
    binary: PathBuf,
    metadata: PathBuf,
    dispatcher: DispatcherSpec,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("Owned Player.exe");
        fs::write(&executable, b"identity only; never executed").unwrap();
        let binary = temp.path().join("GameAssembly.dll");
        let metadata = temp.path().join("Owned Player_Data/il2cpp_data/Metadata/global-metadata.dat");
        fs::create_dir_all(metadata.parent().unwrap()).unwrap();
        fs::write(&metadata, b"owned metadata").unwrap();
        let dispatcher = DispatcherSpec {
            rva: 0x1000,
            bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
        };
        let mut image = vec![0; 0x400];
        image[..2].copy_from_slice(b"MZ");
        image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x80..0x84].copy_from_slice(b"PE\0\0");
        image[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        image[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
        image[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        image[0x194..0x198].copy_from_slice(&0x1000u32.to_le_bytes());
        image[0x198..0x19c].copy_from_slice(&0x200u32.to_le_bytes());
        image[0x19c..0x1a0].copy_from_slice(&0x200u32.to_le_bytes());
        image[0x1ac..0x1b0].copy_from_slice(&0x60000020u32.to_le_bytes());
        image[0x200..0x210].copy_from_slice(&dispatcher.bytes);
        fs::write(&binary, image).unwrap();
        let target = ProcessIdentity {
            pid: 42,
            created: (started().as_nanosecond() / 100 + 116_444_736_000_000_000) as u64,
            executable,
        };
        Self {
            _temp: temp,
            target,
            binary,
            metadata,
            dispatcher,
        }
    }
    fn discovery(&self) -> Response {
        Response {
            code: ResponseCode::Done,
            payload: Discovery {
                build_id: HELPER.into(),
                dispatcher: self.dispatcher,
            }
            .encode(self.target.pid)
            .unwrap(),
        }
    }
    fn shared(&self) -> Arc<Shared> {
        Arc::new(Shared {
            calls: Mutex::new(vec![]),
            log: Mutex::new(log(GameReadiness::Offline)),
            session_log: Mutex::new(None),
            readers: Mutex::new(vec![]),
            requests: Mutex::new(vec![]),
            responses: Mutex::new(
                [
                    self.discovery(),
                    Response {
                        code: ResponseCode::Done,
                        payload: RuntimeStatus {
                            online: false,
                            manager: 0x12340,
                            features_on: 0,
                        }
                        .encode()
                        .unwrap()
                        .to_vec(),
                    },
                ]
                .into(),
            ),
            mode: Mutex::new(Mode::Success),
            metadata: self.metadata.clone(),
            block: AtomicBool::new(false),
            started: Notify::new(),
            release: Notify::new(),
        })
    }
}
#[derive(Clone)]
enum Mode {
    Success,
    Failure(PreparationFailure),
    OnlineAtSecondStage,
    OnlineAtCompletion,
    WrongPreparedPid,
    WrongTransportPid,
    WrongCreated,
    WrongExecutable,
    WrongBuild,
    WrongResidency,
    ChangeMetadata,
}
struct Shared {
    calls: Mutex<Vec<&'static str>>,
    log: Mutex<GameLogState>,
    session_log: Mutex<Option<GameLogState>>,
    readers: Mutex<Vec<Vec<Timestamp>>>,
    requests: Mutex<Vec<Request>>,
    responses: Mutex<VecDeque<Response>>,
    mode: Mutex<Mode>,
    metadata: PathBuf,
    block: AtomicBool,
    started: Notify,
    release: Notify,
}
impl Shared {
    fn record(&self, value: &'static str) {
        self.calls.lock().unwrap().push(value);
    }
    fn count(&self, value: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|v| **v == value).count()
    }
}
struct Logs(Arc<Shared>);
struct Log {
    shared: Arc<Shared>,
    index: usize,
    override_state: Option<GameLogState>,
}
impl LogFactory for Logs {
    type Log = Log;
    fn create(&mut self, target: &ProcessIdentity) -> Result<Log, BackendError> {
        assert_eq!(target.pid, 42);
        self.0.record("log-created");
        let mut readers = self.0.readers.lock().unwrap();
        let index = readers.len();
        readers.push(vec![]);
        Ok(Log {
            shared: self.0.clone(),
            index,
            override_state: if index > 0 { self.0.session_log.lock().unwrap().clone() } else { None },
        })
    }
}
impl LogSource for Log {
    fn read(&mut self, start: Timestamp) -> GameLogState {
        self.shared.record("gate");
        self.shared.readers.lock().unwrap()[self.index].push(start);
        self.override_state.clone().unwrap_or_else(|| self.shared.log.lock().unwrap().clone())
    }
}
struct Wire {
    target: ProcessIdentity,
    shared: Arc<Shared>,
}
impl Transport for Wire {
    fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        self.shared.record("pipe");
        self.shared.requests.lock().unwrap().push(request.clone());
        Ok(self.shared.responses.lock().unwrap().pop_front().expect("unexpected pipe request"))
    }
}
struct Prepare(Arc<Shared>);
impl HelperPreparer for Prepare {
    type Transport = Wire;
    async fn prepare(&mut self, target: &ProcessIdentity, profile: &BuildProfile, admission: &mut dyn AdmissionCheck) -> Result<PreparedHelper<Wire>, PreparationFailure> {
        self.0.record("prepare");
        assert!(profile.helper_build_id().eq_ignore_ascii_case(HELPER));
        assert!(eg_core::hashes::is_sha256(profile.binary_hash()));
        admission.check().map_err(|error| PreparationFailure {
            error,
            residency: PreparationResidency::NotLoaded,
        })?;
        self.0.record("query");
        let mode = self.0.mode.lock().unwrap().clone();
        if matches!(mode, Mode::OnlineAtSecondStage) {
            *self.0.log.lock().unwrap() = log(GameReadiness::Online);
        }
        admission.check().map_err(|error| PreparationFailure {
            error,
            residency: PreparationResidency::NotLoaded,
        })?;
        if let Mode::Failure(error) = mode {
            return Err(error);
        }
        self.0.record("load");
        if self.0.block.swap(false, Ordering::SeqCst) {
            self.0.started.notify_one();
            self.0.release.notified().await;
        }
        let mut result = PreparedHelper {
            target: target.clone(),
            helper_build_id: HELPER.to_ascii_uppercase(),
            residency: PreparationResidency::Loaded,
            transport: Wire {
                target: target.clone(),
                shared: self.0.clone(),
            },
        };
        match mode {
            Mode::OnlineAtCompletion => *self.0.log.lock().unwrap() = log(GameReadiness::Online),
            Mode::WrongPreparedPid => result.target.pid += 1,
            Mode::WrongTransportPid => result.transport.target.pid += 1,
            Mode::WrongCreated => result.target.created += 1,
            Mode::WrongExecutable => result.target.executable = target.executable.with_file_name("Other.exe"),
            Mode::WrongBuild => result.helper_build_id = "0".repeat(64),
            Mode::WrongResidency => result.residency = PreparationResidency::NotLoaded,
            Mode::ChangeMetadata => fs::write(&self.0.metadata, b"changed after profile capture").unwrap(),
            _ => {}
        }
        Ok(result)
    }
}
fn connector(shared: &Arc<Shared>, policy: GatePolicy) -> SessionConnector<Prepare, Logs> {
    SessionConnector::new(Prepare(shared.clone()), Logs(shared.clone()), HELPER, policy).unwrap()
}

#[test]
fn construction_has_no_file_log_or_preparation_activity() {
    let fixture = Fixture::new();
    let shared = fixture.shared();
    let _ = connector(&shared, GatePolicy::Release);
    assert!(shared.calls.lock().unwrap().is_empty());
    assert!(SessionConnector::new(Prepare(shared.clone()), Logs(shared), "not-a-source-id", GatePolicy::Release).is_err());
}

#[test]
fn one_process_bound_admission_reader_and_case_insensitive_helper_id_connect() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let mut connector = connector(&shared, GatePolicy::Release);
        let session = connector.connect(fixture.target.clone(), now()).await.unwrap();
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(session.target(), &fixture.target);
        assert_eq!(*shared.requests.lock().unwrap(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }]);
        let readers = shared.readers.lock().unwrap();
        assert_eq!(readers.len(), 1);
        assert!(readers[0].len() >= 5);
        assert!(readers.iter().flatten().all(|time| *time == started()));
        let attempt = connector.last_attempt().unwrap();
        assert_eq!(attempt.residency, PreparationResidency::Loaded);
        assert_eq!(attempt.outcome, AttemptOutcome::SessionConnected);
        assert_eq!(shared.count("prepare"), 1);
        drop(session);
        assert_eq!(shared.count("pipe"), 2);
    });
}

#[test]
fn release_gate_denial_prevents_profile_io_preparation_and_transport() {
    run(async {
        for state in [log(GameReadiness::Online), log(GameReadiness::NoCharacter), GameLogState::missing(), GameLogState::stale()] {
            let fixture = Fixture::new();
            let shared = fixture.shared();
            *shared.log.lock().unwrap() = state;
            fs::remove_file(&fixture.binary).unwrap();
            let mut connector = connector(&shared, GatePolicy::Release);
            let error = connector.connect(fixture.target, now()).await.err().unwrap();
            assert_eq!(error.kind, ErrorKind::Gate);
            assert_eq!(error.delivery, Some(Delivery::NotSent));
            assert_eq!(shared.count("prepare"), 0);
            assert_eq!(shared.count("pipe"), 0);
            assert_eq!(connector.last_attempt().unwrap().residency, PreparationResidency::NotLoaded);
        }
    });
}

#[test]
fn malformed_target_or_missing_profile_fails_before_preparer() {
    run(async {
        for fault in 0..5 {
            let fixture = Fixture::new();
            let shared = fixture.shared();
            let mut target = fixture.target.clone();
            match fault {
                0 => target.pid = 0,
                1 => target.created = 0,
                2 => target.executable = PathBuf::from("relative.exe"),
                3 => fs::remove_file(&fixture.metadata).unwrap(),
                _ => target.executable = target.executable.with_file_name("Absent.exe"),
            };
            let mut connector = connector(&shared, GatePolicy::Release);
            assert_eq!(connector.connect(target, now()).await.err().unwrap().kind, ErrorKind::Identity);
            assert_eq!(shared.count("prepare"), 0);
            assert_eq!(shared.count("pipe"), 0);
        }
    });
}

#[test]
fn admission_callback_stops_second_preparation_stage_before_load() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        *shared.mode.lock().unwrap() = Mode::OnlineAtSecondStage;
        let mut connector = connector(&shared, GatePolicy::Release);
        let error = connector.connect(fixture.target, now()).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Gate);
        assert_eq!(error.delivery, Some(Delivery::NotSent));
        assert_eq!(shared.count("query"), 1);
        assert_eq!(shared.count("load"), 0);
        assert_eq!(shared.count("pipe"), 0);
    });
}

#[test]
fn online_loss_after_preparer_completes_never_sends_discover() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        *shared.mode.lock().unwrap() = Mode::OnlineAtCompletion;
        let mut connector = connector(&shared, GatePolicy::Release);
        let error = connector.connect(fixture.target, now()).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Gate);
        assert_eq!(error.delivery, Some(Delivery::NotSent));
        assert!(error.retry_when_waiting);
        assert_eq!(connector.last_attempt().unwrap().residency, PreparationResidency::Loaded);
        assert_eq!(shared.count("pipe"), 0);
    });
}

#[test]
fn session_does_not_create_a_second_admission_reader() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        *shared.session_log.lock().unwrap() = Some(log(GameReadiness::Online));
        let mut connector = connector(&shared, GatePolicy::Release);
        let session = connector.connect(fixture.target, now()).await.unwrap();
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(shared.count("log-created"), 1);
        assert_eq!(shared.count("pipe"), 2);
    });
}

#[test]
fn prepared_identity_transport_source_and_residency_are_bound_before_any_pipe_io() {
    run(async {
        for mode in [
            Mode::WrongPreparedPid,
            Mode::WrongTransportPid,
            Mode::WrongCreated,
            Mode::WrongExecutable,
            Mode::WrongBuild,
            Mode::WrongResidency,
        ] {
            let fixture = Fixture::new();
            let shared = fixture.shared();
            *shared.mode.lock().unwrap() = mode;
            let mut connector = connector(&shared, GatePolicy::Release);
            let error = connector.connect(fixture.target, now()).await.err().unwrap();
            assert_eq!(error.kind, ErrorKind::Identity);
            assert_eq!(shared.count("pipe"), 0);
            assert_eq!(shared.count("prepare"), 1);
        }
    });
}

#[test]
fn artifact_change_after_preparation_is_rejected_before_status_and_not_replayed() {
    run(async {
        let fixture = Fixture::new();
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(7);
        }
        let _writer = options.open(&fixture.metadata).unwrap();
        let shared = fixture.shared();
        *shared.mode.lock().unwrap() = Mode::ChangeMetadata;
        let mut connector = connector(&shared, GatePolicy::Release);
        let error = connector.connect(fixture.target, now()).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Identity);
        assert_eq!(error.delivery, Some(Delivery::MayHaveRun));
        assert!(!error.retry_when_waiting);
        assert_eq!(*shared.requests.lock().unwrap(), [Request::Discover]);
        assert_eq!(shared.count("prepare"), 1);
    });
}

#[test]
fn loaded_and_ambiguous_preparer_failures_never_downgrade_to_not_sent() {
    run(async {
        for residency in [PreparationResidency::NotLoaded, PreparationResidency::MayBeLoaded, PreparationResidency::Loaded] {
            let fixture = Fixture::new();
            let shared = fixture.shared();
            let mut error = BackendError::waiting(ErrorKind::Transport, "scripted preparation failure");
            error.delivery = Some(Delivery::NotSent);
            *shared.mode.lock().unwrap() = Mode::Failure(PreparationFailure { error, residency });
            let mut connector = connector(&shared, GatePolicy::Release);
            let error = connector.connect(fixture.target, now()).await.err().unwrap();
            assert_eq!(
                error.delivery,
                Some(if residency == PreparationResidency::NotLoaded {
                    Delivery::NotSent
                } else {
                    Delivery::MayHaveRun
                })
            );
            assert_eq!(error.retry_when_waiting, residency == PreparationResidency::NotLoaded);
            assert_eq!(connector.last_attempt().unwrap().residency, residency);
            assert_eq!(shared.count("prepare"), 1);
            assert_eq!(shared.count("pipe"), 0);
        }
    });
}

#[test]
fn preparation_and_handshake_errors_preserve_restart_required_and_never_retry() {
    run(async {
        for from_helper in [false, true] {
            let fixture = Fixture::new();
            let shared = fixture.shared();
            if from_helper {
                *shared.responses.lock().unwrap() = [Response {
                    code: ResponseCode::RestartRequired,
                    payload: b"restart game".to_vec(),
                }]
                .into();
            } else {
                *shared.mode.lock().unwrap() = Mode::Failure(PreparationFailure {
                    error: BackendError::blocked(ErrorKind::RestartRequired, "an older retained helper exists"),
                    residency: PreparationResidency::Loaded,
                });
            }
            let mut connector = connector(&shared, GatePolicy::Release);
            let error = connector.connect(fixture.target, now()).await.err().unwrap();
            assert_eq!(error.kind, ErrorKind::RestartRequired);
            assert_eq!(error.delivery, Some(Delivery::MayHaveRun));
            assert!(!error.retry_when_waiting);
            assert_eq!(shared.count("prepare"), 1);
            assert_eq!(shared.count("pipe"), usize::from(from_helper));
        }
    });
}

#[test]
fn preparation_wait_rechecks_log_after_await_and_records_in_progress_uncertainty() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        shared.block.store(true, Ordering::SeqCst);
        let mut connector = connector(&shared, GatePolicy::Release);
        let mut future = Box::pin(connector.connect(fixture.target, now()));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        *shared.log.lock().unwrap() = log(GameReadiness::Online);
        shared.release.notify_one();
        let error = future.await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Gate);
        assert_eq!(shared.count("pipe"), 0);
        assert_eq!(connector.last_attempt().unwrap().residency, PreparationResidency::Loaded);
    });
}

#[test]
fn missing_log_never_probes_the_runtime() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        *shared.log.lock().unwrap() = GameLogState::missing();
        let mut connector = connector(&shared, GatePolicy::Release);
        let error = connector.connect(fixture.target, now()).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Gate);
        assert_eq!(shared.count("prepare"), 0);
        assert_eq!(shared.count("pipe"), 0);
    });
}

#[test]
fn helper_not_ready_after_preparation_is_not_an_automatic_bootstrap_retry() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        *shared.responses.lock().unwrap() = [Response {
            code: ResponseCode::NotReady,
            payload: b"runtime not initialized".to_vec(),
        }]
        .into();
        let mut connector = connector(&shared, GatePolicy::Release);
        let error = connector.connect(fixture.target, now()).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::NotReady);
        assert_eq!(error.delivery, Some(Delivery::MayHaveRun));
        assert!(!error.retry_when_waiting);
        assert_eq!(shared.count("prepare"), 1);
        assert_eq!(shared.count("pipe"), 1);
        assert_eq!(connector.last_attempt().unwrap().residency, PreparationResidency::Loaded);
    });
}

#[test]
fn dropped_preparation_future_retains_in_progress_uncertainty() {
    run(async {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        shared.block.store(true, Ordering::SeqCst);
        let mut connector = connector(&shared, GatePolicy::Release);
        let mut future = Box::pin(connector.connect(fixture.target, now()));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        drop(future);
        let attempt = connector.last_attempt().unwrap();
        assert_eq!(attempt.residency, PreparationResidency::MayBeLoaded);
        assert_eq!(attempt.outcome, AttemptOutcome::InProgress);
        assert_eq!(shared.count("prepare"), 1);
        assert_eq!(shared.count("pipe"), 0);
    });
}
