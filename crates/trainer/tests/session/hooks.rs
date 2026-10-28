use super::*;
use eg_trainer::{
    backend::{BackendConnection, HookRequest},
    features::HookId,
    write::{WriteControl, WriteOutcome},
};
use eg_trainer_wire::hooks::{GameplayHook, HookObservation, PatchState};

struct AllowWrite;
impl WriteControl for AllowWrite {
    fn allowed(&mut self) -> bool {
        true
    }
}

#[test]
fn shared_patch_ownership_is_reported_by_the_helper_for_any_disabled_feature() {
    for hook in GameplayHook::ALL {
        let fixture = Fixture::rehashing();
        let shared = || {
            done(
                HookObservation {
                    patch: PatchState::Shared,
                    enabled: false,
                    value: hook.default_value(),
                }
                .encode(),
            )
        };
        let wire = FakeTransport::new(
            fixture.target.clone(),
            [fixture.discover(), status(false, 0x1234, 0), done(vec![]), shared(), status(false, 0x1234, 0), shared()],
        );
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        assert_eq!(run(session.set_gameplay_hook(hook, false, &mut AllowWrite)).unwrap(), WriteOutcome::Confirmed(false));
        run(BackendConnection::refresh(&mut session)).unwrap();
        assert_eq!(session.hook_observation(hook).unwrap().patch, PatchState::Shared);
        assert_eq!(session.observed_hook(hook), Some(false));
        assert_eq!(session.state(), SessionState::Ready);
    }
}

#[test]
fn persistent_stats_use_independent_hook_values_and_readback() {
    let fixture = Fixture::rehashing();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    for (hook, on, value) in [
        (GameplayHook::AttackRate, true, 1.25f64),
        (GameplayHook::LeechRate, true, 2.5),
        (GameplayHook::AttackRate, true, 3.0),
        (GameplayHook::AttackRate, false, 3.0),
    ] {
        wire.push(done(vec![]));
        wire.push(done(
            HookObservation {
                patch: if on { PatchState::Patched } else { PatchState::Original },
                enabled: on,
                value: if on { value.to_bits() } else { hook.default_value() },
            }
            .encode(),
        ));
        let request = HookRequest {
            id: HookId::parse(hook.id()).unwrap(),
            on,
            values: vec![value],
        };
        assert_eq!(
            run(BackendConnection::set_hook(&mut session, &request, &mut AllowWrite)).unwrap(),
            WriteOutcome::Confirmed(on)
        );
        let sent = wire.requests();
        assert!(matches!(sent[sent.len()-2],Request::SetGameplayHook { hook: actual, on: enabled, value: bits, .. }
            if actual==hook && enabled==on && bits==value.to_bits()));
    }
    assert!(!session.hook_observation(GameplayHook::AttackRate).unwrap().enabled);
    assert_eq!(session.hook_observation(GameplayHook::LeechRate).unwrap().value, 2.5f64.to_bits());
    assert!(session.hook_observation(GameplayHook::LeechRate).unwrap().enabled);
    let sent = wire.requests().len();
    for (id, value) in [("attack-rate", 0.0), ("leech-rate", f64::NAN), ("leech-rate", 101.0)] {
        assert!(matches!(
            run(BackendConnection::set_hook(
                &mut session,
                &HookRequest {
                    id: HookId::parse(id).unwrap(),
                    on: true,
                    values: vec![value]
                },
                &mut AllowWrite
            ))
            .unwrap(),
            WriteOutcome::NotApplied { .. }
        ));
    }
    assert_eq!(wire.requests().len(), sent);
}

#[test]
fn rift_mode_is_sent_and_confirmed_independently_of_enabled_state() {
    let fixture = Fixture::rehashing();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    for (choice, on, actual_answer, kept) in [(1.0, true, true, true), (2.0, true, false, true), (1.0, true, false, false), (0.0, false, false, true)] {
        wire.push(done(vec![]));
        wire.push(done(
            HookObservation {
                patch: if on { PatchState::Patched } else { PatchState::Original },
                enabled: on,
                value: u64::from(actual_answer),
            }
            .encode(),
        ));
        let request = HookRequest {
            id: HookId::parse("mysterious-rift").unwrap(),
            on,
            values: vec![choice],
        };
        assert_eq!(
            run(BackendConnection::set_hook(&mut session, &request, &mut AllowWrite)).unwrap(),
            if kept { WriteOutcome::Confirmed(on) } else { WriteOutcome::NotKept(on) }
        );
        let sent = wire.requests();
        assert!(matches!(sent[sent.len()-2], Request::SetGameplayHook { on: sent_on, value, .. } if sent_on == on && value == u64::from(choice != 2.0)));
    }
}

#[test]
fn monitor_cannot_keep_reporting_portal_when_answer_changes_to_boss() {
    let fixture = Fixture::rehashing();
    let observation = |value| {
        done(
            HookObservation {
                patch: PatchState::Patched,
                enabled: true,
                value,
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
            observation(1),
            status(false, 0x1234, 0),
            observation(0),
        ],
    );
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    run(session.set_gameplay_hook(GameplayHook::MysteriousRift, true, &mut AllowWrite)).unwrap();
    assert!(run(BackendConnection::refresh(&mut session)).is_err());
    assert_eq!(session.state(), SessionState::Disconnected);
}

#[test]
fn each_gameplay_hook_requires_a_separate_readback_for_on_and_off() {
    for hook in GameplayHook::ALL {
        let fixture = Fixture::rehashing();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        assert!(session.supports_hook(hook));
        for on in [true, false] {
            wire.push(done(vec![]));
            wire.push(done(
                HookObservation {
                    patch: if on { PatchState::Patched } else { PatchState::Original },
                    enabled: on,
                    value: hook.default_value(),
                }
                .encode(),
            ));
            let request = HookRequest {
                id: HookId::parse(hook.id()).unwrap(),
                on,
                values: if matches!(hook, GameplayHook::Offhand | GameplayHook::ZeroManaCost) {
                    vec![]
                } else if hook == GameplayHook::LeechRate {
                    vec![0.0]
                } else {
                    vec![1.0]
                },
            };
            assert_eq!(
                run(BackendConnection::set_hook(&mut session, &request, &mut AllowWrite)).unwrap(),
                WriteOutcome::Confirmed(on)
            );
            let sent = wire.requests();
            assert!(matches!(sent[sent.len()-2],Request::SetGameplayHook { hook: id,on: enabled,expected_manager:0x1234,.. } if id == hook && enabled == on));
            assert!(matches!(sent[sent.len()-1],Request::ReadGameplayHook { hook: id,expected_manager:0x1234,.. } if id == hook));
        }
    }
}

#[test]
fn a_gameplay_hook_ack_cannot_hide_missing_or_inconsistent_readback() {
    for payload in [
        vec![],
        HookObservation {
            patch: PatchState::Original,
            enabled: true,
            value: 1,
        }
        .encode(),
        HookObservation {
            patch: PatchState::Unknown,
            enabled: false,
            value: 1,
        }
        .encode(),
    ] {
        let fixture = Fixture::rehashing();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(vec![]), done(payload)]);
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        assert!(matches!(
            run(session.set_gameplay_hook(GameplayHook::MysteriousRift, true, &mut AllowWrite)).unwrap(),
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
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0), done(vec![])]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    assert!(matches!(
        run(session.set_gameplay_hook(GameplayHook::Offhand, true, &mut StopAfterAck(true))).unwrap(),
        WriteOutcome::Uncertain { .. }
    ));
    assert!(run(session.set_gameplay_hook(GameplayHook::Offhand, true, &mut AllowWrite)).is_err());
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn hook_monitor_reads_actual_disabled_state_after_helper_suspension() {
    let fixture = Fixture::rehashing();
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
                    value: 1,
                }
                .encode(),
            ),
            status(false, 0x1234, 0),
            done(
                HookObservation {
                    patch: PatchState::Original,
                    enabled: false,
                    value: 1,
                }
                .encode(),
            ),
        ],
    );
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    run(session.set_gameplay_hook(GameplayHook::Offhand, true, &mut AllowWrite)).unwrap();
    run(BackendConnection::refresh(&mut session)).unwrap();
    assert_eq!(session.observed_hook(GameplayHook::Offhand), Some(false));
}

#[test]
fn damage_percentage_must_match_independent_readback() {
    let fixture = Fixture::rehashing();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    for (wanted, actual) in [(0, 0), (50, 50), (100, 100), (25, 50)] {
        wire.push(done(vec![]));
        wire.push(done(
            HookObservation {
                patch: PatchState::Patched,
                enabled: true,
                value: actual,
            }
            .encode(),
        ));
        let request = HookRequest {
            id: HookId::parse("incoming-damage").unwrap(),
            on: true,
            values: vec![wanted as f64],
        };
        assert_eq!(
            run(BackendConnection::set_hook(&mut session, &request, &mut AllowWrite)).unwrap(),
            if wanted == actual { WriteOutcome::Confirmed(true) } else { WriteOutcome::NotKept(true) }
        );
    }
}

#[test]
fn outgoing_modes_require_exact_readback_and_off_accepts_a_verified_shared_target() {
    let fixture = Fixture::rehashing();
    let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
    let mut session = fixture.session(&wire);
    session.enable();
    run(session.connect()).unwrap();
    for (wanted, actual, on) in [(0.0f64, 0.0f64, true), (3.125, 3.125, true), (101.0, 101.0, true), (101.0, 100.0, true), (1.0, 1.0, false)] {
        wire.push(done(vec![]));
        wire.push(done(
            HookObservation {
                patch: if on { PatchState::Patched } else { PatchState::Shared },
                enabled: on,
                value: actual.to_bits(),
            }
            .encode(),
        ));
        let request = HookRequest {
            id: HookId::parse("damage-dealt").unwrap(),
            on,
            values: vec![wanted],
        };
        assert_eq!(
            run(BackendConnection::set_hook(&mut session, &request, &mut AllowWrite)).unwrap(),
            if wanted == actual { WriteOutcome::Confirmed(on) } else { WriteOutcome::NotKept(on) }
        );
    }
    let before = wire.requests().len();
    for value in [f64::NAN, f64::INFINITY, -1.0, 100.5, 102.0] {
        let request = HookRequest {
            id: HookId::parse("damage-dealt").unwrap(),
            on: true,
            values: vec![value],
        };
        assert!(matches!(
            run(BackendConnection::set_hook(&mut session, &request, &mut AllowWrite)).unwrap(),
            WriteOutcome::NotApplied { .. }
        ));
    }
    assert_eq!(wire.requests().len(), before);
}
