//! Session admission uses owned artifacts and scripted services; no game is opened.
#[path = "session/descriptions.rs"]
mod descriptions;
#[path = "session/fields.rs"]
mod fields;
#[path = "session/hooks.rs"]
mod hooks;
#[path = "session/mapping.rs"]
mod mappings;
#[path = "session/unique.rs"]
mod unique;
#[path = "session/write.rs"]
mod writes;
use eg_core::game_log::GameReadiness;
use eg_trainer::{
    connect::build::BuildProfile,
    session::{Cleanup, Session, SessionError, SessionState},
};
use eg_trainer_wire::{
    definitions::{ItemLimits, ValueRange},
    forge::{self, AffixSnapshot, ForgeSnapshot, ItemStatus},
    locale::{self, GameLocale},
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, ProtocolError, Request, Response, ResponseCode, RuntimeStatus},
    transport::{Delivery, Transport, TransportError, TransportFailure},
};
use jiff::Timestamp;
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
    done(Vec::new())
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
    departure_proven: bool,
}

impl FakeTransport {
    fn new(target: ProcessIdentity, steps: impl IntoIterator<Item = Step>) -> Self {
        Self(
            Arc::new(Mutex::new(Wire {
                requests: Vec::new(),
                steps: steps.into_iter().collect(),
                departure_proven: true,
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

    async fn unload(&mut self) -> Result<(), TransportError> {
        let response = self.send(&Request::Unload).await?;
        if response.code == ResponseCode::Done && response.payload.is_empty() && self.0.lock().unwrap().departure_proven {
            Ok(())
        } else {
            Err(TransportError {
                delivery: Delivery::MayHaveRun,
                failure: TransportFailure::Missing,
            })
        }
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        let step = {
            let mut wire = self.0.lock().unwrap();
            wire.requests.push(request.clone());
            wire.steps.pop_front().unwrap_or_else(|| {
                assert_eq!(*request, Request::Unload, "unexpected helper request");
                Step::Reply(Err(TransportError {
                    delivery: Delivery::NotSent,
                    failure: TransportFailure::Missing,
                }))
            })
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

struct Fixture {
    _writers: Vec<fs::File>,
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
            _writers: vec![],
            binary,
            metadata,
            target,
            dispatcher,
        }
    }

    // Keep compatible writers open before connecting to exercise the full-hash
    // fallback. The ordinary fixture instead pins files for the session.
    fn rehashing() -> Self {
        #[allow(unused_mut)]
        let mut fixture = Self::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            for path in [&fixture.binary, &fixture.metadata] {
                fixture._writers.push(fs::OpenOptions::new().read(true).write(true).share_mode(7).open(path).unwrap());
            }
        }
        fixture
    }

    fn profile(&self) -> BuildProfile {
        BuildProfile::capture(&self.binary, &self.metadata, HELPER_ID).unwrap()
    }

    fn discovery(&self) -> Discovery {
        Discovery {
            build_id: HELPER_ID.to_ascii_uppercase(),
            dispatcher: self.dispatcher,
        }
    }

    fn discover(&self) -> Step {
        done(self.discovery().encode(42).unwrap())
    }

    fn session(&self, wire: &FakeTransport) -> Session<FakeTransport> {
        Session::new(self.target.clone(), wire.clone(), self.profile()).unwrap()
    }
}

fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn session_is_inert_until_enabled_and_connected() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
    let mut session = fixture.session(&wire);
    assert!(matches!(run(session.connect()), Err(SessionError::Disabled)));
    assert!(wire.requests().is_empty());
    session.enable();
    run(session.connect()).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
}

#[test]
fn session_rejects_a_transport_bound_to_another_process_identity_before_io() {
    let fixture = Fixture::new();
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
            Session::new(fixture.target.clone(), wire.clone(), fixture.profile()),
            Err(SessionError::Identity(_))
        ));
        assert!(wire.requests().is_empty());
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
        assert!(matches!(Session::new(fixture.target.clone(), wire.clone(), unrelated), Err(SessionError::Identity(_))));
        assert!(wire.requests().is_empty());
    }
}

#[test]
fn successful_handshake_unloads_on_disable() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 2), clear()]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.target(), &fixture.target);
    assert_eq!(session.runtime_status().unwrap().features_on, 2);
    assert_eq!(run(session.disable()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disabled);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }, Request::Unload]);
    assert!(matches!(run(session.connect()), Err(SessionError::Disabled)));
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn reconnecting_invalidates_the_previous_cleanup_acknowledgement() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [fixture.discover(), status(false, 0x1234, 1), clear(), fixture.discover(), status(false, 0x1234, 3), clear()],
    );
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.cleanup(), Cleanup::ConfirmedOff);
    run(session.connect()).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.runtime_status().unwrap().features_on, 3);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    assert_eq!(wire.requests().len(), 5);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(wire.requests().len(), 6);
}

#[test]
fn helper_offline_required_revokes_gameplay_but_allows_owner_unload() {
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
        let mut session = fixture.session(&wire);
        session.enable();
        let result = if during_refresh {
            run(session.connect()).unwrap();
            assert_eq!(session.state(), SessionState::Ready);
            run(session.refresh())
        } else {
            run(session.connect())
        };
        assert!(matches!(
            result,
            Err(SessionError::Helper {
                code: ResponseCode::OfflineRequired,
                may_have_run: false,
                ..
            })
        ));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        let before_disconnect = wire.requests();
        assert_eq!(before_disconnect.len(), if during_refresh { 3 } else { 2 });
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(run(session.disable()), Cleanup::Unconfirmed);
        assert_eq!(session.state(), SessionState::Disabled);
        assert_eq!(wire.requests(), [before_disconnect, vec![Request::Unload]].concat());
        assert!(!wire.requests().iter().any(|request| matches!(request, Request::ClearHooks { .. })));
    }
}

#[test]
fn incompatible_discovery_is_rejected_before_status() {
    let fixture = Fixture::new();
    let valid = fixture.discovery().encode(42).unwrap();
    let mut wrong_length = valid.clone();
    wrong_length[..4].copy_from_slice(&20_u32.to_le_bytes());
    let wrong_pid = fixture.discovery().encode(43).unwrap();
    let mut wrong_build = fixture.discovery();
    wrong_build.build_id = "f".repeat(64);
    let wrong_build = wrong_build.encode(42).unwrap();
    let mut wrong_bytes = fixture.discovery();
    wrong_bytes.dispatcher.bytes[0] ^= 1;
    let wrong_bytes = wrong_bytes.encode(42).unwrap();
    let missing_field = valid[..valid.len() - 1].to_vec();
    for payload in [wrong_length, wrong_pid, wrong_build, wrong_bytes, missing_field] {
        let wire = FakeTransport::new(fixture.target.clone(), [done(payload)]);
        let mut session = fixture.session(&wire);
        session.enable();
        assert!(run(session.connect()).is_err());
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests(), [Request::Discover]);
    }
}

#[test]
fn artifacts_changed_while_discovery_is_in_flight_prevent_status() {
    for change_binary in [false, true] {
        let fixture = Fixture::rehashing();
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
        let mut session = fixture.session(&wire);
        session.enable();
        assert!(matches!(run(session.connect()), Err(SessionError::Identity(_))));
        assert_eq!(wire.requests(), [Request::Discover]);
    }
}

#[test]
fn runtime_online_or_absent_character_releases_attachment_with_only_owner_unload() {
    for (online, manager, readiness) in [
        (true, 0x1234, GameReadiness::Online),
        (false, 0, GameReadiness::NoCharacter),
        (true, 0, GameReadiness::NoCharacter),
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(online, manager, 3)]);
        let mut session = fixture.session(&wire);
        session.enable();
        assert!(matches!(run(session.connect()), Err(SessionError::NotOffline(actual)) if actual == readiness));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }, Request::Unload]);
    }
}

#[test]
fn refresh_observing_runtime_online_revokes_readiness_with_only_owner_unload() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 1), status(true, 0x1234, 1)]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert!(matches!(run(session.refresh()), Err(SessionError::NotOffline(GameReadiness::Online))));
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(run(session.disable()), Cleanup::Unconfirmed);
    assert_eq!(wire.requests().len(), 4);
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
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
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
    let mut session = fixture.session(&wire);
    session.enable();
    let mut attaching = Box::pin(session.connect());
    assert!(matches!(attaching.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(attaching);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.connect()), Err(SessionError::Busy)));
    assert!(matches!(run(session.refresh()), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 2);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests().len(), 3);
    wire.push(fixture.discover());
    wire.push(status(false, 0x1234, 0));
    run(session.connect()).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 5);
}

#[test]
fn canceled_discovery_requires_explicit_disconnect_without_cleanup_replay() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [Step::Pending]);
    let mut session = fixture.session(&wire);
    session.enable();
    let mut attaching = Box::pin(session.connect());
    assert!(matches!(attaching.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(attaching);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.connect()), Err(SessionError::Busy)));
    assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests(), [Request::Discover, Request::Unload]);
}

#[test]
fn canceled_cleanup_remains_closing_until_explicit_disconnect_finishes() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 1), Step::Pending, clear()]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
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
    assert!(matches!(run(session.connect()), Err(SessionError::Busy)));
    assert!(matches!(run(session.refresh()), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests().len(), 3);
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
        let mut session = fixture.session(&wire);
        session.enable();
        match run(session.connect()).unwrap_err() {
            SessionError::Helper { code: actual, may_have_run, .. } => {
                assert_eq!(actual, code);
                assert_eq!(may_have_run, code == ResponseCode::Failed);
            }
            error => panic!("unexpected session error: {error}"),
        }
        assert_eq!(session.state(), SessionState::Disconnected);
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
        let mut session = fixture.session(&wire);
        session.enable();
        assert!(matches!(run(session.connect()), Err(SessionError::Transport(error)) if error.delivery == delivery));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests().len(), 2);
    }
}

#[test]
fn malformed_status_does_not_become_ready_or_trigger_a_replay() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), done(vec![0; 15])]);
    let mut session = fixture.session(&wire);
    session.enable();
    assert!(matches!(run(session.connect()), Err(SessionError::Protocol(ProtocolError::InvalidData))));
    assert_eq!(session.state(), SessionState::Disconnected);
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
    let mut session = fixture.session(&wire);
    assert!(matches!(run(session.read_forge()), Err(SessionError::Disabled)));
    session.enable();
    assert!(matches!(run(session.read_forge()), Err(SessionError::NotConnected)));
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
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    let initial_status = *session.runtime_status().unwrap();
    let observed = run(session.read_forge()).unwrap().unwrap();
    assert_eq!(observed, expected);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.runtime_status(), Some(&initial_status));
    assert_eq!(run(session.read_forge()).unwrap(), None);
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
        let fixture = Fixture::rehashing();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        fs::write(if change_binary { &fixture.binary } else { &fixture.metadata }, b"changed before forge read").unwrap();
        assert!(matches!(run(session.read_forge()), Err(SessionError::Identity(_))));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
        assert_eq!(wire.requests(), [Request::Discover, Request::Status { dispatcher: fixture.dispatcher }, Request::Unload]);
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
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        let error = run(session.read_forge()).unwrap_err();
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
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        session.enable();
        assert!(matches!(run(session.read_forge()), Err(SessionError::NotConnected)));
        assert!(matches!(run(session.refresh()), Err(SessionError::NotConnected)));
        assert_eq!(wire.requests().len(), 3);
        wire.push(fixture.discover());
        wire.push(status(false, 0x5678, 0));
        run(session.connect()).unwrap();
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
        assert_eq!(run(session.read_forge()).unwrap(), None);
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
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert!(matches!(
        run(session.read_forge()),
        Err(SessionError::Helper {
            code: ResponseCode::OfflineRequired,
            may_have_run: false,
            ..
        })
    ));
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    session.enable();
    assert!(matches!(run(session.read_forge()), Err(SessionError::NotConnected)));
    assert!(matches!(run(session.refresh()), Err(SessionError::NotConnected)));
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(wire.requests().len(), 3);
    wire.push(fixture.discover());
    wire.push(status(false, 0x5678, 0));
    run(session.connect()).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 5);
}

#[test]
fn canceled_forge_read_remains_busy_until_explicit_disconnect_without_replay() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), Step::Pending, clear()]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    let mut reading = Box::pin(session.read_forge());
    assert!(matches!(reading.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    drop(reading);
    assert_eq!(session.state(), SessionState::Connecting);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
    session.enable();
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(matches!(run(session.read_forge()), Err(SessionError::Busy)));
    assert!(matches!(run(session.connect()), Err(SessionError::Busy)));
    assert!(matches!(run(session.refresh()), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(
        wire.requests(),
        [
            Request::Discover,
            Request::Status { dispatcher: fixture.dispatcher },
            Request::ReadForge { dispatcher: fixture.dispatcher },
            Request::Unload
        ]
    );
    wire.push(fixture.discover());
    wire.push(status(false, 0x1234, 0));
    run(session.connect()).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 6);
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
    let mut session = fixture.session(&wire);
    assert!(matches!(run(session.read_locale()), Err(SessionError::Disabled)));
    session.enable();
    assert!(matches!(run(session.read_locale()), Err(SessionError::NotConnected)));
    assert!(wire.requests().is_empty());
    for step in [
        fixture.discover(),
        status(false, 0x1234, 0),
        done(locale::encode(&first).unwrap()),
        done(locale::encode(&changed).unwrap()),
    ] {
        wire.push(step);
    }
    run(session.connect()).unwrap();
    let initial = run(session.read_locale()).unwrap();
    assert_eq!(initial, first);
    assert_eq!(run(session.read_locale()).unwrap(), changed);
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
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        assert!(run(session.read_locale()).is_err());
        assert_eq!(session.runtime_status(), None);
        assert_eq!(session.cleanup(), Cleanup::Unconfirmed);
        assert!(matches!(run(session.read_locale()), Err(SessionError::NotConnected)));
        assert!(matches!(run(session.read_forge()), Err(SessionError::NotConnected)));
        assert_eq!(wire.requests().len(), 3);
        let expected = GameLocale {
            language: "ja-JP".into(),
            game_version: String::new(),
        };
        for step in [fixture.discover(), status(false, 0x1234, 0), done(locale::encode(&expected).unwrap())] {
            wire.push(step);
        }
        run(session.connect()).unwrap();
        assert_eq!(run(session.read_locale()).unwrap(), expected);
        assert_eq!(wire.requests().len(), 6);
    }
}

#[test]
fn unload_ack_without_native_departure_evidence_stays_unconfirmed_and_is_not_replayed() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), clear()]);
    wire.0.lock().unwrap().departure_proven = false;
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
    assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
    assert_eq!(wire.requests().last(), Some(&Request::Unload));
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn typed_travel_response_keeps_the_lease_but_removes_old_manager_authority() {
    let fixture = Fixture::new();
    let loading = Step::Reply(Ok(Response {
        code: ResponseCode::CharacterLoading,
        payload: b"changing area".to_vec(),
    }));
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [fixture.discover(), status(false, 0x1234, 0), loading, status(false, 0x5678, 0), clear()],
    );
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert!(matches!(run(session.refresh()), Err(SessionError::CharacterLoading)));
    assert_eq!(session.state(), SessionState::Ready);
    assert!(session.runtime_status().is_none());
    assert!(session.artifact_bytes_read().is_some());
    run(session.refresh()).unwrap();
    assert_eq!(session.runtime_status().unwrap().manager, 0x5678);
    assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
    assert_eq!(wire.requests().iter().filter(|request| **request == Request::Discover).count(), 1);
}
