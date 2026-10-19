use super::*;
use eg_game_access::mapping::{self, ImplicitDescriptionRequest, ModifierSnapshot};
fn line(index: u32, rolls: Vec<u8>) -> ModifierSnapshot {
    ModifierSnapshot {
        index,
        text: "Armour Ω".into(),
        range: "[1 to 9]".into(),
        roll_ids: rolls,
        editable: true,
    }
}
#[test]
fn implicit_descriptions_bind_complete_expected_item_and_keep_fixed_lines_after_roll_storage() {
    let fixture = Fixture::new();
    let item = forge_item();
    let lines = vec![line(1, vec![1]), line(3, vec![])];
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [fixture.discover(), status(false, 0x1234, 0), done(mapping::encode_modifiers(&lines).unwrap())],
    );
    let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert_eq!(run(session.describe_implicits(now(), &item)).unwrap(), Some(lines));
    assert_eq!(
        wire.requests()[2],
        Request::DescribeImplicits(ImplicitDescriptionRequest::new(fixture.dispatcher, 0x1234, item).unwrap())
    );
    assert_eq!(session.state(), SessionState::Ready);
}
#[test]
fn optional_formatting_refusal_preserves_connection_and_later_forge_reads() {
    for code in [ResponseCode::Failed, ResponseCode::Refused, ResponseCode::StaleItem, ResponseCode::NotReady] {
        let fixture = Fixture::new();
        let item = forge_item();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [
                fixture.discover(),
                status(false, 0x1234, 0),
                Step::Reply(Ok(Response {
                    code,
                    payload: b"formatter unavailable".to_vec(),
                })),
                done(forge::encode(Some(&item)).unwrap()),
            ],
        );
        let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert_eq!(run(session.describe_implicits(now(), &item)).unwrap(), None);
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(run(session.read_forge(now())).unwrap(), Some(item));
        assert_eq!(wire.requests().len(), 4);
    }
}
#[test]
fn malformed_or_cross_slot_implicit_mapping_revokes_connection_without_retry() {
    for lines in [vec![line(1, vec![0])], vec![line(3, vec![3])], vec![line(8, vec![])], vec![line(0, vec![0, 1])]] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [fixture.discover(), status(false, 0x1234, 0), done(mapping::encode_modifiers(&lines).unwrap())],
        );
        let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(matches!(run(session.describe_implicits(now(), &forge_item())), Err(SessionError::Protocol(_))));
        assert_eq!(session.state(), SessionState::Blocked);
        assert_eq!(wire.requests().len(), 3);
    }
}
#[test]
fn optional_description_offline_or_transport_failure_still_revokes_admission() {
    for step in [
        Step::Reply(Ok(Response {
            code: ResponseCode::OfflineRequired,
            payload: vec![],
        })),
        Step::Reply(Err(TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Timeout,
        })),
        done(vec![1]),
    ] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), step]);
        let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(run(session.describe_implicits(now(), &forge_item())).is_err());
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(session.runtime_status(), None);
        assert_eq!(wire.requests().len(), 3);
    }
}
#[test]
fn conditional_description_frame_rejects_reserved_bytes_truncation_and_excess() {
    let fixture = Fixture::new();
    let request = Request::DescribeImplicits(ImplicitDescriptionRequest::new(fixture.dispatcher, 0x1234, forge_item()).unwrap());
    let frame = request.encode();
    assert_eq!(Request::decode(&frame).unwrap(), request);
    for offset in [8, 24, 36, 47, 72, 80, 335] {
        let mut bad = frame.clone();
        bad[offset] = 1;
        assert!(Request::decode(&bad).is_err());
    }
    assert!(Request::decode(&frame[..frame.len() - 1]).is_err());
    let mut bad = frame;
    bad.push(0);
    assert!(Request::decode(&bad).is_err());
}

#[test]
fn optional_formatter_failure_cannot_restore_admission_after_log_gate_loss() {
    let fixture = Fixture::new();
    let log = FakeLog::new([offline()]);
    let changed = log.clone();
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            Step::Action(
                Box::new(move || changed.set(online())),
                Response {
                    code: ResponseCode::Failed,
                    payload: b"formatter threw".to_vec(),
                },
            ),
        ],
    );
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert!(matches!(run(session.describe_implicits(now(), &forge_item())), Err(SessionError::Gate(_))));
    assert_ne!(session.state(), SessionState::Ready);
    assert_eq!(session.runtime_status(), None);
    assert_eq!(wire.requests().len(), 3);
}
#[test]
fn canceled_description_does_not_replay_or_leave_ready() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), Step::Pending]);
    let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let expected = forge_item();
    let mut query = Box::pin(session.describe_implicits(now(), &expected));
    assert!(query.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(query);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(run(session.describe_implicits(now(), &expected)).is_err());
    assert_eq!(wire.requests().len(), 3);
}
