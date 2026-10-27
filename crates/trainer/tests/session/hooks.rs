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
                }
                .encode(),
            ));
            let request = HookRequest {
                id: HookId::parse(hook.id()).unwrap(),
                on,
                values: vec![],
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
        }
        .encode(),
        HookObservation {
            patch: PatchState::Unknown,
            enabled: false,
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
                }
                .encode(),
            ),
            status(false, 0x1234, 0),
            done(
                HookObservation {
                    patch: PatchState::Original,
                    enabled: false,
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
