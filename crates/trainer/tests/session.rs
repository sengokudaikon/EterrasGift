//! Session admission uses owned artifacts and scripted services; no game is opened.
#[path = "session/descriptions.rs"]
mod descriptions;
#[path = "session/fields.rs"]
mod fields;
#[path = "session/rate_write.rs"]
mod rate_writes;
#[path = "session/write.rs"]
mod writes;
use eg_core::game_log::{GameLogEvent, GameLogState, GameLogStatus, GameReadiness, LoggedCharacter};
use eg_game_access::{
    catalog::{self, CatalogEntries, CatalogKind, CatalogPage, DefinitionName, DefinitionNameSource, ItemTypeDefinition},
    definitions::{self, ItemLimits, ValueRange},
    forge::{self, AffixSnapshot, ForgeSnapshot, ItemStatus},
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, ProtocolError, REQUIRED_KEYS, Request, Response, ResponseCode, RuntimeStatus},
    transport::{Delivery, Transport, TransportError, TransportFailure},
};
use eg_trainer::{
    gate::{GatePolicy, GateVerdict, LogSource},
    profile::BuildProfile,
    session::{Cleanup, Session, SessionError, SessionState},
};
use jiff::{SignedDuration, Timestamp};
use std::{
    collections::VecDeque,
    fs,
    future::{Future, pending},
    path::PathBuf,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

const HELPER_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn started() -> Timestamp {
    "2026-10-18T10:00:00Z".parse().unwrap()
}

fn catalog_page(kind: CatalogKind, cursor: u32, total: u32) -> CatalogPage {
    CatalogPage {
        locale: GameLocale {
            language: "en-fixture".into(),
            game_version: "fixture-version".into(),
        },
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

#[test]
fn catalog_capture_pins_identity_and_publishes_only_after_all_pages_and_final_locale() {
    let fixture = Fixture::new();
    let first = catalog_page(CatalogKind::Bases, 0, 17);
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            done(locale::encode(&first.locale).unwrap()),
            done(definitions::encode(&item_limits()).unwrap()),
            done(catalog::encode(&first).unwrap()),
            done(catalog::encode(&catalog_page(CatalogKind::Bases, 16, 17)).unwrap()),
            done(catalog::encode(&catalog_page(CatalogKind::Affixes, 0, 0)).unwrap()),
            done(catalog::encode(&catalog_page(CatalogKind::Uniques, 0, 0)).unwrap()),
            done(catalog::encode(&catalog_page(CatalogKind::Rarities, 0, 0)).unwrap()),
            done(locale::encode(&first.locale).unwrap()),
        ],
    );
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let catalog = run(session.capture_catalog(now())).unwrap();
    assert_eq!(catalog.bases.len(), 17);
    assert_eq!(catalog.identity.locale, first.locale);
    assert_eq!(catalog.identity.binary_hash, fixture.profile().binary_hash());
    assert_eq!(catalog.limits, item_limits());
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(
        wire.requests()[4],
        Request::ReadCatalogPage {
            dispatcher: fixture.dispatcher,
            kind: CatalogKind::Bases,
            cursor: 0
        }
    );
    assert_eq!(
        wire.requests()[5],
        Request::ReadCatalogPage {
            dispatcher: fixture.dispatcher,
            kind: CatalogKind::Bases,
            cursor: 16
        }
    );
    assert_eq!(wire.requests().len(), 10);
}

#[test]
fn catalog_page_binds_response_to_query_and_never_replays_or_recovers_implicitly() {
    for fault in 0..4 {
        let fixture = Fixture::new();
        let log = FakeLog::new([offline()]);
        let mut page = catalog_page(CatalogKind::Bases, 0, 17);
        match fault {
            0 => page = catalog_page(CatalogKind::Bases, 16, 17),
            1 => page = catalog_page(CatalogKind::Affixes, 0, 0),
            _ => {}
        }
        let reply = if fault == 2 {
            done(vec![0xff])
        } else if fault == 3 {
            Step::Reply(Err(TransportError {
                delivery: Delivery::MayHaveRun,
                failure: TransportFailure::Timeout,
            }))
        } else {
            done(catalog::encode(&page).unwrap())
        };
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), reply]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(run(session.read_catalog_page(now(), CatalogKind::Bases, 0)).is_err());
        assert_eq!(session.state(), SessionState::Blocked);
        assert!(session.runtime_status().is_none());
        assert!(run(session.capture_catalog(now())).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
}

#[test]
fn catalog_capture_rejects_midpage_drift_and_discards_complete_capture_after_language_change() {
    for fault in 0..4 {
        let fixture = Fixture::new();
        let first = catalog_page(CatalogKind::Bases, 0, 17);
        let mut second = catalog_page(CatalogKind::Bases, 16, 17);
        match fault {
            0 => second.total = 18,
            1 => second.locale.language = "sr-Latn-RS".into(),
            2 => {
                if let CatalogEntries::Bases(entries) = &mut second.entries {
                    entries[0].item_type = 0;
                }
            }
            _ => {}
        }
        if fault == 0
            && let CatalogEntries::Bases(entries) = &mut second.entries
        {
            entries.push(ItemTypeDefinition {
                item_type: 17,
                name: DefinitionName {
                    text: "new base".into(),
                    source: DefinitionNameSource::StoredDisplay,
                },
                sub_types: vec![],
            });
        }
        let mut steps = vec![
            fixture.discover(),
            status(false, 0x1234, 0),
            done(locale::encode(&first.locale).unwrap()),
            done(definitions::encode(&item_limits()).unwrap()),
            done(catalog::encode(&first).unwrap()),
            done(catalog::encode(&second).unwrap()),
        ];
        if fault == 3 {
            for kind in [CatalogKind::Affixes, CatalogKind::Uniques, CatalogKind::Rarities] {
                steps.push(done(catalog::encode(&catalog_page(kind, 0, 0)).unwrap()));
            }
            let mut changed = first.locale.clone();
            changed.language = "sr-Latn-RS".into();
            steps.push(done(locale::encode(&changed).unwrap()));
        }
        let wire = FakeTransport::new(fixture.target.clone(), steps);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(matches!(run(session.capture_catalog(now())), Err(SessionError::Protocol(_))));
        assert_eq!(session.state(), SessionState::Blocked);
        let count = wire.requests().len();
        assert!(run(session.capture_catalog(now())).is_err());
        assert_eq!(wire.requests().len(), count);
    }
}

#[test]
fn catalog_read_preserves_gate_artifact_and_cancellation_barriers() {
    for fault in 0..3 {
        let fixture = Fixture::new();
        let page = catalog_page(CatalogKind::Bases, 0, 1);
        let log = FakeLog::new([offline()]);
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        if fault == 0 {
            fs::write(&fixture.metadata, b"changed catalog metadata").unwrap();
        }
        if fault == 1 {
            let changed = log.clone();
            wire.push(Step::Action(
                Box::new(move || changed.set(online())),
                Response {
                    code: ResponseCode::Done,
                    payload: catalog::encode(&page).unwrap(),
                },
            ));
        }
        if fault == 2 {
            wire.push(Step::Pending);
            let mut future = Box::pin(session.read_catalog_page(now(), CatalogKind::Bases, 0));
            let mut context = Context::from_waker(Waker::noop());
            assert!(future.as_mut().poll(&mut context).is_pending());
            drop(future);
            assert_eq!(session.state(), SessionState::Connecting);
            assert!(matches!(run(session.capture_catalog(now())), Err(SessionError::Busy)));
        } else {
            assert!(run(session.read_catalog_page(now(), CatalogKind::Bases, 0)).is_err());
            assert_eq!(wire.requests().len(), if fault == 0 { 2 } else { 3 });
            assert_ne!(session.state(), SessionState::Ready);
        }
        assert!(session.runtime_status().is_none());
    }
}

fn now() -> Timestamp {
    started() + SignedDuration::from_secs(120)
}

fn offline() -> GameLogState {
    GameLogState::started()
        .apply(GameLogEvent::ShellStateChanged("CharacterSelect".into()))
        .apply(GameLogEvent::CharacterLoaded(LoggedCharacter {
            name: "Owned test character".into(),
            level: 23,
            online: false,
            cycle: "Legacy".into(),
            class: "Rogue".into(),
        }))
        .apply(GameLogEvent::ShellStateChanged("None".into()))
}

fn online() -> GameLogState {
    let mut state = offline();
    state.readiness = GameReadiness::Online;
    state.character.as_mut().unwrap().online = true;
    state
}

fn menu() -> GameLogState {
    GameLogState::started().apply(GameLogEvent::ShellStateChanged("CharacterSelect".into()))
}

fn done(payload: Vec<u8>) -> Step {
    Step::Reply(Ok(Response {
        code: ResponseCode::Done,
        payload,
    }))
}

fn status(online: bool, manager: u64, features_on: u32) -> Step {
    done(RuntimeStatus { online, manager, features_on }.encode().unwrap().to_vec())
}

fn clear() -> Step {
    done(0_i32.to_le_bytes().to_vec())
}

fn forge_item() -> ForgeSnapshot {
    let mut snapshot = ForgeSnapshot {
        item: 0x1020_3040,
        individual_id: 0x89ab_cdef,
        fingerprint: 0,
        name: "Owned forge item 🗡".into(),
        item_type: 3,
        sub_type: 42,
        rarity: 5,
        class_requirement: -17,
        affixes: vec![AffixSnapshot {
            slot: 0,
            id: 123,
            tier: 8,
            roll: 255,
            kind: 2,
            seal: -1,
            name: "Authored affix".into(),
            special: 3,
        }],
        implicit_rolls: vec![0, 127, 255],
        identity: None,
        status: ItemStatus {
            forging_potential: 17,
            forging_potential_type: -3,
            legendary_potential: 2,
            weavers_will: 9,
            weavers_touch: 4,
            corrupted: true,
            ruined: false,
            level_requirement: 52,
        },
    };
    snapshot.fingerprint = snapshot.legacy_fingerprint();
    snapshot
}

fn item_limits() -> ItemLimits {
    ItemLimits {
        forging_potential: ValueRange { minimum: 3, maximum: 211 },
        legendary_potential: ValueRange { minimum: 1, maximum: 6 },
        weavers_will: ValueRange { minimum: 2, maximum: 49 },
        weavers_touch: ValueRange { minimum: 4, maximum: 77 },
        level_requirement: ValueRange { minimum: 0, maximum: 137 },
    }
}

enum Step {
    Reply(Result<Response, TransportError>),
    Action(Box<dyn FnOnce() + Send>, Response),
    Pending,
}

#[derive(Clone)]
struct FakeTransport(Arc<Mutex<Wire>>, ProcessIdentity);

struct Wire {
    requests: Vec<Request>,
    steps: VecDeque<Step>,
}

impl FakeTransport {
    fn new(target: ProcessIdentity, steps: impl IntoIterator<Item = Step>) -> Self {
        Self(
            Arc::new(Mutex::new(Wire {
                requests: Vec::new(),
                steps: steps.into_iter().collect(),
            })),
            target,
        )
    }

    fn requests(&self) -> Vec<Request> {
        self.0.lock().unwrap().requests.clone()
    }

    fn push(&self, step: Step) {
        self.0.lock().unwrap().steps.push_back(step);
    }
}

impl Transport for FakeTransport {
    fn target(&self) -> &ProcessIdentity {
        &self.1
    }

    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        let step = {
            let mut wire = self.0.lock().unwrap();
            wire.requests.push(request.clone());
            wire.steps.pop_front().expect("unexpected helper request")
        };
        match step {
            Step::Reply(response) => response,
            Step::Action(action, response) => {
                action();
                Ok(response)
            }
            Step::Pending => pending().await,
        }
    }
}

#[derive(Clone)]
struct FakeLog(Arc<Mutex<Log>>);

struct Log {
    states: VecDeque<GameLogState>,
    process_starts: Vec<Timestamp>,
}

impl FakeLog {
    fn new(states: impl IntoIterator<Item = GameLogState>) -> Self {
        Self(Arc::new(Mutex::new(Log {
            states: states.into_iter().collect(),
            process_starts: Vec::new(),
        })))
    }

    fn set(&self, state: GameLogState) {
        self.0.lock().unwrap().states = [state].into();
    }

    fn reads(&self) -> Vec<Timestamp> {
        self.0.lock().unwrap().process_starts.clone()
    }
}

impl LogSource for FakeLog {
    fn read(&mut self, process_start: Timestamp) -> GameLogState {
        let mut log = self.0.lock().unwrap();
        log.process_starts.push(process_start);
        if log.states.len() > 1 {
            log.states.pop_front().unwrap()
        } else {
            log.states.front().expect("log fixture requires a state").clone()
        }
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
        let metadata = temp.path().join("Last Epoch_Data/il2cpp_data/Metadata/global-metadata.dat");
        fs::create_dir_all(metadata.parent().unwrap()).unwrap();
        fs::write(&metadata, b"owned metadata fixture").unwrap();
        let executable = temp.path().join("Last Epoch.exe");
        fs::write(&executable, b"owned executable identity; never executed").unwrap();
        let dispatcher = DispatcherSpec {
            rva: 0x1000,
            bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
        };
        let mut image = vec![0; 0x400];
        image[..2].copy_from_slice(b"MZ");
        image[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        image[0x80..0x84].copy_from_slice(b"PE\0\0");
        image[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        image[0x86..0x88].copy_from_slice(&1_u16.to_le_bytes());
        image[0x94..0x96].copy_from_slice(&240_u16.to_le_bytes());
        image[0x96..0x98].copy_from_slice(&0x2022_u16.to_le_bytes());
        image[0x98..0x9a].copy_from_slice(&0x20b_u16.to_le_bytes());
        image[0xb0..0xb8].copy_from_slice(&0x0001_8000_0000_u64.to_le_bytes());
        image[0xb8..0xbc].copy_from_slice(&0x1000_u32.to_le_bytes());
        image[0xbc..0xc0].copy_from_slice(&0x200_u32.to_le_bytes());
        image[0xd0..0xd4].copy_from_slice(&0x2000_u32.to_le_bytes());
        image[0xd4..0xd8].copy_from_slice(&0x200_u32.to_le_bytes());
        image[0x104..0x108].copy_from_slice(&16_u32.to_le_bytes());
        let section = 0x188;
        image[section..section + 8].copy_from_slice(b".text\0\0\0");
        image[section + 8..section + 12].copy_from_slice(&0x100_u32.to_le_bytes());
        image[section + 12..section + 16].copy_from_slice(&0x1000_u32.to_le_bytes());
        image[section + 16..section + 20].copy_from_slice(&0x200_u32.to_le_bytes());
        image[section + 20..section + 24].copy_from_slice(&0x200_u32.to_le_bytes());
        image[section + 36..section + 40].copy_from_slice(&0x6000_0020_u32.to_le_bytes());
        image[0x200..0x210].copy_from_slice(&dispatcher.bytes);
        fs::write(&binary, image).unwrap();
        let created = u64::try_from(started().as_nanosecond() / 100 + 116_444_736_000_000_000).unwrap();
        let target = ProcessIdentity { pid: 42, created, executable };
        Self {
            _temp: temp,
            binary,
            metadata,
            target,
            dispatcher,
        }
    }

    fn profile(&self) -> BuildProfile {
        BuildProfile::capture(&self.binary, &self.metadata, HELPER_ID).unwrap()
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            build_id: HELPER_ID.to_ascii_uppercase(),
            dispatcher: self.dispatcher,
            offsets: REQUIRED_KEYS.iter().enumerate().map(|(index, key)| ((*key).into(), 16 + index as u64 * 8)).collect(),
        }
    }

    fn discover(&self) -> Step {
        done(self.discovery().encode(42).unwrap())
    }

    fn session(&self, wire: &FakeTransport, log: &FakeLog, policy: GatePolicy) -> Session<FakeTransport, FakeLog> {
        Session::new(self.target.clone(), wire.clone(), log.clone(), self.profile(), policy).unwrap()
    }
}

fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn disabled_by_default_and_release_gate_precedes_all_transport_io() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), []);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    assert_eq!(session.state(), SessionState::Disabled);
    assert!(matches!(run(session.connect(now())), Err(SessionError::Disabled)));
    assert!(wire.requests().is_empty());
    assert!(log.reads().is_empty());
    session.enable();
    assert_eq!(session.state(), SessionState::Disconnected);
    for state in [GameLogState::missing(), GameLogState::stale(), online(), menu()] {
        log.set(state);
        assert!(matches!(run(session.connect(now())), Err(SessionError::Gate(_))));
        assert!(wire.requests().is_empty());
    }
    assert!(log.reads().iter().all(|stamp| *stamp == started()));
}

#[test]
fn session_rejects_a_transport_bound_to_another_process_identity_before_io() {
    let fixture = Fixture::new();
    let log = FakeLog::new([offline()]);
    for other in [
        ProcessIdentity {
            pid: 43,
            ..fixture.target.clone()
        },
        ProcessIdentity {
            created: fixture.target.created + 1,
            ..fixture.target.clone()
        },
        ProcessIdentity {
            executable: fixture.target.executable.with_file_name("Different.exe"),
            ..fixture.target.clone()
        },
    ] {
        let wire = FakeTransport::new(other, []);
        assert!(matches!(
            Session::new(fixture.target.clone(), wire.clone(), log.clone(), fixture.profile(), GatePolicy::Release),
            Err(SessionError::Identity(_))
        ));
        assert!(wire.requests().is_empty());
        assert!(log.reads().is_empty());
    }
}

#[test]
fn session_rejects_identical_artifacts_outside_the_target_installation_before_io() {
    let fixture = Fixture::new();
    let other_installation = Fixture::new();
    let copied_metadata = fixture._temp.path().join("replacement-metadata.dat");
    fs::copy(&fixture.metadata, &copied_metadata).unwrap();
    let expected = fixture.profile();
    for unrelated in [
        other_installation.profile(),
        BuildProfile::capture(&other_installation.binary, &fixture.metadata, HELPER_ID).unwrap(),
        BuildProfile::capture(&fixture.binary, &other_installation.metadata, HELPER_ID).unwrap(),
        BuildProfile::capture(&fixture.binary, &copied_metadata, HELPER_ID).unwrap(),
    ] {
        // The content hashes and helper ID match; only installation provenance differs.
        assert_eq!(unrelated.binary_hash(), expected.binary_hash());
        assert_eq!(unrelated.metadata_hash(), expected.metadata_hash());
        assert_eq!(unrelated.helper_build_id(), expected.helper_build_id());
        let wire = FakeTransport::new(fixture.target.clone(), []);
        let log = FakeLog::new([offline()]);
        assert!(matches!(
            Session::new(fixture.target.clone(), wire.clone(), log.clone(), unrelated, GatePolicy::Release),
            Err(SessionError::Identity(_))
        ));
        assert!(wire.requests().is_empty());
        assert!(log.reads().is_empty());
    }
}

#[test]
fn startup_grace_blocks_probes_then_development_can_check_runtime() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x10000, 0), clear()]);
    let log = FakeLog::new([GameLogState::missing()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Development);
    session.enable();
    let early = started() + SignedDuration::from_secs(59);
    assert!(matches!(run(session.connect(early)), Err(SessionError::Gate(decision)) if decision.verdict == GateVerdict::Wait));
    assert_eq!(session.state(), SessionState::WaitingForGame);
    assert!(wire.requests().is_empty());
    run(session.connect(started() + SignedDuration::from_secs(60))).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(run(session.disable()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disabled);
}

#[test]
fn successful_handshake_rechecks_log_and_clears_hooks_on_disable() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 2), clear()]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.target(), &fixture.target);
    assert_eq!(session.runtime_status().unwrap().features_on, 2);
    assert_eq!(log.reads(), [started(), started()]);
    assert_eq!(run(session.disable()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disabled);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(
        wire.requests(),
        [
            Request::Discover,
            Request::Status { dispatcher: fixture.dispatcher },
            Request::ClearHooks { dispatcher: fixture.dispatcher }
        ]
    );
    assert!(matches!(run(session.connect(now())), Err(SessionError::Disabled)));
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn reconnecting_invalidates_the_previous_cleanup_acknowledgement() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [fixture.discover(), status(false, 0x1234, 1), clear(), fixture.discover(), status(false, 0x1234, 3), clear()],
    );
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.cleanup(), Cleanup::ConfirmedOff);
    run(session.connect(now())).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.runtime_status().unwrap().features_on, 3);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    assert_eq!(wire.requests().len(), 5);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(wire.requests().len(), 6);
}

#[test]
fn helper_offline_required_releases_attachment_without_cleanup_afterward() {
    for during_refresh in [false, true] {
        let fixture = Fixture::new();
        let mut steps = vec![fixture.discover()];
        if during_refresh {
            steps.push(status(false, 0x1234, 2));
        }
        steps.push(Step::Reply(Ok(Response {
            code: ResponseCode::OfflineRequired,
            payload: b"diagnostic text cannot authorize cleanup".to_vec(),
        })));
        let wire = FakeTransport::new(fixture.target.clone(), steps);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        let result = if during_refresh {
            run(session.connect(now())).unwrap();
            assert_eq!(session.state(), SessionState::Ready);
            run(session.refresh(now()))
        } else {
            run(session.connect(now()))
        };
        assert!(matches!(
            result,
            Err(SessionError::Helper {
                code: ResponseCode::OfflineRequired,
                may_have_run: false,
                ..
            })
        ));
        assert_eq!(session.state(), SessionState::WaitingForCharacter);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        let before_disconnect = wire.requests();
        assert_eq!(before_disconnect.len(), if during_refresh { 3 } else { 2 });
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(run(session.disable()), Cleanup::Unconfirmed);
        assert_eq!(session.state(), SessionState::Disabled);
        assert_eq!(wire.requests(), before_disconnect);
        assert!(!wire.requests().iter().any(|request| matches!(request, Request::ClearHooks { .. })));
    }
}

#[test]
fn log_changing_during_discovery_prevents_the_first_status() {
    for next in [online(), menu(), GameLogState::missing()] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover()]);
        let log = FakeLog::new([offline(), next]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        assert!(matches!(run(session.connect(now())), Err(SessionError::Gate(_))));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(wire.requests(), [Request::Discover]);
        assert_eq!(log.reads(), [started(), started()]);
    }
}

#[test]
fn incompatible_discovery_is_rejected_before_status() {
    let fixture = Fixture::new();
    let valid = fixture.discovery().encode(42).unwrap();
    let mut wrong_version = valid.clone();
    wrong_version[..4].copy_from_slice(&20_u32.to_le_bytes());
    let wrong_pid = fixture.discovery().encode(43).unwrap();
    let mut wrong_build = fixture.discovery();
    wrong_build.build_id = "f".repeat(64);
    let wrong_build = wrong_build.encode(42).unwrap();
    let mut wrong_bytes = fixture.discovery();
    wrong_bytes.dispatcher.bytes[0] ^= 1;
    let wrong_bytes = wrong_bytes.encode(42).unwrap();
    let mut missing_field = valid.clone();
    let key_position = missing_field
        .windows(REQUIRED_KEYS[0].len())
        .position(|value| value == REQUIRED_KEYS[0].as_bytes())
        .unwrap();
    missing_field[key_position] = b'X';
    for payload in [wrong_version, wrong_pid, wrong_build, wrong_bytes, missing_field] {
        let wire = FakeTransport::new(fixture.target.clone(), [done(payload)]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        assert!(run(session.connect(now())).is_err());
        assert_eq!(session.state(), SessionState::Blocked);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests(), [Request::Discover]);
        assert_eq!(log.reads().len(), 1);
    }
}

#[test]
fn artifacts_changed_while_discovery_is_in_flight_prevent_status() {
    for change_binary in [false, true] {
        let fixture = Fixture::new();
        let changed = if change_binary { fixture.binary.clone() } else { fixture.metadata.clone() };
        let payload = fixture.discovery().encode(42).unwrap();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [Step::Action(
                Box::new(move || fs::write(changed, b"changed after capture").unwrap()),
                Response {
                    code: ResponseCode::Done,
                    payload,
                },
            )],
        );
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        assert!(matches!(run(session.connect(now())), Err(SessionError::Identity(_))));
        assert_eq!(wire.requests(), [Request::Discover]);
    }
}

#[test]
fn runtime_online_or_absent_character_releases_attachment_without_further_calls() {
    for (online, manager, readiness) in [
        (true, 0x1234, GameReadiness::Online),
        (false, 0, GameReadiness::NoCharacter),
        (true, 0, GameReadiness::NoCharacter),
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(online, manager, 3)]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        assert!(matches!(run(session.connect(now())), Err(SessionError::NotOffline(actual)) if actual == readiness));
        assert_eq!(session.state(), SessionState::WaitingForCharacter);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }]);
    }
}

#[test]
fn refresh_losing_offline_log_releases_attachment_without_further_calls() {
    for unavailable in [false, true] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 1)]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        log.set(if unavailable {
            GameLogState {
                status: GameLogStatus::Unreadable,
                ..offline()
            }
        } else {
            online()
        });
        assert!(matches!(run(session.refresh(now())), Err(SessionError::Gate(_))));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }]);
    }
}

#[test]
fn refresh_observing_runtime_online_revokes_readiness_without_further_calls() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 1), status(true, 0x1234, 1)]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert!(matches!(run(session.refresh(now())), Err(SessionError::NotOffline(GameReadiness::Online))));
    assert_eq!(session.state(), SessionState::WaitingForCharacter);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(run(session.disable()), Cleanup::Unconfirmed);
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn failed_or_incomplete_cleanup_never_claims_features_are_off() {
    let failed = || {
        Step::Reply(Err(TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Timeout,
        }))
    };
    // An on state, a helper failure, a malformed response, and transport loss.
    let on = vec![1, 0, 0, 0, 1, b'x', 1, 0, 0];
    for cleanup in [
        failed(),
        done(on),
        done(vec![0]),
        Step::Reply(Ok(Response {
            code: ResponseCode::Failed,
            payload: b"cleanup failed".to_vec(),
        })),
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 1), cleanup]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests().len(), 3);
    }
}

#[test]
fn canceled_attachment_stays_connecting_until_explicit_disconnect() {
    let fixture = Fixture::new();
    // Discovery completes; the Status was submitted but no answer arrived.
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), Step::Pending, clear()]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    let mut attaching = Box::pin(session.connect(now()));
    assert!(matches!(attaching.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(attaching);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.connect(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.refresh(now())), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 2);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests().len(), 3);
    wire.push(fixture.discover());
    wire.push(status(false, 0x1234, 0));
    run(session.connect(now())).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 5);
}

#[test]
fn canceled_discovery_requires_explicit_disconnect_without_cleanup_replay() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [Step::Pending]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    let mut attaching = Box::pin(session.connect(now()));
    assert!(matches!(attaching.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(attaching);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.connect(now())), Err(SessionError::Busy)));
    assert_eq!(run(session.disconnect()), Cleanup::NotNeeded);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests(), [Request::Discover]);
}

#[test]
fn canceled_cleanup_remains_closing_until_explicit_disconnect_finishes() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 1), Step::Pending, clear()]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    // Disconnect's timeout requires a runtime, but cancellation itself is manual.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let entered = runtime.enter();
    let mut closing = Box::pin(session.disconnect());
    assert!(matches!(closing.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(closing);
    drop(entered);
    assert_eq!(session.state(), SessionState::Closing);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    assert_eq!(session.runtime_status(), None);
    session.enable();
    assert_eq!(session.state(), SessionState::Closing);
    assert!(matches!(run(session.connect(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.refresh(now())), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests().len(), 4);
}

#[test]
fn helper_failure_codes_preserve_uncertainty_and_do_not_replay_requests() {
    for code in [
        ResponseCode::Failed,
        ResponseCode::NotReady,
        ResponseCode::StaleItem,
        ResponseCode::Refused,
        ResponseCode::OfflineRequired,
        ResponseCode::RestartRequired,
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                Step::Reply(Ok(Response {
                    code,
                    payload: b"failure text has no control meaning".to_vec(),
                })),
            ],
        );
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        match run(session.connect(now())).unwrap_err() {
            SessionError::Helper { code: actual, may_have_run, .. } => {
                assert_eq!(actual, code);
                assert_eq!(may_have_run, code == ResponseCode::Failed);
            }
            error => panic!("unexpected session error: {error}"),
        }
        assert_eq!(
            session.state(),
            match code {
                ResponseCode::NotReady => SessionState::WaitingForGame,
                ResponseCode::OfflineRequired => SessionState::WaitingForCharacter,
                _ => SessionState::Blocked,
            }
        );
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }]);
    }
}

#[test]
fn transport_loss_is_not_replayed_and_retains_delivery_information() {
    for delivery in [Delivery::NotSent, Delivery::MayHaveRun] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                Step::Reply(Err(TransportError {
                    delivery,
                    failure: TransportFailure::Timeout,
                })),
            ],
        );
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        assert!(matches!(run(session.connect(now())), Err(SessionError::Transport(error)) if error.delivery == delivery));
        assert_eq!(session.state(), SessionState::Blocked);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests().len(), 2);
    }
}

#[test]
fn malformed_status_does_not_become_ready_or_trigger_a_replay() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), done(vec![0; 15])]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    assert!(matches!(run(session.connect(now())), Err(SessionError::Protocol(ProtocolError::InvalidData))));
    assert_eq!(session.state(), SessionState::Blocked);
    assert_eq!(wire.requests().len(), 2);
}

#[test]
fn profile_requires_file_backed_executable_dispatcher_and_bounded_headers() {
    let fixture = Fixture::new();
    fixture.profile().validate(&fixture.dispatcher).unwrap();
    for rva in [0_u64, 0x11f1, 0x1200, u64::MAX] {
        assert!(fixture.profile().validate(&DispatcherSpec { rva, ..fixture.dispatcher }).is_err());
    }
    let original = fs::read(&fixture.binary).unwrap();
    for (index, value) in [(0x84, 0x4c), (0x99, 0x01), (0x188 + 39, 0x40)] {
        let mut malformed = original.clone();
        malformed[index] = value;
        fs::write(&fixture.binary, malformed).unwrap();
        assert!(fixture.profile().validate(&fixture.dispatcher).is_err());
    }
    for length in [0, 0x3f, 0x80, 0x98, 0x188, 0x20f] {
        fs::write(&fixture.binary, &original[..length]).unwrap();
        assert!(fixture.profile().validate(&fixture.dispatcher).is_err());
    }
}

#[test]
fn forge_read_requires_enabled_ready_and_offline_admission_before_io() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), []);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::Disabled)));
    assert!(log.reads().is_empty());
    session.enable();
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::NotConnected)));
    for denied in [GameLogState::missing(), GameLogState::stale(), menu(), online()] {
        log.set(denied);
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::Gate(_))));
        assert_eq!(session.runtime_status(), None);
    }
    assert!(wire.requests().is_empty());
}

#[test]
fn forge_read_returns_owned_item_then_empty_without_losing_readiness() {
    let fixture = Fixture::new();
    let expected = forge_item();
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            done(forge::encode(Some(&expected)).unwrap()),
            done(forge::encode(None).unwrap()),
        ],
    );
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let initial_status = *session.runtime_status().unwrap();
    let observed = run(session.read_forge(now())).unwrap().unwrap();
    assert_eq!(observed, expected);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.runtime_status(), Some(&initial_status));
    assert_eq!(run(session.read_forge(now())).unwrap(), None);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.runtime_status(), Some(&initial_status));
    assert_eq!(
        wire.requests(),
        [
            Request::Discover,
            Request::Status { dispatcher: fixture.dispatcher },
            Request::ReadForge { dispatcher: fixture.dispatcher },
            Request::ReadForge { dispatcher: fixture.dispatcher }
        ]
    );
    drop(session);
    // The result owns its strings, affixes and roll bytes; ending the session leaves them valid.
    assert_eq!(observed.name, "Owned forge item 🗡");
    assert_eq!(observed.affixes[0].name, "Authored affix");
    assert_eq!(observed.implicit_rolls, [0, 127, 255]);
    assert_eq!(wire.requests().len(), 4);
}

#[test]
fn changed_artifacts_prevent_forge_io_and_release_the_session() {
    for change_binary in [false, true] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        fs::write(if change_binary { &fixture.binary } else { &fixture.metadata }, b"changed before forge read").unwrap();
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::Identity(_))));
        assert_eq!(session.state(), SessionState::Blocked);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }]);
    }
}

#[test]
fn lost_log_admission_prevents_forge_io_even_after_artifact_validation() {
    for lose_on_second_check in [false, true] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let states = if lose_on_second_check {
            vec![offline(), offline(), offline(), online()]
        } else {
            vec![offline(), offline(), online()]
        };
        let log = FakeLog::new(states);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::Gate(_))));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }]);
    }
}

#[test]
fn log_lost_while_forge_reply_is_in_flight_discards_snapshot_without_more_io() {
    for denied in [online(), menu(), GameLogState::missing(), GameLogState::stale()] {
        let fixture = Fixture::new();
        let log = FakeLog::new([offline()]);
        let changed_log = log.clone();
        let reply = Response {
            code: ResponseCode::Done,
            payload: forge::encode(Some(&forge_item())).unwrap(),
        };
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [fixture.discover(), status(false, 0x1234, 0), Step::Action(Box::new(move || changed_log.set(denied)), reply)],
        );
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::Gate(_))));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(run(session.disable()), Cleanup::Unconfirmed);
        assert_eq!(
            wire.requests(),
            [
                Request::Discover,
                Request::Status { dispatcher: fixture.dispatcher },
                Request::ReadForge { dispatcher: fixture.dispatcher }
            ]
        );
    }
}

#[test]
fn rejected_forge_reply_blocks_without_replay_and_requires_an_explicit_new_handshake() {
    let cases = [
        done(vec![1]),
        Step::Reply(Ok(Response {
            code: ResponseCode::Failed,
            payload: b"read failed".to_vec(),
        })),
        Step::Reply(Err(TransportError {
            delivery: Delivery::NotSent,
            failure: TransportFailure::Timeout,
        })),
        Step::Reply(Err(TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Timeout,
        })),
    ];
    for (case, reply) in cases.into_iter().enumerate() {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), reply]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        let error = run(session.read_forge(now())).unwrap_err();
        match case {
            0 => assert!(matches!(error, SessionError::Protocol(ProtocolError::InvalidData))),
            1 => assert!(matches!(
                error,
                SessionError::Helper {
                    code: ResponseCode::Failed,
                    may_have_run: true,
                    ..
                }
            )),
            2 => assert!(matches!(error, SessionError::Transport(TransportError { delivery: Delivery::NotSent, .. }))),
            3 => assert!(matches!(
                error,
                SessionError::Transport(TransportError {
                    delivery: Delivery::MayHaveRun,
                    ..
                })
            )),
            _ => unreachable!(),
        }
        assert_eq!(session.state(), SessionState::Blocked);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        session.enable();
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::NotConnected)));
        assert!(matches!(run(session.refresh(now())), Err(SessionError::NotConnected)));
        assert_eq!(wire.requests().len(), 3);
        wire.push(fixture.discover());
        wire.push(status(false, 0x5678, 0));
        run(session.connect(now())).unwrap();
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(
            wire.requests(),
            [
                Request::Discover,
                Request::Status { dispatcher: fixture.dispatcher },
                Request::ReadForge { dispatcher: fixture.dispatcher },
                Request::Discover,
                Request::Status { dispatcher: fixture.dispatcher }
            ]
        );
        wire.push(done(forge::encode(None).unwrap()));
        assert_eq!(run(session.read_forge(now())).unwrap(), None);
        assert_eq!(wire.requests().len(), 6);
    }
}

#[test]
fn forge_runtime_offline_refusal_waits_without_io_until_an_explicit_connect() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            Step::Reply(Ok(Response {
                code: ResponseCode::OfflineRequired,
                payload: b"manager destroyed before graph access".to_vec(),
            })),
        ],
    );
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert!(matches!(
        run(session.read_forge(now())),
        Err(SessionError::Helper {
            code: ResponseCode::OfflineRequired,
            may_have_run: false,
            ..
        })
    ));
    assert_eq!(session.state(), SessionState::WaitingForCharacter);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    session.enable();
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::NotConnected)));
    assert!(matches!(run(session.refresh(now())), Err(SessionError::NotConnected)));
    assert_eq!(session.state(), SessionState::WaitingForCharacter);
    assert_eq!(wire.requests().len(), 3);
    wire.push(fixture.discover());
    wire.push(status(false, 0x5678, 0));
    run(session.connect(now())).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 5);
}

#[test]
fn canceled_forge_read_remains_busy_until_explicit_disconnect_without_replay() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), Step::Pending, clear()]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let mut reading = Box::pin(session.read_forge(now()));
    assert!(matches!(reading.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(reading);
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    session.enable();
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.connect(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.refresh(now())), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(
        wire.requests(),
        [
            Request::Discover,
            Request::Status { dispatcher: fixture.dispatcher },
            Request::ReadForge { dispatcher: fixture.dispatcher },
            Request::ClearHooks { dispatcher: fixture.dispatcher }
        ]
    );
    wire.push(fixture.discover());
    wire.push(status(false, 0x1234, 0));
    run(session.connect(now())).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 6);
}

#[test]
fn literal_limits_require_admission_and_keep_owned_results_across_forge_reads() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), []);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::Disabled)));
    assert!(log.reads().is_empty());
    session.enable();
    assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::NotConnected)));
    assert!(wire.requests().is_empty());
    wire.push(fixture.discover());
    wire.push(status(false, 0x1234, 0));
    wire.push(done(definitions::encode(&item_limits()).unwrap()));
    wire.push(done(forge::encode(None).unwrap()));
    run(session.connect(now())).unwrap();
    let initial_status = *session.runtime_status().unwrap();
    let limits = run(session.read_item_limits(now())).unwrap();
    assert_eq!(limits, item_limits());
    assert_eq!(run(session.read_forge(now())).unwrap(), None);
    assert_eq!(session.runtime_status(), Some(&initial_status));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests()[2], Request::ReadItemLimits { dispatcher: fixture.dispatcher });
    log.set(online());
    assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::Gate(_))));
    assert_eq!(wire.requests().len(), 4);
    drop(session);
    assert_eq!(limits, item_limits());
}

#[test]
fn literal_limits_fail_closed_on_malformed_reply_runtime_refusal_and_transport_uncertainty() {
    for (reply, state) in [
        (done(vec![0; 39]), SessionState::Blocked),
        (done(vec![0; 41]), SessionState::Blocked),
        (done([256_i32.to_le_bytes().as_slice(), &[0; 36]].concat()), SessionState::Blocked),
        (
            Step::Reply(Ok(Response {
                code: ResponseCode::OfflineRequired,
                payload: b"runtime went online".to_vec(),
            })),
            SessionState::WaitingForCharacter,
        ),
        (
            Step::Reply(Err(TransportError {
                delivery: Delivery::MayHaveRun,
                failure: TransportFailure::Timeout,
            })),
            SessionState::Blocked,
        ),
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), reply]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(run(session.read_item_limits(now())).is_err());
        assert_eq!(session.state(), state);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::NotConnected)));
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::NotConnected)));
        assert_eq!(wire.requests().len(), 3);
    }
}

#[test]
fn literal_limits_recheck_artifacts_and_discard_replies_after_lost_log_admission() {
    for change_artifact in [false, true] {
        let fixture = Fixture::new();
        let log = FakeLog::new([offline()]);
        let changed_log = log.clone();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                status(false, 0x1234, 0),
                Step::Action(
                    Box::new(move || changed_log.set(online())),
                    Response {
                        code: ResponseCode::Done,
                        payload: definitions::encode(&item_limits()).unwrap(),
                    },
                ),
            ],
        );
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        if change_artifact {
            fs::write(&fixture.metadata, b"changed before literal read").unwrap();
            assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::Identity(_))));
        } else {
            assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::Gate(_))));
        }
        assert_eq!(session.state(), if change_artifact { SessionState::Blocked } else { SessionState::WaitingForCharacter });
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests().len(), if change_artifact { 2 } else { 3 });
    }
}

#[test]
fn canceled_literal_limits_read_blocks_both_read_operations_until_explicit_disconnect() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), Step::Pending, clear()]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let mut reading = Box::pin(session.read_item_limits(now()));
    assert!(matches!(reading.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(reading);
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(session.runtime_status(), None);
    assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests()[3], Request::ClearHooks { dispatcher: fixture.dispatcher });
}

#[test]
fn locale_requires_ready_and_returns_owned_changed_language_with_stable_version() {
    let fixture = Fixture::new();
    let first = GameLocale {
        language: "en-Ω-🌿".into(),
        game_version: "23.7.137-fixture".into(),
    };
    let changed = GameLocale {
        language: "sr-Latn-RS".into(),
        ..first.clone()
    };
    let wire = FakeTransport::new(fixture.target.clone(), []);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    assert!(matches!(run(session.read_locale(now())), Err(SessionError::Disabled)));
    session.enable();
    assert!(matches!(run(session.read_locale(now())), Err(SessionError::NotConnected)));
    assert!(wire.requests().is_empty());
    for step in [
        fixture.discover(),
        status(false, 0x1234, 0),
        done(locale::encode(&first).unwrap()),
        done(locale::encode(&changed).unwrap()),
    ] {
        wire.push(step);
    }
    run(session.connect(now())).unwrap();
    let initial = run(session.read_locale(now())).unwrap();
    assert_eq!(initial, first);
    assert_eq!(run(session.read_locale(now())).unwrap(), changed);
    assert_eq!(initial, first);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests()[2], Request::ReadLocale { dispatcher: fixture.dispatcher });
    assert_eq!(wire.requests()[3], Request::ReadLocale { dispatcher: fixture.dispatcher });
    drop(session);
    assert_eq!(initial.language, "en-Ω-🌿");
}

#[test]
fn unready_or_malformed_locale_requires_explicit_reconnect_without_replay() {
    for step in [
        done(vec![1, 0, 0, 0, b'e']),
        Step::Reply(Ok(Response {
            code: ResponseCode::NotReady,
            payload: b"selection operation pending".to_vec(),
        })),
        Step::Reply(Ok(Response {
            code: ResponseCode::OfflineRequired,
            payload: b"runtime online".to_vec(),
        })),
        Step::Reply(Err(TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Timeout,
        })),
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), step]);
        let log = FakeLog::new([offline()]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(run(session.read_locale(now())).is_err());
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert!(matches!(run(session.read_locale(now())), Err(SessionError::NotConnected)));
        assert!(matches!(run(session.read_forge(now())), Err(SessionError::NotConnected)));
        assert_eq!(wire.requests().len(), 3);
        let expected = GameLocale {
            language: "ja-JP".into(),
            game_version: String::new(),
        };
        for step in [fixture.discover(), status(false, 0x1234, 0), done(locale::encode(&expected).unwrap())] {
            wire.push(step);
        }
        run(session.connect(now())).unwrap();
        assert_eq!(run(session.read_locale(now())).unwrap(), expected);
        assert_eq!(wire.requests().len(), 6);
    }
}

#[test]
fn locale_discards_inflight_reply_after_admission_loss_and_rechecks_artifacts() {
    for change_artifact in [false, true] {
        let fixture = Fixture::new();
        let log = FakeLog::new([offline()]);
        let changed_log = log.clone();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                status(false, 0x1234, 0),
                Step::Action(
                    Box::new(move || changed_log.set(online())),
                    Response {
                        code: ResponseCode::Done,
                        payload: locale::encode(&GameLocale {
                            language: "en-US".into(),
                            game_version: "1.5".into(),
                        })
                        .unwrap(),
                    },
                ),
            ],
        );
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        if change_artifact {
            fs::write(&fixture.binary, b"changed before locale read").unwrap();
            assert!(matches!(run(session.read_locale(now())), Err(SessionError::Identity(_))));
        } else {
            assert!(matches!(run(session.read_locale(now())), Err(SessionError::Gate(_))));
        }
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests().len(), if change_artifact { 2 } else { 3 });
        assert!(run(session.read_locale(now())).is_err());
        assert_eq!(wire.requests().len(), if change_artifact { 2 } else { 3 });
    }
}

#[test]
fn canceled_locale_read_preserves_shared_busy_barrier_until_disconnect() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), Step::Pending, clear()]);
    let log = FakeLog::new([offline()]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let mut reading = Box::pin(session.read_locale(now()));
    assert!(matches!(reading.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(reading);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.read_locale(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.read_item_limits(now())), Err(SessionError::Busy)));
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
}
