#![cfg(windows)]
//! Owned Windows files and scripted transports only; no game/process access.
#[path = "pe_support/mod.rs"]
mod pe_support;
use eg_trainer::{
    connect::build::BuildIdentity,
    session::{Session, SessionError, SessionState},
};
use eg_trainer_wire::{
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode, RuntimeStatus},
    transport::{Transport, TransportError},
};
use file_ops::lease::ReadLease;
use jiff::Timestamp;
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    future::{Future, pending},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const HELPER: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
fn now() -> Timestamp {
    "2026-10-19T10:00:00Z".parse().unwrap()
}
fn run<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}
fn assert_shared(error: std::io::Error) {
    assert_eq!(error.raw_os_error(), Some(32), "expected sharing violation: {error}");
}
fn locale() -> GameLocale {
    GameLocale {
        language: "en".into(),
        game_version: "owned-lease".into(),
    }
}
struct Fixture {
    _temp: tempfile::TempDir,
    binary: PathBuf,
    metadata: PathBuf,
    target: ProcessIdentity,
    dispatcher: DispatcherSpec,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("GameAssembly.dll");
        let metadata = temp.path().join("Owned_Data/il2cpp_data/Metadata/global-metadata.dat");
        fs::create_dir_all(metadata.parent().unwrap()).unwrap();
        fs::write(&metadata, b"owned lease metadata").unwrap();
        let exe = temp.path().join("Owned.exe");
        fs::write(&exe, b"owned identity only").unwrap();
        let dispatcher = pe_support::dispatcher();
        fs::write(&binary, pe_support::image(&dispatcher)).unwrap();
        let target = ProcessIdentity {
            pid: 42,
            created: ((now().as_nanosecond() - 120_000_000_000) / 100 + 116_444_736_000_000_000) as u64,
            executable: exe,
        };
        Self {
            _temp: temp,
            binary,
            metadata,
            target,
            dispatcher,
        }
    }
    fn build(&self) -> BuildIdentity {
        BuildIdentity::capture(&self.binary, &self.metadata, HELPER).unwrap()
    }
    fn wire(&self) -> Wire {
        let discovery = Discovery {
            build_id: HELPER.into(),
            dispatcher: self.dispatcher,
        };
        let responses = vec![
            discovery.encode(42).unwrap(),
            RuntimeStatus {
                online: false,
                manager: 0x12340,
                features_on: 0,
            }
            .encode()
            .unwrap()
            .to_vec(),
            locale::encode(&locale()).unwrap(),
        ];
        Wire {
            target: self.target.clone(),
            shared: Arc::new(Mutex::new(WireState {
                responses: responses.into(),
                requests: vec![],
                pending: false,
                refuse_runtime: false,
                assert_locks: true,
                replace_metadata: false,
            })),
            binary: self.binary.clone(),
            metadata: self.metadata.clone(),
        }
    }
}
struct WireState {
    responses: VecDeque<Vec<u8>>,
    requests: Vec<Request>,
    pending: bool,
    refuse_runtime: bool,
    assert_locks: bool,
    replace_metadata: bool,
}
#[derive(Clone)]
struct Wire {
    target: ProcessIdentity,
    shared: Arc<Mutex<WireState>>,
    binary: PathBuf,
    metadata: PathBuf,
}
impl Transport for Wire {
    fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    async fn unload(&mut self) -> Result<(), TransportError> {
        self.send(&Request::Unload).await?;
        Ok(()) // Authored transport models native departure after the control ACK.
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        let (wait, payload) = {
            let mut s = self.shared.lock().unwrap();
            s.requests.push(request.clone());
            if matches!(request, Request::Unload) {
                return Ok(Response {
                    code: ResponseCode::Done,
                    payload: vec![0; 4],
                });
            }
            if s.assert_locks && !matches!(request, Request::Discover | Request::Status { .. }) {
                assert_shared(OpenOptions::new().write(true).open(&self.binary).unwrap_err());
                assert_shared(OpenOptions::new().write(true).open(&self.metadata).unwrap_err());
            }
            if s.refuse_runtime && matches!(request, Request::ReadLocale { .. }) {
                return Ok(Response {
                    code: ResponseCode::OfflineRequired,
                    payload: b"offline character required".to_vec(),
                });
            }
            if s.replace_metadata && matches!(request, Request::ReadLocale { .. }) {
                fs::write(&self.metadata, b"changed during fallback reply").unwrap();
            }
            let wait = s.pending && matches!(request, Request::ReadLocale { .. });
            (wait, if wait { vec![] } else { s.responses.pop_front().expect("unexpected request") })
        };
        if wait {
            pending::<()>().await;
        }
        Ok(Response {
            code: ResponseCode::Done,
            payload,
        })
    }
}

#[test]
fn wrong_hash_or_replaced_artifacts_fail_and_failed_capture_drops_partial_locks() {
    let f = Fixture::new();
    let build = f.build();
    fs::write(&f.metadata, b"replacement").unwrap();
    assert!(build.capture_lease(&f.dispatcher).is_err());
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
    assert!(ReadLease::capture(&fs::canonicalize(&f.binary).unwrap(), &"0".repeat(64)).is_err());
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
}

#[test]
fn dispatcher_and_executable_section_are_validated_against_the_retained_file() {
    let f = Fixture::new();
    let build = f.build();
    for rva in [0, 0x11f1, u64::MAX] {
        assert!(build.capture_lease(&DispatcherSpec { rva, ..f.dispatcher }).is_err());
    }
    let mut wrong = f.dispatcher;
    wrong.bytes[0] ^= 1;
    assert!(build.capture_lease(&wrong).is_err());
    let mut bytes = fs::read(&f.binary).unwrap();
    bytes[0x1af] = 0x40;
    fs::write(&f.binary, &bytes).unwrap();
    assert!(f.build().capture_lease(&f.dispatcher).is_err());
}

#[test]
fn profile_lease_has_constant_validation_bytes_and_releases_both_files() {
    let f = Fixture::new();
    let build = f.build();
    let lease = build.capture_lease(&f.dispatcher).unwrap();
    let content = fs::metadata(&f.binary).unwrap().len() + fs::metadata(&f.metadata).unwrap().len();
    assert!((content..=content + 4096).contains(&lease.bytes_read().unwrap()));
    assert_shared(OpenOptions::new().write(true).open(&f.metadata).unwrap_err());
    drop(lease);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
}

#[test]
fn existing_writer_selects_explicit_full_hash_fallback_instead_of_claiming_a_lease() {
    let f = Fixture::new();
    let build = f.build();
    let writer = OpenOptions::new().read(true).write(true).share_mode(7).open(&f.metadata).unwrap();
    let lease = build.capture_lease(&f.dispatcher).unwrap();
    assert_eq!(lease.bytes_read(), None);
    drop(lease);
    drop(writer);
}

#[test]
fn session_reuses_one_lease_across_polling_and_public_reads() {
    let f = Fixture::new();
    // Large enough to expose content I/O regressions independently of timing.
    OpenOptions::new().write(true).open(&f.binary).unwrap().set_len(8 * 1024 * 1024).unwrap();
    let (mut session, shared) = connected(&f);
    let initial = session.artifact_bytes_read().unwrap();
    assert!(initial > 8 * 1024 * 1024);
    assert_eq!(run(session.read_locale()).unwrap(), locale());
    for _ in 0..12 {
        let mut state = shared.lock().unwrap();
        state.responses.push_back(
            RuntimeStatus {
                online: false,
                manager: 0x12340,
                features_on: 0,
            }
            .encode()
            .unwrap()
            .to_vec(),
        );
        state.responses.push_back(locale::encode(&locale()).unwrap());
        state.responses.push_back(eg_trainer_wire::forge::encode(None).unwrap());
        drop(state);
        run(session.refresh()).unwrap();
        run(session.read_locale()).unwrap();
        assert_eq!(run(session.read_forge()).unwrap(), None);
        assert_eq!(session.artifact_bytes_read(), Some(initial));
    }
    assert_shared(OpenOptions::new().write(true).open(&f.metadata).unwrap_err());
    run(session.disconnect());
    assert_eq!(session.artifact_bytes_read(), None);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
}

#[test]
fn runtime_refusal_releases_lease_and_stops_all_further_io() {
    let f = Fixture::new();
    let wire = f.wire();
    let shared = wire.shared.clone();
    shared.lock().unwrap().refuse_runtime = true;
    let mut session = Session::new(f.target.clone(), wire, f.build()).unwrap();
    session.enable();
    run(session.connect()).unwrap();
    assert!(matches!(
        run(session.read_locale()),
        Err(SessionError::Helper {
            code: ResponseCode::OfflineRequired,
            ..
        })
    ));
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(shared.lock().unwrap().requests.len(), 3);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
    run(session.disconnect());
    assert_eq!(shared.lock().unwrap().requests.len(), 4);
    assert_eq!(shared.lock().unwrap().requests.last(), Some(&Request::Unload));
}

#[test]
fn changed_artifact_before_a_read_is_rejected_before_its_request() {
    for binary in [true, false] {
        let f = Fixture::new();
        let wire = f.wire();
        let shared = wire.shared.clone();
        let _writer = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(7)
            .open(if binary { &f.binary } else { &f.metadata })
            .unwrap();
        let mut session = Session::new(f.target.clone(), wire, f.build()).unwrap();
        session.enable();
        run(session.connect()).unwrap();
        fs::write(if binary { &f.binary } else { &f.metadata }, b"changed before lease").unwrap();
        assert!(matches!(run(session.read_locale()), Err(SessionError::Identity(_))));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(shared.lock().unwrap().requests.len(), 2);
    }
}

#[test]
fn fallback_hashes_unchanged_artifacts_once_rather_than_on_every_read() {
    let f = Fixture::new();
    let writer = OpenOptions::new().read(true).write(true).share_mode(7).open(&f.metadata).unwrap();
    let (mut session, shared) = connected(&f);
    // No lease holds the files against writes: the fallback is what this checks.
    shared.lock().unwrap().assert_locks = false;
    assert_eq!(session.artifact_bytes_read(), None, "another writer selects the fallback");
    assert_eq!(run(session.read_locale()).unwrap(), locale());
    // Connecting and the read checked the files; the content was read once.
    let after_connect = session.artifact_full_validations().unwrap();
    assert_eq!(after_connect, 1);
    for _ in 0..12 {
        let mut state = shared.lock().unwrap();
        state.responses.push_back(
            RuntimeStatus {
                online: false,
                manager: 0x12340,
                features_on: 0,
            }
            .encode()
            .unwrap()
            .to_vec(),
        );
        state.responses.push_back(locale::encode(&locale()).unwrap());
        state.responses.push_back(eg_trainer_wire::forge::encode(None).unwrap());
        drop(state);
        run(session.refresh()).unwrap();
        run(session.read_locale()).unwrap();
        assert_eq!(run(session.read_forge()).unwrap(), None);
    }
    assert_eq!(session.artifact_full_validations(), Some(after_connect), "unchanged files are not read again");
    run(session.disconnect());
    drop(writer);
}

#[test]
fn fallback_rehashes_each_read_and_rejects_an_inflight_artifact_change() {
    let f = Fixture::new();
    let wire = f.wire();
    let shared = wire.shared.clone();
    let writer = OpenOptions::new().read(true).write(true).share_mode(7).open(&f.metadata).unwrap();
    let mut session = Session::new(f.target.clone(), wire, f.build()).unwrap();
    session.enable();
    run(session.connect()).unwrap();
    {
        let mut state = shared.lock().unwrap();
        state.assert_locks = false;
        state.replace_metadata = true;
    }
    assert!(matches!(run(session.read_locale()), Err(SessionError::Identity(_))));
    assert_eq!(shared.lock().unwrap().requests.len(), 3);
    assert_eq!(session.state(), SessionState::Disconnected);
    drop(writer);
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
}

fn connected(f: &Fixture) -> (Session<Wire>, Arc<Mutex<WireState>>) {
    let wire = f.wire();
    let shared = wire.shared.clone();
    let mut session = Session::new(f.target.clone(), wire, f.build()).unwrap();
    session.enable();
    run(session.connect()).unwrap();
    (session, shared)
}
