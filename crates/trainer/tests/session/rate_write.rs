use super::*;
use eg_trainer::{
    backend::AttackLeechSnapshot,
    field_writes::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, RateWriteStatus},
    writes::{WriteControl, WriteObservation, WriteOutcome},
};
use eg_trainer_wire::fields::{self, AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate};

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
fn ready(f: &Fixture, wire: &FakeTransport, log: &FakeLog) -> Session<FakeTransport> {
    let mut session = f.session(wire, log, GatePolicy::Release);
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
fn explicit_rate_change_needs_two_exact_full_observations_preserves_bits_and_record() {
    for (target, value) in [
        (AttackLeechTarget::AttackRate, 1.1),
        (AttackLeechTarget::AttackRate, 0.01),
        (AttackLeechTarget::IncreasedLeechRate, 0.0),
    ] {
        let f = Fixture::new();
        let p = plan(&f, target, value);
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), ack(&p), ack(&p), clear()]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        assert_eq!(
            run(session.write(&p.clone().into())).unwrap(),
            WriteOutcome::Confirmed(WriteObservation::Field(*p.projected()))
        );
        assert_eq!(session.state(), SessionState::Ready);
        assert!(session.last_write().is_none());
        assert!(matches!(&wire.requests()[2],Request::SetAttackLeechRate(w) if *w.expected()==observation() && w.projected()==*p.projected() && w.expected_manager()==0x1234));
        assert_eq!(
            wire.requests()[3],
            Request::ReadAttackLeech {
                dispatcher: f.dispatcher,
                expected_manager: 0x1234
            }
        );
        run(session.disconnect());
        assert_eq!(session.last_rate_write().unwrap().status, RateWriteStatus::Confirmed(*p.projected()));
        assert_eq!(wire.requests().len(), 5);
    }
}
#[test]
fn malformed_unavailable_and_every_full_ack_drift_are_uncertain_without_independent_io() {
    for fault in 0..13 {
        let f = Fixture::new();
        let p = plan(&f, AttackLeechTarget::AttackRate, 1.1);
        let mut actual = *p.projected();
        let bytes = match fault {
            0 => vec![1],
            1 => fields::encode(&AttackLeechRead::Unavailable(AttackLeechUnavailable::StatsAbsent)),
            2..=9 => {
                mutate(&mut actual, fault - 2);
                fields::encode(&AttackLeechRead::Available(actual))
            }
            10 => {
                let mut bytes = fields::encode(&AttackLeechRead::Available(actual));
                bytes.push(0);
                bytes
            }
            11 => {
                let mut bytes = fields::encode(&AttackLeechRead::Available(actual));
                bytes[4] = 2;
                bytes
            }
            _ => {
                let mut bytes = fields::encode(&AttackLeechRead::Available(actual));
                bytes[7] = 1;
                bytes
            }
        };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), done(bytes)]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        assert!(matches!(run(session.set_attack_leech_rate(&p)).unwrap(), RateWriteStatus::Uncertain { .. }));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert!(run(session.set_attack_leech_rate(&p)).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
}
#[test]
fn only_trusted_prestore_refusal_or_not_sent_is_definitive_never_a_retry() {
    for fault in 0..10 {
        let f = Fixture::new();
        let p = plan(&f, AttackLeechTarget::AttackRate, 1.1);
        let step = if fault < 2 {
            Step::Reply(Err(TransportError {
                delivery: if fault == 0 { Delivery::NotSent } else { Delivery::MayHaveRun },
                failure: TransportFailure::Timeout,
            }))
        } else {
            let code = [
                ResponseCode::NotReady,
                ResponseCode::StaleItem,
                ResponseCode::Refused,
                ResponseCode::OfflineRequired,
                ResponseCode::RestartRequired,
                ResponseCode::Failed,
                ResponseCode::Refused,
                ResponseCode::NotReady,
            ][fault - 2];
            let payload = if fault == 8 {
                vec![0xff]
            } else if fault == 9 {
                vec![b'x'; 4097]
            } else {
                b"trusted refusal".to_vec()
            };
            Step::Reply(Ok(Response { code, payload }))
        };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), step]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        let result = run(session.set_attack_leech_rate(&p)).unwrap();
        assert_eq!(matches!(result, RateWriteStatus::Uncertain { .. }), fault == 1 || fault >= 7);
        if (2..=4).contains(&fault) {
            assert_eq!(session.state(), SessionState::Ready);
            wire.push(done(fields::encode(&AttackLeechRead::Available(observation()))));
            assert!(matches!(run(session.read_attack_leech()).unwrap(), AttackLeechRead::Available(_)));
            assert_eq!(wire.requests().len(), 4);
        } else {
            assert!(run(session.set_attack_leech_rate(&p)).is_err());
            assert_eq!(wire.requests().len(), 3);
        }
        assert_eq!(wire.requests().iter().filter(|r| matches!(r, Request::SetAttackLeechRate(_))).count(), 1);
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
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
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
#[test]
fn artifact_loss_at_either_reply_withholds_confirmation_and_stops_further_io() {
    for readback in [false, true] {
        for artifact in [false, true] {
            let f = Fixture::rehashing();
            let p = plan(&f, AttackLeechTarget::AttackRate, 1.1);
            let log = FakeLog::new([offline()]);
            let metadata = if artifact { f.binary.clone() } else { f.metadata.clone() };
            let action = Step::Action(
                Box::new(move || fs::write(metadata, b"artifact replaced").unwrap()),
                Response {
                    code: ResponseCode::Done,
                    payload: fields::encode(&AttackLeechRead::Available(*p.projected())),
                },
            );
            let mut steps = vec![f.discover(), status(false, 0x1234, 0)];
            if readback {
                steps.push(ack(&p));
            }
            steps.push(action);
            let wire = FakeTransport::new(f.target.clone(), steps);
            let mut session = ready(&f, &wire, &log);
            assert!(matches!(run(session.set_attack_leech_rate(&p)).unwrap(), RateWriteStatus::Uncertain { .. }));
            assert_ne!(session.state(), SessionState::Ready);
            assert_eq!(wire.requests().len(), if readback { 4 } else { 3 });
        }
    }
}
#[test]
fn preflight_wrong_process_manager_or_changed_artifact_has_no_rate_record_or_rpc() {
    for fault in 0..3 {
        let f = Fixture::rehashing();
        let p = if fault == 0 {
            plan(&Fixture::rehashing(), AttackLeechTarget::AttackRate, 1.1)
        } else {
            plan(&f, AttackLeechTarget::AttackRate, 1.1)
        };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, if fault == 1 { 0x9999 } else { 0x1234 }, 0)]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        if fault == 2 {
            fs::write(&f.metadata, b"changed before send").unwrap();
        }
        assert!(matches!(run(session.set_attack_leech_rate(&p)), Err(SessionError::Identity(_))));
        assert!(session.last_rate_write().is_none());
        assert_eq!(wire.requests().len(), 2);
    }
}

#[test]
fn abandonment_at_send_or_read_keeps_pending_and_busy_until_disconnect_makes_uncertain() {
    for readback in [false, true] {
        let f = Fixture::new();
        let p = plan(&f, AttackLeechTarget::AttackRate, 1.1);
        let mut steps = vec![f.discover(), status(false, 0x1234, 0)];
        if readback {
            steps.push(ack(&p));
        }
        steps.push(Step::Pending);
        let wire = FakeTransport::new(f.target.clone(), steps);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        let mut future = Box::pin(session.set_attack_leech_rate(&p));
        assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        drop(future);
        assert_eq!(session.last_rate_write().unwrap().status, RateWriteStatus::Pending);
        assert_eq!(session.state(), SessionState::Connecting);
        assert!(matches!(run(session.set_attack_leech_rate(&p)), Err(SessionError::Busy)));
        assert!(matches!(run(session.read_forge()), Err(SessionError::Busy)));
        assert!(matches!(run(session.read_attack_leech()), Err(SessionError::Busy)));
        assert_eq!(wire.requests().len(), if readback { 4 } else { 3 });
        wire.push(clear());
        run(session.disconnect());
        assert!(matches!(session.last_rate_write().unwrap().status, RateWriteStatus::Uncertain { .. }));
        assert!(session.last_write().is_none());
        assert_eq!(wire.requests().len(), if readback { 5 } else { 4 });
    }
}
#[test]
fn cleanup_control_cancels_only_before_unsent_rpcs() {
    struct StopAfter(usize);
    impl WriteControl for StopAfter {
        fn allowed(&mut self) -> bool {
            let was = self.0;
            self.0 = self.0.saturating_sub(1);
            was > 0
        }
    }
    for limit in [0, 1] {
        let f = Fixture::new();
        let p = plan(&f, AttackLeechTarget::AttackRate, 1.1);
        let mut steps = vec![f.discover(), status(false, 0x1234, 0)];
        if limit == 1 {
            steps.push(ack(&p));
        }
        let wire = FakeTransport::new(f.target.clone(), steps);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        let outcome = run(session.set_attack_leech_rate_controlled(&p, &mut StopAfter(limit)));
        if limit == 0 {
            assert!(matches!(outcome, Err(SessionError::Busy)));
            assert!(session.last_rate_write().is_none());
            assert_eq!(session.state(), SessionState::Ready);
        } else {
            assert!(matches!(outcome.unwrap(), RateWriteStatus::Uncertain { .. }));
            assert_eq!(session.state(), SessionState::Disconnected);
        }
        assert_eq!(wire.requests().len(), 2 + limit);
    }
}

use eg_trainer::{
    catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    edits::{ExpectedObservation, ItemEdit, StatusEdit, StatusField},
    writes::{ForgePlan, WriteStatus},
};
fn forge_plan(fixture: &Fixture) -> ForgePlan {
    let profile = fixture.profile();
    let catalog = GameCatalog {
        identity: CatalogIdentity {
            binary_hash: profile.binary_hash().into(),
            metadata_hash: profile.metadata_hash().into(),
            locale: GameLocale {
                language: "en".into(),
                game_version: "authored".into(),
            },
            extraction_version: EXTRACTION_VERSION,
        },
        limits: item_limits(),
        bases: Default::default(),
        affixes: Default::default(),
        uniques: Default::default(),
        rarities: Default::default(),
    };
    let expected = ExpectedObservation::new(1, fixture.target.clone(), catalog.identity.clone(), forge_item()).unwrap();
    ForgePlan::edit(
        &expected,
        0x1234,
        &catalog,
        ItemEdit::Status(StatusEdit {
            field: StatusField::LevelRequirement,
            value: 17,
            unrestricted: false,
        }),
        false,
    )
    .unwrap()
}

#[test]
fn rate_and_forge_records_survive_each_other_including_abandoned_second_submission() {
    for rate_first in [false, true] {
        for abandon in [false, true] {
            let f = Fixture::new();
            let rate = plan(&f, AttackLeechTarget::AttackRate, 1.1);
            let forge = forge_plan(&f);
            let first = if rate_first {
                fields::encode(&AttackLeechRead::Available(*rate.projected()))
            } else {
                forge::encode(Some(forge.projected())).unwrap()
            };
            let mut steps = vec![f.discover(), status(false, 0x1234, 0), done(first.clone()), done(first)];
            steps.push(if abandon {
                Step::Pending
            } else {
                Step::Reply(Ok(Response {
                    code: ResponseCode::Failed,
                    payload: b"second operation may have stored".to_vec(),
                }))
            });
            let wire = FakeTransport::new(f.target.clone(), steps);
            let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
            if rate_first {
                assert!(matches!(run(session.set_attack_leech_rate(&rate)).unwrap(), RateWriteStatus::Confirmed(_)));
                let kept = session.last_rate_write().unwrap().clone();
                if abandon {
                    let mut future = Box::pin(session.edit_forge(&forge));
                    assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
                    drop(future);
                    assert_eq!(session.last_write().unwrap().status, WriteStatus::Pending);
                    wire.push(clear());
                } else {
                    assert!(matches!(run(session.edit_forge(&forge)).unwrap(), WriteStatus::Uncertain { .. }));
                }
                assert_eq!(session.last_rate_write(), Some(&kept));
                run(session.disconnect());
                assert!(matches!(session.last_write().unwrap().status, WriteStatus::Uncertain { .. }));
                assert_eq!(session.last_rate_write(), Some(&kept));
            } else {
                assert!(matches!(run(session.edit_forge(&forge)).unwrap(), WriteStatus::Confirmed(_)));
                let kept = session.last_write().unwrap().clone();
                if abandon {
                    let mut future = Box::pin(session.set_attack_leech_rate(&rate));
                    assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
                    drop(future);
                    assert_eq!(session.last_rate_write().unwrap().status, RateWriteStatus::Pending);
                    wire.push(clear());
                } else {
                    assert!(matches!(run(session.set_attack_leech_rate(&rate)).unwrap(), RateWriteStatus::Uncertain { .. }));
                }
                assert_eq!(session.last_write(), Some(&kept));
                run(session.disconnect());
                assert!(matches!(session.last_rate_write().unwrap().status, RateWriteStatus::Uncertain { .. }));
                assert_eq!(session.last_write(), Some(&kept));
            }
            assert_eq!(wire.requests().len(), 6);
        }
    }
}
