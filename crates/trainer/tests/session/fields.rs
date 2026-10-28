use super::*;
use eg_trainer::{
    backend::AttackLeechSnapshot,
    fields::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, RateWriteStatus},
    write::{WriteObservation, WriteOutcome},
};
use eg_trainer_wire::fields::{self, AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate};

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
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert_eq!(run(session.read_attack_leech()).unwrap(), expected);
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
fn attack_leech_rejects_malformed_or_wrong_manager_without_retry() {
    for payload in [vec![0xff], fields::encode(&available(0x9999))] {
        let fixture = Fixture::new();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(payload)]);
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        assert!(matches!(run(session.read_attack_leech()), Err(SessionError::Protocol(_))));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert!(session.runtime_status().is_none());
        assert!(run(session.read_attack_leech()).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
}
fn observation() -> AttackLeechObservation {
    AttackLeechObservation {
        manager: ObservedObjectIdentity::new(0x1234, 0x2000).unwrap(),
        actor: ObservedObjectIdentity::new(0x3000, 0x4000).unwrap(),
        stats: ObservedObjectIdentity::new(0x5000, 0x6000).unwrap(),
        attack_rate: ObservedRate { raw_bits: 0x7fc01234 },
        increased_leech_rate: ObservedRate { raw_bits: 0x80000000 },
    }
}
fn plan(f: &Fixture, target: AttackLeechTarget, value: f64) -> AttackLeechPlan {
    let expected = AttackLeechSnapshot {
        process: f.target.clone(),
        generation: 37,
        sequence: 19,
        manager: 0x1234,
        read: AttackLeechRead::Available(observation()),
    };
    match AttackLeechPlan::prepare(&expected, target, value).unwrap() {
        AttackLeechDecision::Change(plan) => plan,
        AttackLeechDecision::NoOp => panic!("test needs a change"),
    }
}
fn ack(plan: &AttackLeechPlan) -> Step {
    done(fields::encode(&AttackLeechRead::Available(*plan.projected())))
}
fn ready(f: &Fixture, wire: &FakeTransport) -> Session<FakeTransport> {
    let mut session = f.session(wire);
    session.enable();
    run(session.connect()).unwrap();
    session
}
fn mutate(o: &mut AttackLeechObservation, part: usize) {
    let new = |v: ObservedObjectIdentity, native: bool| ObservedObjectIdentity::new(v.wrapper_token() ^ u64::from(!native), v.native_token() ^ u64::from(native)).unwrap();
    match part {
        0 => o.manager = new(o.manager, false),
        1 => o.manager = new(o.manager, true),
        2 => o.actor = new(o.actor, false),
        3 => o.actor = new(o.actor, true),
        4 => o.stats = new(o.stats, false),
        5 => o.stats = new(o.stats, true),
        6 => o.attack_rate.raw_bits ^= 1,
        7 => o.increased_leech_rate.raw_bits ^= 1,
        _ => unreachable!(),
    }
}
#[test]
fn explicit_rate_change_needs_two_exact_full_observations_and_preserves_bits() {
    for (target, value) in [
        (AttackLeechTarget::AttackRate, 1.1),
        (AttackLeechTarget::AttackRate, 0.01),
        (AttackLeechTarget::IncreasedLeechRate, 0.0),
    ] {
        let f = Fixture::new();
        let p = plan(&f, target, value);
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), ack(&p), ack(&p), clear()]);
        let mut session = ready(&f, &wire);
        assert_eq!(
            run(session.write(&p.clone().into())).unwrap(),
            WriteOutcome::Confirmed(WriteObservation::Field(*p.projected()))
        );
        assert_eq!(session.state(), SessionState::Ready);
        assert!(matches!(&wire.requests()[2],Request::SetAttackLeechRate(w) if *w.expected()==observation() && w.projected()==*p.projected() && w.expected_manager()==0x1234));
        assert_eq!(
            wire.requests()[3],
            Request::ReadAttackLeech {
                dispatcher: f.dispatcher,
                expected_manager: 0x1234
            }
        );
        run(session.disconnect());
        assert_eq!(wire.requests().len(), 5);
    }
}
#[test]
fn independent_rate_drift_is_known_but_changed_identity_or_failed_reads_are_uncertain() {
    for fault in 0..24 {
        let f = Fixture::new();
        let p = plan(&f, AttackLeechTarget::AttackRate, 1.1);
        let mut actual = *p.projected();
        let last = match fault {
            0 => done(vec![1]),
            1..=8 => {
                mutate(&mut actual, fault - 1);
                done(fields::encode(&AttackLeechRead::Available(actual)))
            }
            9..=16 => done(fields::encode(&AttackLeechRead::Unavailable(
                [
                    AttackLeechUnavailable::OfflineRequired,
                    AttackLeechUnavailable::ManagerAbsent,
                    AttackLeechUnavailable::ManagerChanged,
                    AttackLeechUnavailable::ActorCacheAbsent,
                    AttackLeechUnavailable::StatsAbsent,
                    AttackLeechUnavailable::NativeObjectDestroyed,
                    AttackLeechUnavailable::GraphChanged,
                    AttackLeechUnavailable::UnsupportedStatsClass,
                ][fault - 9],
            ))),
            17 | 18 => Step::Reply(Err(TransportError {
                delivery: if fault == 17 { Delivery::NotSent } else { Delivery::MayHaveRun },
                failure: TransportFailure::Timeout,
            })),
            _ => Step::Reply(Ok(Response {
                code: [
                    ResponseCode::Failed,
                    ResponseCode::NotReady,
                    ResponseCode::Refused,
                    ResponseCode::OfflineRequired,
                    ResponseCode::RestartRequired,
                ][fault - 19],
                payload: b"read refused after store".to_vec(),
            })),
        };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), ack(&p), last]);
        let mut session = ready(&f, &wire);
        let result = run(session.set_attack_leech_rate(&p)).unwrap();
        if (7..=8).contains(&fault) {
            assert_eq!(result, RateWriteStatus::NotKept(actual));
            assert_eq!(session.state(), SessionState::Ready);
        } else {
            assert!(matches!(result, RateWriteStatus::Uncertain { .. }));
            assert_eq!(session.state(), SessionState::Disconnected);
        }
        assert_eq!(wire.requests().len(), 4);
    }
}
