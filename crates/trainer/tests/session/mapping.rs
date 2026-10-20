use super::*;
use eg_trainer::{edits::ExpectedObservation, mapping::MappingRequest};
use eg_trainer_wire::mapping::{self, MappingAddress, MappingStart, MappingTarget, RawMappingPage, RawValueSample};

fn expected(fixture: &Fixture) -> ExpectedObservation {
    let profile = fixture.profile();
    ExpectedObservation::new(
        1,
        fixture.target.clone(),
        eg_trainer::catalog::CatalogIdentity {
            binary_hash: profile.binary_hash().into(),
            metadata_hash: profile.metadata_hash().into(),
            locale: GameLocale {
                language: "en".into(),
                game_version: "authored".into(),
            },
            extraction_version: eg_trainer::catalog::EXTRACTION_VERSION,
        },
        forge_item(),
    )
    .unwrap()
}
fn request() -> MappingRequest {
    MappingRequest {
        address: MappingAddress {
            target: MappingTarget::Affix,
            index: 0,
            line: None,
        },
        draft: None,
    }
}
fn page(start: MappingStart) -> RawMappingPage {
    RawMappingPage {
        language: "en".into(),
        context: b"detached game context".to_vec(),
        address: request().address,
        start,
        samples: match start {
            MappingStart::ContextOnly => vec![],
            MappingStart::Samples(start) => (u16::from(start)..u16::from(start) + 16)
                .map(|roll| RawValueSample {
                    raw_quantity: f64::from(roll) / 255.,
                    display: format!("{}%", 1000 + roll),
                    unit: "%".into(),
                    effects: vec![format!("Game text {roll}")],
                })
                .collect(),
        },
    }
}
fn pages() -> Vec<Step> {
    std::iter::once(MappingStart::ContextOnly)
        .chain((0..=240).step_by(16).map(MappingStart::Samples))
        .map(|start| done(mapping::encode_mapping(&page(start)).unwrap()))
        .collect()
}
fn ready(fixture: &Fixture, wire: &FakeTransport, log: &FakeLog) -> Session<FakeTransport, FakeLog> {
    let mut session = fixture.session(wire, log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    session
}
#[test]
fn complete_sampled_mapping_uses_exact_display_and_full_conditional_queries() {
    let fixture = Fixture::new();
    let observation = expected(&fixture);
    let mut steps = vec![fixture.discover(), status(false, 0x1234, 0)];
    steps.extend(pages());
    let wire = FakeTransport::new(fixture.target.clone(), steps);
    let mut session = ready(&fixture, &wire, &FakeLog::new([offline()]));
    let table = run(session.resolve_mapping(now(), &observation, &request())).unwrap();
    assert_eq!(table.samples().len(), 256);
    assert_eq!(table.sample(99).value, 1099.);
    assert_eq!(table.sample(99).raw_quantity, 99. / 255.);
    assert_eq!(table.sample(99).display, "1099%");
    assert_eq!(table.sample(99).effects, ["Game text 99"]);
    assert_eq!(table.nearest(1111.1, 0), Ok(111));
    for (index, request) in wire.requests()[2..].iter().enumerate() {
        assert!(matches!(request, Request::MapRoll(query)
            if query.expected() == observation.snapshot()
            && query.manager() == 0x1234
            && query.slot() == 0
            && query.start() == if index == 0 { MappingStart::ContextOnly } else { MappingStart::Samples(((index - 1) * 16) as u8) }));
    }
    assert_eq!(session.state(), SessionState::Ready);
    assert!(session.last_write().is_none());
}
#[test]
fn draft_mapping_changes_only_detached_query_and_retains_original_observation() {
    use eg_trainer::edits::{AffixEdit, ItemEdit, SealState};
    let fixture = Fixture::new();
    let mut request = request();
    request.draft = Some(ItemEdit::Affix(AffixEdit {
        slot: 0,
        id: 123,
        tier: 4,
        roll: 81,
        seal: SealState::Sealed,
        allow_incompatible: false,
    }));
    let observation = expected(&fixture);
    let mut steps = vec![fixture.discover(), status(false, 0x1234, 0)];
    steps.extend(pages());
    let wire = FakeTransport::new(fixture.target.clone(), steps);
    let mut session = ready(&fixture, &wire, &FakeLog::new([offline()]));
    let table = run(session.resolve_mapping(now(), &observation, &request)).unwrap();
    table.recheck(&observation, &request).unwrap();
    for request in &wire.requests()[2..] {
        assert!(
            matches!(request, Request::MapRoll(q) if q.expected() == observation.snapshot() && q.draft().is_some_and(|d| d.id == 123 && d.tier == 4 && d.roll == 81 && d.seal == 1))
        );
    }
}
#[test]
fn mapping_display_context_and_optional_helper_failures_stay_local() {
    for fault in 0..7 {
        let fixture = Fixture::new();
        let observation = expected(&fixture);
        let mut steps = vec![fixture.discover(), status(false, 0x1234, 0)];
        match fault {
            0..=3 => steps.push(Step::Reply(Ok(Response {
                code: [ResponseCode::Refused, ResponseCode::Failed, ResponseCode::NotReady, ResponseCode::StaleItem][fault],
                payload: b"optional mapping unavailable".to_vec(),
            }))),
            _ => {
                steps.push(done(mapping::encode_mapping(&page(MappingStart::ContextOnly)).unwrap()));
                let mut sample = page(MappingStart::Samples(0));
                match fault {
                    4 => sample.context.push(1),
                    5 => sample.samples[0].display = "ambiguous 12 and 17%".into(),
                    6 => sample.language = "fr".into(),
                    _ => unreachable!(),
                }
                steps.push(done(mapping::encode_mapping(&sample).unwrap()));
            }
        }
        steps.push(done(forge::encode(Some(observation.snapshot())).unwrap()));
        let wire = FakeTransport::new(fixture.target.clone(), steps);
        let mut session = ready(&fixture, &wire, &FakeLog::new([offline()]));
        let failure = run(session.resolve_mapping(now(), &observation, &request()));
        if matches!(fault, 1..=4 | 6) {
            assert!(matches!(failure, Err(SessionError::MappingRetryable(_))), "{fault}");
        } else {
            assert!(matches!(failure, Err(SessionError::Mapping(_))), "{fault}");
        }
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(run(session.read_forge(now())).unwrap().as_ref(), Some(observation.snapshot()));
    }
}
#[test]
fn mapping_offline_transport_and_malformed_wire_failures_revoke_without_replay() {
    for failure in 0..4 {
        let fixture = Fixture::new();
        let log = FakeLog::new([offline()]);
        let changed = log.clone();
        let last = match failure {
            0 => Step::Reply(Ok(Response {
                code: ResponseCode::OfflineRequired,
                payload: vec![],
            })),
            1 => Step::Reply(Err(TransportError {
                delivery: Delivery::MayHaveRun,
                failure: TransportFailure::Timeout,
            })),
            2 => done(vec![0xff]),
            3 => Step::Action(
                Box::new(move || changed.set(online())),
                Response {
                    code: ResponseCode::Refused,
                    payload: b"optional failure".to_vec(),
                },
            ),
            _ => unreachable!(),
        };
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                status(false, 0x1234, 0),
                done(mapping::encode_mapping(&page(MappingStart::ContextOnly)).unwrap()),
                last,
            ],
        );
        let mut session = ready(&fixture, &wire, &log);
        let observation = expected(&fixture);
        assert!(run(session.resolve_mapping(now(), &observation, &request())).is_err());
        assert_ne!(session.state(), SessionState::Ready);
        assert!(session.runtime_status().is_none());
        assert!(run(session.resolve_mapping(now(), &observation, &request())).is_err());
        assert_eq!(wire.requests().len(), 4);
    }
}
#[test]
fn canceled_mapping_never_replays_and_artifact_change_between_pages_blocks() {
    for cancel in [true, false] {
        let fixture = Fixture::new();
        let final_step = if cancel {
            Step::Pending
        } else {
            let path = fixture.metadata.clone();
            Step::Action(
                Box::new(move || fs::write(&path, b"changed during mapping").unwrap()),
                Response {
                    code: ResponseCode::Done,
                    payload: mapping::encode_mapping(&page(MappingStart::Samples(0))).unwrap(),
                },
            )
        };
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                status(false, 0x1234, 0),
                done(mapping::encode_mapping(&page(MappingStart::ContextOnly)).unwrap()),
                final_step,
            ],
        );
        let mut session = ready(&fixture, &wire, &FakeLog::new([offline()]));
        #[cfg(windows)]
        let _writer = if !cancel {
            // Exercise the supported rehash fallback rather than attempting to
            // write through the Windows read-only artifact lease.
            use std::os::windows::fs::OpenOptionsExt;
            Some(fs::OpenOptions::new().read(true).write(true).share_mode(7).open(&fixture.metadata).unwrap())
        } else {
            None
        };
        let observation = expected(&fixture);
        let request = request();
        if cancel {
            let mut operation = Box::pin(session.resolve_mapping(now(), &observation, &request));
            assert!(operation.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            drop(operation);
            assert_eq!(session.state(), SessionState::Connecting);
        } else {
            assert!(matches!(run(session.resolve_mapping(now(), &observation, &request)), Err(SessionError::Identity(_))));
            assert_eq!(session.state(), SessionState::Blocked);
        }
        assert!(run(session.resolve_mapping(now(), &observation, &request)).is_err());
        assert_eq!(wire.requests().len(), 4);
    }
}

#[test]
fn obsolete_mapping_stops_between_pages_and_keeps_the_drained_session_ready() {
    use eg_trainer::session::{CaptureControl, CaptureObserver};
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Stop(Arc<AtomicBool>);
    impl CaptureObserver for Stop {
        fn control(&mut self) -> CaptureControl {
            if self.0.load(Ordering::SeqCst) {
                CaptureControl::Stop
            } else {
                CaptureControl::Continue
            }
        }
    }
    let fixture = Fixture::new();
    let canceled = Arc::new(AtomicBool::new(false));
    let stop = canceled.clone();
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            done(mapping::encode_mapping(&page(MappingStart::ContextOnly)).unwrap()),
            Step::Action(
                Box::new(move || stop.store(true, Ordering::SeqCst)),
                Response {
                    code: ResponseCode::Done,
                    payload: mapping::encode_mapping(&page(MappingStart::Samples(0))).unwrap(),
                },
            ),
            done(forge::encode(Some(expected(&fixture).snapshot())).unwrap()),
        ],
    );
    let mut session = ready(&fixture, &wire, &FakeLog::new([offline()]));
    let mut observer = Stop(canceled);
    let result = run(session.resolve_mapping_observed(now(), &expected(&fixture), &request(), &mut observer));
    assert!(matches!(result, Err(SessionError::Busy)));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 4, "the next fifteen pages were not submitted");
    assert!(session.last_write().is_none());
    assert!(run(session.read_forge(now())).unwrap().is_some());
    let count = wire.requests().len();
    assert!(run(session.resolve_mapping_observed(now(), &expected(&fixture), &request(), &mut observer)).is_err());
    assert_eq!(wire.requests().len(), count, "already-canceled reads do no I/O");
}
