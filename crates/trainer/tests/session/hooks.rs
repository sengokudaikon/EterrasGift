use super::*;
use eg_trainer::{
    backend::{BackendConnection, HookRequest},
    features::HookId,
    writes::{WriteControl, WriteOutcome},
};
use eg_trainer_wire::hooks::{BooleanHook, HookObservation, PatchState};

struct AllowWrite;
impl WriteControl for AllowWrite {
    fn allowed(&mut self) -> bool {
        true
    }
}

#[test]
fn rift_mode_is_sent_and_confirmed_independently_of_enabled_state() {
    let fixture = Fixture::rehashing();
    let log = FakeLog::new([offline()]);
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    for (choice, on, actual_answer, kept) in [(1.0, true, true, true), (2.0, true, false, true), (1.0, true, false, false), (0.0, false, false, true)] {
        wire.push(done(vec![]));
        wire.push(done(
            HookObservation {
                patch: if on { PatchState::Patched } else { PatchState::Original },
                enabled: on,
                answer: actual_answer,
            }
            .encode(),
        ));
        let request = HookRequest {
            id: HookId::parse("mysterious-rift").unwrap(),
            on,
            values: vec![choice],
        };
        assert_eq!(
            run(BackendConnection::set_hook(&mut session, now(), &request, &mut AllowWrite)).unwrap(),
            if kept { WriteOutcome::Confirmed(on) } else { WriteOutcome::NotKept(on) }
        );
        let sent = wire.requests();
        assert!(matches!(sent[sent.len()-2], Request::SetBooleanHook { on: sent_on, answer, .. } if sent_on == on && answer == (choice != 2.0)));
    }
}

#[test]
fn monitor_cannot_keep_reporting_portal_when_answer_changes_to_boss() {
    let fixture = Fixture::rehashing();
    let log = FakeLog::new([offline()]);
    let observation = |answer| {
        done(
            HookObservation {
                patch: PatchState::Patched,
                enabled: true,
                answer,
            }
            .encode(),
        )
    };
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            done(vec![]),
            observation(true),
            status(false, 0x1234, 0),
            observation(false),
        ],
    );
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    run(session.set_boolean_hook(BooleanHook::MysteriousRift, true, &mut AllowWrite)).unwrap();
    assert!(run(BackendConnection::refresh(&mut session, now())).is_err());
    assert_eq!(session.state(), SessionState::Disconnected);
}

#[test]
fn each_boolean_hook_requires_a_separate_readback_for_on_and_off() {
    for hook in BooleanHook::ALL {
        let fixture = Fixture::rehashing();
        let log = FakeLog::new([offline()]);
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(session.supports_hook(HookId::parse(hook.id()).unwrap()));
        for on in [true, false] {
            wire.push(done(vec![]));
            wire.push(done(
                HookObservation {
                    patch: if on { PatchState::Patched } else { PatchState::Original },
                    enabled: on,
                    answer: true,
                }
                .encode(),
            ));
            let request = HookRequest {
                id: HookId::parse(hook.id()).unwrap(),
                on,
                values: if hook == BooleanHook::MysteriousRift { vec![1.0] } else { vec![] },
            };
            assert_eq!(
                run(BackendConnection::set_hook(&mut session, now(), &request, &mut AllowWrite)).unwrap(),
                WriteOutcome::Confirmed(on)
            );
            let sent = wire.requests();
            assert!(matches!(sent[sent.len()-2],Request::SetBooleanHook { hook: id,on: enabled,expected_manager:0x1234,.. } if id == hook && enabled == on));
            assert!(matches!(sent[sent.len()-1],Request::ReadBooleanHook { hook: id,expected_manager:0x1234,.. } if id == hook));
        }
    }
}

#[test]
fn a_boolean_hook_ack_cannot_hide_missing_or_inconsistent_readback() {
    for payload in [
        vec![],
        HookObservation {
            patch: PatchState::Original,
            enabled: true,
            answer: true,
        }
        .encode(),
        HookObservation {
            patch: PatchState::Unknown,
            enabled: false,
            answer: true,
        }
        .encode(),
    ] {
        let fixture = Fixture::rehashing();
        let log = FakeLog::new([offline()]);
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(vec![]), done(payload)]);
        let mut session = fixture.session(&wire, &log, GatePolicy::Release);
        session.enable();
        run(session.connect(now())).unwrap();
        assert!(matches!(
            run(session.set_boolean_hook(BooleanHook::MysteriousRift, true, &mut AllowWrite)).unwrap(),
            WriteOutcome::Uncertain { .. }
        ));
        assert_eq!(wire.requests().len(), 4);
        assert_eq!(session.state(), SessionState::Disconnected);
    }
}

#[test]
fn hook_cleanup_queued_after_ack_prevents_readback_and_never_retries() {
    struct StopAfterAck(bool);
    impl WriteControl for StopAfterAck {
        fn allowed(&mut self) -> bool {
            std::mem::take(&mut self.0)
        }
    }
    let fixture = Fixture::rehashing();
    let log = FakeLog::new([offline()]);
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(vec![])]);
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    assert!(matches!(
        run(session.set_boolean_hook(BooleanHook::Offhand, true, &mut StopAfterAck(true))).unwrap(),
        WriteOutcome::Uncertain { .. }
    ));
    assert!(run(session.set_boolean_hook(BooleanHook::Offhand, true, &mut AllowWrite)).is_err());
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn hook_monitor_reads_actual_disabled_state_after_helper_suspension() {
    let fixture = Fixture::rehashing();
    let log = FakeLog::new([offline()]);
    let wire = FakeTransport::new(
        fixture.target.clone(),
        [
            fixture.discover(),
            status(false, 0x1234, 0),
            done(vec![]),
            done(
                HookObservation {
                    patch: PatchState::Patched,
                    enabled: true,
                    answer: true,
                }
                .encode(),
            ),
            status(false, 0x1234, 0),
            done(
                HookObservation {
                    patch: PatchState::Original,
                    enabled: false,
                    answer: true,
                }
                .encode(),
            ),
        ],
    );
    let mut session = fixture.session(&wire, &log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    run(session.set_boolean_hook(BooleanHook::Offhand, true, &mut AllowWrite)).unwrap();
    run(BackendConnection::refresh(&mut session, now())).unwrap();
    assert_eq!(session.observed_hook(HookId::parse("offhand").unwrap()), Some(false));
}
