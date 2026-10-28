use super::*;
use eg_trainer::{item::edit::ExpectedObservation, item::values::MappingRequest};
use eg_trainer_wire::mapping::{self, MappingAddress, MappingStart, MappingTarget, RawMappingPage, RawValueSample};

fn expected(fixture: &Fixture) -> ExpectedObservation {
    let profile = fixture.profile();
    ExpectedObservation::new(
        1,
        fixture.target.clone(),
        eg_trainer::definitions::catalog::CatalogIdentity {
            binary_hash: profile.binary_hash().into(),
            metadata_hash: profile.metadata_hash().into(),
            locale: GameLocale {
                language: "en".into(),
                game_version: "authored".into(),
            },
            extraction_version: eg_trainer::definitions::catalog::EXTRACTION_VERSION,
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
fn ready(fixture: &Fixture, wire: &FakeTransport) -> Session<FakeTransport> {
    let mut session = fixture.session(wire);
    session.enable();
    run(session.connect()).unwrap();
    session
}
#[test]
fn complete_sampled_mapping_uses_exact_display_and_full_conditional_queries() {
    let fixture = Fixture::new();
    let observation = expected(&fixture);
    let mut steps = vec![fixture.discover(), status(false, 0x1234, 0)];
    steps.extend(pages());
    let wire = FakeTransport::new(fixture.target.clone(), steps);
    let mut session = ready(&fixture, &wire);
    let bytes_read = session.artifact_bytes_read();
    let table = run(session.resolve_mapping(&observation, &request())).unwrap();
    assert_eq!(session.artifact_bytes_read(), bytes_read);
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
}
#[test]
fn draft_mapping_changes_only_detached_query_and_retains_original_observation() {
    use eg_trainer::item::edit::{AffixEdit, ItemEdit, SealState};
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
    let mut session = ready(&fixture, &wire);
    let table = run(session.resolve_mapping(&observation, &request)).unwrap();
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
        let mut session = ready(&fixture, &wire);
        let failure = run(session.resolve_mapping(&observation, &request()));
        if matches!(fault, 1..=4 | 6) {
            assert!(matches!(failure, Err(SessionError::MappingRetryable(_))), "{fault}");
        } else {
            assert!(matches!(failure, Err(SessionError::Mapping(_))), "{fault}");
        }
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(run(session.read_forge()).unwrap().as_ref(), Some(observation.snapshot()));
    }
}
#[test]
fn mapping_offline_transport_and_malformed_wire_failures_revoke_without_replay() {
    for failure in 0..3 {
        let fixture = Fixture::new();
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
        let mut session = ready(&fixture, &wire);
        let observation = expected(&fixture);
        assert!(run(session.resolve_mapping(&observation, &request())).is_err());
        assert_ne!(session.state(), SessionState::Ready);
        assert!(session.runtime_status().is_none());
        assert!(run(session.resolve_mapping(&observation, &request())).is_err());
        assert_eq!(wire.requests().len(), 4);
    }
}
#[test]
fn canceled_mapping_never_replays_and_artifact_change_between_pages_blocks() {
    for cancel in [true, false] {
        let fixture = Fixture::rehashing();
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
        let mut session = ready(&fixture, &wire);
        let observation = expected(&fixture);
        let request = request();
        if cancel {
            let mut operation = Box::pin(session.resolve_mapping(&observation, &request));
            assert!(operation.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            drop(operation);
            assert_eq!(session.state(), SessionState::Connecting);
        } else {
            assert!(matches!(run(session.resolve_mapping(&observation, &request)), Err(SessionError::Identity(_))));
            assert_eq!(session.state(), SessionState::Disconnected);
        }
        assert!(run(session.resolve_mapping(&observation, &request)).is_err());
        assert_eq!(wire.requests().len(), 4);
    }
}

#[test]
fn obsolete_mapping_stops_between_pages_and_keeps_the_drained_session_ready() {
    use eg_trainer::session::{ReadControl, ReadObserver};
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Stop(Arc<AtomicBool>);
    impl ReadObserver for Stop {
        fn control(&mut self) -> ReadControl {
            if self.0.load(Ordering::SeqCst) { ReadControl::Stop } else { ReadControl::Continue }
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
    let mut session = ready(&fixture, &wire);
    let mut observer = Stop(canceled);
    let result = run(session.resolve_mapping_observed(&expected(&fixture), &request(), None, &mut observer));
    assert!(matches!(result, Err(SessionError::Busy)));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 4, "the next fifteen pages were not submitted");
    assert!(run(session.read_forge()).unwrap().is_some());
    let count = wire.requests().len();
    assert!(run(session.resolve_mapping_observed(&expected(&fixture), &request(), None, &mut observer)).is_err());
    assert_eq!(wire.requests().len(), count, "already-canceled reads do no I/O");
}

/// Stops from its `stop_at`th control check on. A lookup checks on entry, then for each page before and after the
/// artifact check (the second immediately before the request) and after the reply.
struct StopAt {
    calls: usize,
    stop_at: usize,
}
impl eg_trainer::session::ReadObserver for StopAt {
    fn control(&mut self) -> eg_trainer::session::ReadControl {
        self.calls += 1;
        if self.calls >= self.stop_at {
            eg_trainer::session::ReadControl::Stop
        } else {
            eg_trainer::session::ReadControl::Continue
        }
    }
}

#[test]
fn a_stop_at_each_check_around_the_first_page_keeps_the_session_and_its_files_for_cleanup() {
    // On entry, before the artifact check, after it (immediately before the request): nothing is sent. After the
    // reply: the page was received, and nothing more is sent.
    for (stop_at, sent) in [(1, 2), (2, 2), (3, 2), (4, 3)] {
        let fixture = Fixture::new();
        let mut steps = vec![fixture.discover(), status(false, 0x1234, 0)];
        if sent == 3 {
            steps.push(done(mapping::encode_mapping(&page(MappingStart::ContextOnly)).unwrap()));
        }
        steps.push(clear());
        let wire = FakeTransport::new(fixture.target.clone(), steps);
        let mut session = ready(&fixture, &wire);
        let mut observer = StopAt { calls: 0, stop_at };
        let result = run(session.resolve_mapping_observed(&expected(&fixture), &request(), None, &mut observer));
        assert!(matches!(result, Err(SessionError::Busy)), "stop at {stop_at}: {result:?}");
        assert_eq!(session.state(), SessionState::Ready);
        assert!(session.runtime_status().is_some());
        assert_eq!(wire.requests().len(), sent, "stop at {stop_at}");
        assert!(fs::OpenOptions::new().write(true).open(&fixture.metadata).is_err(), "the session still holds its files");
        assert_eq!(run(session.disconnect()), Cleanup::ConfirmedOff);
        assert_eq!(wire.requests()[sent], Request::Unload);
        assert!(fs::OpenOptions::new().write(true).open(&fixture.metadata).is_ok());
    }
}

#[test]
fn an_observer_stop_does_not_turn_a_runtime_refusal_into_a_cancellation() {
    let fixture = Fixture::new();
    let refusal = Step::Reply(Ok(Response {
        code: ResponseCode::OfflineRequired,
        payload: b"offline character required".to_vec(),
    }));
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), refusal]);
    let mut session = ready(&fixture, &wire);
    // It would stop once the reply is in: the refusal must still be reported as one.
    let mut observer = StopAt { calls: 0, stop_at: 4 };
    assert!(matches!(
        run(session.resolve_mapping_observed(&expected(&fixture), &request(), None, &mut observer)),
        Err(SessionError::Helper {
            code: ResponseCode::OfflineRequired,
            ..
        })
    ));
    assert_eq!(session.state(), SessionState::Disconnected);
    assert_eq!(run(session.disconnect()), Cleanup::Unconfirmed);
    assert_eq!(wire.requests().len(), 4, "only owner Unload follows refusal");
}
