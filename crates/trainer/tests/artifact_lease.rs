#![cfg(windows)]
//! Owned Windows files and scripted transports only; no game/process access.
use eg_core::{
    game_log::{GameLogState, GameLogStatus, GameReadiness},
    hashes,
};
use eg_game_access::artifact::ReadLease;
use eg_trainer::session::{CaptureControl, CaptureObserver, CaptureOutcome, CaptureProgress, Cleanup};
use eg_trainer::{
    gate::LogSource,
    profile::BuildProfile,
    session::{Session, SessionError, SessionState},
};
use eg_trainer_wire::{
    catalog::{self, CatalogEntries, CatalogKind, CatalogPage, DefinitionName, DefinitionNameSource, ItemTypeDefinition},
    definitions::{self, ItemLimits, ValueRange},
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode, RuntimeStatus},
    transport::{Transport, TransportError},
};
use jiff::Timestamp;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    future::{Future, pending},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    task::{Context, Waker},
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
fn limits() -> ItemLimits {
    let r = ValueRange { minimum: 0, maximum: 255 };
    ItemLimits {
        forging_potential: r,
        legendary_potential: r,
        weavers_will: r,
        weavers_touch: r,
        level_requirement: r,
    }
}
fn page(kind: CatalogKind, cursor: u32, total: u32) -> CatalogPage {
    CatalogPage {
        locale: locale(),
        kind,
        cursor,
        total,
        entries: match kind {
            CatalogKind::Bases => CatalogEntries::Bases(
                (cursor..total.min(cursor + 16))
                    .map(|id| ItemTypeDefinition {
                        item_type: id as u8,
                        name: DefinitionName {
                            text: format!("Base {id}"),
                            source: DefinitionNameSource::StoredDisplay,
                        },
                        sub_types: vec![],
                    })
                    .collect(),
            ),
            CatalogKind::Affixes => CatalogEntries::Affixes(vec![]),
            CatalogKind::Uniques => CatalogEntries::Uniques(vec![]),
            CatalogKind::Rarities => CatalogEntries::Rarities(vec![]),
        },
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
        let dispatcher = DispatcherSpec {
            rva: 0x1000,
            bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
        };
        let mut pe = vec![0; 0x400];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        pe[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
        pe[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        pe[0x194..0x198].copy_from_slice(&0x1000u32.to_le_bytes());
        pe[0x198..0x19c].copy_from_slice(&0x200u32.to_le_bytes());
        pe[0x19c..0x1a0].copy_from_slice(&0x200u32.to_le_bytes());
        pe[0x1ac..0x1b0].copy_from_slice(&0x60000020u32.to_le_bytes());
        pe[0x200..0x210].copy_from_slice(&dispatcher.bytes);
        fs::write(&binary, pe).unwrap();
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
    fn profile(&self) -> BuildProfile {
        BuildProfile::capture(&self.binary, &self.metadata, HELPER).unwrap()
    }
    fn wire(&self, log: Logs) -> Wire {
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
            definitions::encode(&limits()).unwrap(),
            catalog::encode(&page(CatalogKind::Bases, 0, 17)).unwrap(),
            catalog::encode(&page(CatalogKind::Bases, 16, 17)).unwrap(),
            catalog::encode(&page(CatalogKind::Affixes, 0, 0)).unwrap(),
            catalog::encode(&page(CatalogKind::Uniques, 0, 0)).unwrap(),
            catalog::encode(&page(CatalogKind::Rarities, 0, 0)).unwrap(),
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
            _log: log,
        }
    }
}
#[derive(Clone)]
struct Logs(Arc<Mutex<GameReadiness>>);
impl Logs {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(GameReadiness::Offline)))
    }
}
impl LogSource for Logs {
    fn read(&mut self, _: Timestamp) -> GameLogState {
        let readiness = *self.0.lock().unwrap();
        GameLogState {
            status: GameLogStatus::Current,
            readiness,
            character: None,
            shell_state: Some("None".into()),
            offline_mode: Some(readiness == GameReadiness::Offline),
            problem: None,
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
    _log: Logs,
}
impl Transport for Wire {
    fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        let (wait, payload) = {
            let mut s = self.shared.lock().unwrap();
            s.requests.push(request.clone());
            if matches!(request, Request::ClearHooks { .. }) {
                return Ok(Response {
                    code: ResponseCode::Done,
                    payload: vec![0; 4],
                });
            }
            if s.assert_locks && !matches!(request, Request::Discover | Request::Status { .. }) {
                assert_shared(OpenOptions::new().write(true).open(&self.binary).unwrap_err());
                assert_shared(OpenOptions::new().write(true).open(&self.metadata).unwrap_err());
            }
            if s.refuse_runtime && matches!(request, Request::ReadCatalogPage { .. }) {
                return Ok(Response {
                    code: ResponseCode::OfflineRequired,
                    payload: b"offline character required".to_vec(),
                });
            }
            if s.replace_metadata && matches!(request, Request::ReadCatalogPage { .. }) {
                fs::write(&self.metadata, b"changed during fallback catalog reply").unwrap();
            }
            let wait = s.pending && matches!(request, Request::ReadCatalogPage { .. });
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
fn lease_blocks_writes_delete_and_rename_but_allows_compatible_reads_until_drop() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("owned.bin");
    fs::write(&path, b"owned immutable bytes").unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let lease = ReadLease::capture(&canonical, &hashes::of_bytes(&fs::read(&path).unwrap())).unwrap();
    assert_shared(OpenOptions::new().write(true).open(&path).unwrap_err());
    assert_shared(fs::remove_file(&path).unwrap_err());
    assert_shared(fs::rename(&path, temp.path().join("renamed.bin")).unwrap_err());
    assert!(File::open(&path).is_ok());
    drop(lease);
    fs::write(&path, b"released").unwrap();
    let renamed = temp.path().join("renamed.bin");
    fs::rename(path, &renamed).unwrap();
    fs::remove_file(renamed).unwrap();
}

#[test]
fn data_byte_counter_proves_metadata_only_path_checks_and_bounded_positional_reads() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("owned.bin");
    let bytes = vec![42u8; 1024 * 1024 + 17];
    fs::write(&path, &bytes).unwrap();
    let lease = ReadLease::capture(&fs::canonicalize(path).unwrap(), &hashes::of_bytes(&bytes)).unwrap();
    let initial = lease.bytes_read();
    assert_eq!(initial, bytes.len() as u64);
    for _ in 0..100 {
        lease.verify_path().unwrap();
    }
    assert_eq!(lease.bytes_read(), initial);
    let mut output = [0; 16];
    lease.read_exact_at(3, &mut output).unwrap();
    assert_eq!(output, [42; 16]);
    assert_eq!(lease.bytes_read(), initial + 16);
    assert!(lease.read_exact_at(u64::MAX, &mut output).is_err());
    assert!(lease.read_exact_at(lease.len() - 1, &mut output).is_err());
    assert_eq!(lease.bytes_read(), initial + 16);
}

#[test]
fn wrong_hash_or_replaced_artifacts_fail_and_failed_capture_drops_partial_locks() {
    let f = Fixture::new();
    let profile = f.profile();
    fs::write(&f.metadata, b"replacement").unwrap();
    assert!(profile.capture_lease(&f.dispatcher).is_err());
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
    assert!(ReadLease::capture(&fs::canonicalize(&f.binary).unwrap(), &"0".repeat(64)).is_err());
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
}

#[test]
fn dispatcher_and_executable_section_are_validated_against_the_retained_file() {
    let f = Fixture::new();
    let profile = f.profile();
    for rva in [0, 0x11f1, u64::MAX] {
        assert!(profile.capture_lease(&DispatcherSpec { rva, ..f.dispatcher }).is_err());
    }
    let mut wrong = f.dispatcher;
    wrong.bytes[0] ^= 1;
    assert!(profile.capture_lease(&wrong).is_err());
    let mut bytes = fs::read(&f.binary).unwrap();
    bytes[0x1af] = 0x40;
    fs::write(&f.binary, &bytes).unwrap();
    assert!(f.profile().capture_lease(&f.dispatcher).is_err());
}

#[test]
fn profile_lease_has_constant_validation_bytes_and_releases_both_files() {
    let f = Fixture::new();
    let profile = f.profile();
    let lease = profile.capture_lease(&f.dispatcher).unwrap();
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
    let profile = f.profile();
    let writer = OpenOptions::new().read(true).write(true).share_mode(7).open(&f.metadata).unwrap();
    let lease = profile.capture_lease(&f.dispatcher).unwrap();
    assert_eq!(lease.bytes_read(), None);
    drop(lease);
    drop(writer);
}

#[test]
fn session_reuses_one_lease_across_capture_polling_and_public_reads() {
    let f = Fixture::new();
    // Large enough to expose content I/O regressions independently of timing.
    OpenOptions::new().write(true).open(&f.binary).unwrap().set_len(8 * 1024 * 1024).unwrap();
    let (mut session, shared) = connected(&f);
    let initial = session.artifact_bytes_read().unwrap();
    assert!(initial > 8 * 1024 * 1024);
    let catalog = run(session.capture_catalog(now())).unwrap();
    assert_eq!(catalog.bases.len(), 17);
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
        state.responses.push_back(definitions::encode(&limits()).unwrap());
        state.responses.push_back(eg_trainer_wire::forge::encode(None).unwrap());
        drop(state);
        run(session.refresh(now())).unwrap();
        run(session.read_locale(now())).unwrap();
        run(session.read_item_limits(now())).unwrap();
        assert_eq!(run(session.read_forge(now())).unwrap(), None);
        assert_eq!(session.artifact_bytes_read(), Some(initial));
    }
    assert_shared(OpenOptions::new().write(true).open(&f.metadata).unwrap_err());
    run(session.disconnect());
    assert_eq!(session.artifact_bytes_read(), None);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
}

#[test]
fn capture_future_drop_retains_session_lease_and_preserves_connecting_barrier() {
    let f = Fixture::new();
    let log = Logs::new();
    let wire = f.wire(log.clone());
    let shared = wire.shared.clone();
    shared.lock().unwrap().pending = true;
    let mut session = Session::new(f.target.clone(), wire, f.profile()).unwrap();
    session.enable();
    run(session.connect(now())).unwrap();
    let mut future = Box::pin(session.capture_catalog(now()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(future.as_mut().poll(&mut context).is_pending());
    assert_shared(OpenOptions::new().write(true).open(&f.metadata).unwrap_err());
    drop(future);
    assert_shared(OpenOptions::new().write(true).open(&f.metadata).unwrap_err());
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.connect(now())), Err(SessionError::Busy)));
    let before = shared.lock().unwrap().requests.len();
    run(session.disconnect());
    assert_eq!(shared.lock().unwrap().requests.len(), before + 1);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
}

#[test]
fn per_page_runtime_refusal_discards_capture_releases_lease_and_stops_all_further_io() {
    let f = Fixture::new();
    let log = Logs::new();
    let wire = f.wire(log.clone());
    let shared = wire.shared.clone();
    shared.lock().unwrap().refuse_runtime = true;
    let mut session = Session::new(f.target.clone(), wire, f.profile()).unwrap();
    session.enable();
    run(session.connect(now())).unwrap();
    assert!(matches!(
        run(session.capture_catalog(now())),
        Err(SessionError::Helper {
            code: ResponseCode::OfflineRequired,
            ..
        })
    ));
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(shared.lock().unwrap().requests.len(), 5);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
    run(session.disconnect());
    assert_eq!(shared.lock().unwrap().requests.len(), 5);
}

#[test]
fn changed_artifact_before_capture_is_rejected_before_any_catalog_request() {
    for binary in [true, false] {
        let f = Fixture::new();
        let log = Logs::new();
        let wire = f.wire(log.clone());
        let shared = wire.shared.clone();
        let _writer = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(7)
            .open(if binary { &f.binary } else { &f.metadata })
            .unwrap();
        let mut session = Session::new(f.target.clone(), wire, f.profile()).unwrap();
        session.enable();
        run(session.connect(now())).unwrap();
        fs::write(if binary { &f.binary } else { &f.metadata }, b"changed before lease").unwrap();
        assert!(matches!(run(session.capture_catalog(now())), Err(SessionError::Identity(_))));
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
    run(session.capture_catalog(now())).unwrap();
    // Connecting and every catalog page checked the files; the content was read once.
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
        run(session.refresh(now())).unwrap();
        run(session.read_locale(now())).unwrap();
        assert_eq!(run(session.read_forge(now())).unwrap(), None);
    }
    assert_eq!(session.artifact_full_validations(), Some(after_connect), "unchanged files are not read again");
    run(session.disconnect());
    drop(writer);
}

#[test]
fn fallback_rehashes_each_page_and_rejects_an_inflight_artifact_change() {
    let f = Fixture::new();
    let log = Logs::new();
    let wire = f.wire(log.clone());
    let shared = wire.shared.clone();
    let writer = OpenOptions::new().read(true).write(true).share_mode(7).open(&f.metadata).unwrap();
    let mut session = Session::new(f.target.clone(), wire, f.profile()).unwrap();
    session.enable();
    run(session.connect(now())).unwrap();
    {
        let mut state = shared.lock().unwrap();
        state.assert_locks = false;
        state.replace_metadata = true;
    }
    assert!(matches!(run(session.capture_catalog(now())), Err(SessionError::Identity(_))));
    assert_eq!(shared.lock().unwrap().requests.len(), 5);
    assert_eq!(session.state(), SessionState::Disconnected);
    drop(writer);
    assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
}

struct Observe {
    events: Vec<CaptureProgress>,
    stop: bool,
    when: fn(&CaptureProgress) -> bool,
    calls: usize,
    stop_on_control: Option<usize>,
}
impl Observe {
    fn at(when: fn(&CaptureProgress) -> bool) -> Self {
        Self {
            events: vec![],
            stop: false,
            when,
            calls: 0,
            stop_on_control: None,
        }
    }
}
impl CaptureObserver for Observe {
    fn control(&mut self) -> CaptureControl {
        self.calls += 1;
        if self.stop || self.stop_on_control == Some(self.calls) {
            self.stop = true;
            CaptureControl::Stop
        } else {
            CaptureControl::Continue
        }
    }
    fn progress(&mut self, event: CaptureProgress) {
        self.stop |= (self.when)(&event);
        self.events.push(event);
    }
}
fn connected(f: &Fixture) -> (Session<Wire>, Arc<Mutex<WireState>>) {
    let log = Logs::new();
    let wire = f.wire(log.clone());
    let shared = wire.shared.clone();
    let mut session = Session::new(f.target.clone(), wire, f.profile()).unwrap();
    session.enable();
    run(session.connect(now())).unwrap();
    (session, shared)
}

#[test]
fn successful_progress_counts_actual_payload_bytes_without_publishing_partial_catalogs() {
    let f = Fixture::new();
    let (mut session, shared) = connected(&f);
    let mut observer = Observe::at(|_| false);
    let outcome = run(session.capture_catalog_observed(now(), &mut observer)).unwrap();
    assert!(matches!(outcome,CaptureOutcome::Complete(catalog) if catalog.bases.len()==17));
    let copied: Vec<_> = observer
        .events
        .iter()
        .filter_map(|event| match event {
            CaptureProgress::Page {
                kind,
                cursor,
                total,
                count,
                pages,
                bytes,
            } => Some((*kind, *cursor, *total, *count, *pages, *bytes)),
            _ => None,
        })
        .collect();
    let expected = [
        (CatalogKind::Bases, 0, 17, 16),
        (CatalogKind::Bases, 16, 17, 1),
        (CatalogKind::Affixes, 0, 0, 0),
        (CatalogKind::Uniques, 0, 0, 0),
        (CatalogKind::Rarities, 0, 0, 0),
    ];
    let mut bytes = 0;
    for (index, (kind, cursor, total, count)) in expected.into_iter().enumerate() {
        bytes += catalog::encode(&page(kind, cursor, total)).unwrap().len() as u64;
        assert_eq!(copied[index], (kind, cursor, total, count, index as u32 + 1, bytes));
    }
    assert_eq!(observer.events.last(), Some(&CaptureProgress::Complete { pages: 5, bytes }));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(shared.lock().unwrap().requests.len(), 10);
}

#[test]
fn stopping_after_a_page_keeps_session_lease_ready_and_allows_confirmed_cleanup() {
    let f = Fixture::new();
    let (mut session, shared) = connected(&f);
    let mut observer = Observe::at(|event| {
        matches!(
            event,
            CaptureProgress::Page {
                kind: CatalogKind::Bases,
                cursor: 0,
                ..
            }
        )
    });
    assert_eq!(run(session.capture_catalog_observed(now(), &mut observer)).unwrap(), CaptureOutcome::Stopped);
    assert_eq!(shared.lock().unwrap().requests.len(), 5);
    assert_eq!(session.state(), SessionState::Ready);
    assert!(session.runtime_status().is_some());
    assert_shared(OpenOptions::new().write(true).open(&f.metadata).unwrap_err());
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert!(OpenOptions::new().write(true).open(&f.metadata).is_ok());
    assert_eq!(shared.lock().unwrap().requests.len(), 6);
    assert!(!observer.events.iter().any(|event| matches!(event, CaptureProgress::Complete { .. })));
}

#[test]
fn deliberate_stops_before_first_rpc_after_limits_and_before_final_locale_preserve_cleanup() {
    for phase in 0..4 {
        let f = Fixture::new();
        let (mut session, shared) = connected(&f);
        let mut observer = Observe::at(match phase {
            0 => |_| false,
            1 => |event| matches!(event, CaptureProgress::LeaseVerified { .. }),
            2 => |event| matches!(event, CaptureProgress::LimitsRead),
            _ => |event| matches!(event, CaptureProgress::VerifyingLocale { .. }),
        });
        observer.stop = phase == 0;
        assert_eq!(run(session.capture_catalog_observed(now(), &mut observer)).unwrap(), CaptureOutcome::Stopped);
        assert_eq!(shared.lock().unwrap().requests.len(), [2, 2, 4, 9][phase]);
        assert_eq!(session.state(), SessionState::Ready);
        assert_shared(OpenOptions::new().write(true).open(&f.binary).unwrap_err());
        assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
        assert!(OpenOptions::new().write(true).open(&f.binary).is_ok());
    }
}

#[test]
fn stop_after_final_validation_discards_even_a_complete_local_catalog() {
    let f = Fixture::new();
    let (mut session, shared) = connected(&f);
    let mut observer = Observe::at(|event| matches!(event, CaptureProgress::Complete { .. }));
    assert_eq!(run(session.capture_catalog_observed(now(), &mut observer)).unwrap(), CaptureOutcome::Stopped);
    assert_eq!(shared.lock().unwrap().requests.len(), 10);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
}

#[test]
fn control_is_checked_again_after_artifact_validation_immediately_before_rpc() {
    let f = Fixture::new();
    let (mut session, shared) = connected(&f);
    let mut observer = Observe::at(|_| false);
    observer.stop_on_control = Some(6);
    assert_eq!(run(session.capture_catalog_observed(now(), &mut observer)).unwrap(), CaptureOutcome::Stopped);
    assert_eq!(
        shared.lock().unwrap().requests.len(),
        3,
        "locale finished; limits stopped after file validation before send"
    );
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
}

#[test]
fn observer_stop_does_not_turn_runtime_refusal_into_successful_cancellation() {
    let f = Fixture::new();
    let (mut session, shared) = connected(&f);
    shared.lock().unwrap().refuse_runtime = true;
    let mut observer = Observe::at(|event| matches!(event, CaptureProgress::Page { .. }));
    assert!(matches!(
        run(session.capture_catalog_observed(now(), &mut observer)),
        Err(SessionError::Helper {
            code: ResponseCode::OfflineRequired,
            ..
        })
    ));
    assert_eq!(session.state(), SessionState::Disconnected);
    let before = shared.lock().unwrap().requests.len();
    assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
    assert_eq!(shared.lock().unwrap().requests.len(), before);
}
