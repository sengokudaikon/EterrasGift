use super::*;
use eg_game_access::fields::{self, AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate};

fn available(manager: u64) -> AttackLeechRead {
    AttackLeechRead::Available(AttackLeechObservation {
        manager: ObservedObjectIdentity::new(manager, 0x2000).unwrap(),
        actor: ObservedObjectIdentity::new(0x3000, 0x4000).unwrap(),
        stats: ObservedObjectIdentity::new(0x5000, 0x6000).unwrap(),
        attack_rate: ObservedRate { raw_bits: 0x7fc01234 },
        increased_leech_rate: ObservedRate { raw_bits: (-0.0_f32).to_bits() },
    })
}

#[test]
fn attack_leech_read_binds_admitted_manager_and_preserves_owned_float_bits() {
    let fixture = Fixture::new();
    let expected = available(0x1234);
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(fields::encode(&expected))]);
    let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert_eq!(run(session.read_attack_leech(now())).unwrap(), expected);
    assert_eq!(
        wire.requests()[2],
        Request::ReadAttackLeech {
            dispatcher: fixture.dispatcher,
            expected_manager: 0x1234
        }
    );
    assert_eq!(session.state(), SessionState::Ready);
}

#[test]
fn attack_leech_global_refusals_revoke_admission_but_local_graph_reasons_remain_typed_and_ready() {
    for reason in [
        AttackLeechUnavailable::OfflineRequired,
        AttackLeechUnavailable::ManagerAbsent,
        AttackLeechUnavailable::ManagerChanged,
        AttackLeechUnavailable::ActorCacheAbsent,
        AttackLeechUnavailable::StatsAbsent,
        AttackLeechUnavailable::NativeObjectDestroyed,
        AttackLeechUnavailable::GraphChanged,
        AttackLeechUnavailable::UnsupportedStatsClass,
    ] {
        let fixture = Fixture::new();
        let expected = AttackLeechRead::Unavailable(reason);
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(fields::encode(&expected))]);
        let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert_eq!(run(session.read_attack_leech(now())).unwrap(), expected);
        let global = matches!(
            reason,
            AttackLeechUnavailable::OfflineRequired | AttackLeechUnavailable::ManagerAbsent | AttackLeechUnavailable::ManagerChanged
        );
        assert_eq!(session.state(), if global { SessionState::WaitingForCharacter } else { SessionState::Ready });
        assert_eq!(session.runtime_status().is_none(), global);
        if global {
            assert!(matches!(run(session.read_attack_leech(now())), Err(SessionError::NotConnected)));
        }
        assert_eq!(wire.requests().len(), 3);
    }
}

#[test]
fn attack_leech_rejects_malformed_or_wrong_manager_without_retry() {
    for payload in [vec![0xff], fields::encode(&available(0x9999))] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(payload)]);
        let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(matches!(run(session.read_attack_leech(now())), Err(SessionError::Protocol(_))));
        assert_eq!(session.state(), SessionState::Blocked);
        assert!(session.runtime_status().is_none());
        assert!(run(session.read_attack_leech(now())).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
}

#[test]
fn attack_leech_rechecks_log_and_artifacts_after_reply_and_artifacts_before_io() {
    for fault in 0..3 {
        let fixture = Fixture::new();
        let log = FakeLog::new([offline()]);
        let changed_log = log.clone();
        let changed_metadata = fixture.metadata.clone();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        wire.push(Step::Action(
            Box::new(move || {
                if fault == 0 {
                    changed_log.set(online());
                } else {
                    fs::write(&changed_metadata, b"changed field profile").unwrap();
                }
            }),
            Response {
                code: ResponseCode::Done,
                payload: fields::encode(&available(0x1234)),
            },
        ));
        if fault == 2 {
            fs::write(&fixture.metadata, b"changed before read").unwrap();
        }
        let error = run(session.read_attack_leech(now())).unwrap_err();
        assert!(matches!(error, SessionError::Gate(_) | SessionError::Identity(_)));
        assert!(session.runtime_status().is_none());
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(wire.requests().len(), if fault == 2 { 2 } else { 3 });
    }
}

#[test]
fn attack_leech_dropped_future_stays_busy_and_never_replays() {
    let fixture = Fixture::new();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), Step::Pending]);
    let mut session = fixture.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    let mut read = Box::pin(session.read_attack_leech(now()));
    assert!(read.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(read);
    assert_eq!(session.state(), SessionState::Connecting);
    assert!(session.runtime_status().is_none());
    assert!(matches!(run(session.read_attack_leech(now())), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
}
