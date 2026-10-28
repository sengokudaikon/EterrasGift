use super::*;
use eg_trainer::backend::Activity;
use eg_trainer::write::{WriteObservation, WriteOutcome, WritePlan};
use eg_trainer_wire::fields::{AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate};

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
fn attack_leech_capability_is_explicit_and_never_enables_generic_fields_or_writes() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        assert!(!handle.snapshot().capabilities.read_attack_leech);
        assert_eq!(command(&handle, Command::ReadAttackLeech).await.unwrap_err().kind, ErrorKind::NotReady);
        assert_eq!(shared.count("attack-leech"), 0);
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        command(&handle, Command::ReadForge).await.unwrap();
        let caps = handle.snapshot().capabilities;
        assert!(caps.read_attack_leech);
        assert!(!caps.level_requirement);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn attack_leech_observations_bind_process_generation_and_monotonic_sequence_including_local_unavailable() {
    run(async {
        let shared = Shared::new();
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        *shared.attack_leech_read.lock().unwrap() = available(0x1000);
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        let first = handle.snapshot().attack_leech.unwrap();
        assert_eq!(first.process, target());
        assert_eq!(first.generation, handle.snapshot().generation);
        assert_eq!(first.manager, 0x1000);
        assert_eq!(first.sequence, 1);
        assert_eq!(first.read, available(0x1000));
        for reason in [
            AttackLeechUnavailable::ActorCacheAbsent,
            AttackLeechUnavailable::StatsAbsent,
            AttackLeechUnavailable::NativeObjectDestroyed,
            AttackLeechUnavailable::GraphChanged,
            AttackLeechUnavailable::UnsupportedStatsClass,
        ] {
            *shared.attack_leech_read.lock().unwrap() = AttackLeechRead::Unavailable(reason);
            let previous = handle.snapshot().attack_leech.unwrap().sequence;
            command(&handle, Command::ReadAttackLeech).await.unwrap();
            let state = handle.snapshot();
            assert_eq!(state.phase, Phase::Ready);
            assert_eq!(state.attack_leech.as_ref().unwrap().sequence, previous + 1);
            assert_eq!(state.attack_leech.unwrap().read, AttackLeechRead::Unavailable(reason));
        }
        let previous = handle.snapshot().attack_leech.unwrap().sequence;
        command(&handle, Command::Disconnect).await.unwrap();
        assert!(handle.snapshot().attack_leech.is_none());
        command(&handle, Command::Connect).await.unwrap();
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        let next = handle.snapshot().attack_leech.unwrap();
        assert_eq!(next.sequence, previous + 1);
        assert_ne!(next.generation, first.generation);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn attack_leech_manager_drift_clears_previous_observation_and_wrong_wrapper_never_publishes() {
    run(async {
        let shared = Shared::new();
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        *shared.attack_leech_read.lock().unwrap() = available(0x1000);
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        shared.status.lock().unwrap().manager = 0x9999;
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(handle.snapshot().attack_leech.is_none());
        assert_eq!(command(&handle, Command::ReadAttackLeech).await.unwrap_err().kind, ErrorKind::Protocol);
        assert!(handle.snapshot().attack_leech.is_none());
        assert!(handle.snapshot().attached.is_none());
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn field_write_faults_keep_raw_expected_and_readback_bits_without_an_event_consumer() {
    run(async {
        for uncertain in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let faults = eg_core::fault::FaultLog::with_fallbacks(directory.path(), Some(directory.path()), vec![]);
            let shared = writable();
            let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options());
            let worker = tokio::spawn(worker.with_fault_log(faults).run());
            connected(&handle).await;
            let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
            let mut actual = *plan.projected();
            actual.attack_rate.raw_bits = 1.2_f32.to_bits();
            *shared.rate_write_result.lock().unwrap() = Some(Ok(if uncertain {
                RateWriteStatus::uncertain("lost field readback")
            } else {
                RateWriteStatus::NotKept(actual)
            }));
            assert!(command(&handle, Command::Write(plan.into())).await.is_err());
            shutdown(&handle, worker).await;
            let reports = std::fs::read_dir(directory.path())
                .unwrap()
                .map(|e| serde_json::from_slice::<serde_json::Value>(&std::fs::read(e.unwrap().path()).unwrap()).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(reports.len(), 1);
            let report = &reports[0];
            assert_eq!(report["operation"], "Trainer field write");
            assert_eq!(report["expected"]["observation"]["attackRateBits"], 0x7fc12345_u32);
            assert_eq!(report["readback"]["observation"].is_null(), uncertain);
            if !uncertain {
                assert_eq!(report["readback"]["observation"]["attackRateBits"], 1.2_f32.to_bits());
            }
        }
    });
}

#[test]
fn generic_field_write_publishes_all_actual_bits_and_advances_sequence_without_reconnect() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        let mut actual = *plan.projected();
        actual.attack_rate.raw_bits = 1.2_f32.to_bits();
        actual.increased_leech_rate.raw_bits = 0.25_f32.to_bits();
        *shared.rate_write_result.lock().unwrap() = Some(Ok(RateWriteStatus::NotKept(actual)));
        let generic = WritePlan::from(plan.clone());
        assert_eq!(command(&handle, Command::Write(generic.clone())).await.unwrap_err().kind, ErrorKind::WriteNotKept);
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::Ready);
        assert_eq!(state.write_outcome(&generic), Some(WriteOutcome::NotKept(WriteObservation::Field(actual))));
        let observed = state.attack_leech.unwrap();
        assert_eq!(observed.read, AttackLeechRead::Available(actual));
        assert_eq!(observed.sequence, plan.before().sequence + 1);
        assert_eq!(shared.count("connect"), 1);
        assert_eq!(shared.count("rate-write"), 1);
        assert_eq!(shared.count("rate-confirmation-read"), 1);
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        assert_eq!(shared.count("rate-write"), 1);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn not_kept_label_cannot_publish_another_player_or_an_exact_success() {
    run(async {
        for wrong_identity in [false, true] {
            let shared = writable();
            let (handle, worker) = start(&shared, options());
            connected(&handle).await;
            let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
            let mut actual = *plan.projected();
            if wrong_identity {
                actual.actor = ObservedObjectIdentity::new(0x8888, 0x9999).unwrap();
                actual.attack_rate.raw_bits ^= 1;
            }
            *shared.rate_write_result.lock().unwrap() = Some(Ok(RateWriteStatus::NotKept(actual)));
            let generic = WritePlan::from(plan);
            assert_eq!(command(&handle, Command::Write(generic.clone())).await.unwrap_err().kind, ErrorKind::WriteUncertain);
            assert!(matches!(handle.snapshot().write_outcome(&generic), Some(WriteOutcome::Uncertain { .. })));
            assert!(handle.snapshot().attack_leech.is_none());
            assert_eq!(handle.snapshot().phase, Phase::Blocked);
            shutdown(&handle, worker).await;
        }
    });
}
fn observed() -> AttackLeechRead {
    let id = |n| ObservedObjectIdentity::new(n, n + 1).unwrap();
    AttackLeechRead::Available(AttackLeechObservation {
        manager: id(0x1000),
        actor: id(0x2000),
        stats: id(0x3000),
        attack_rate: ObservedRate { raw_bits: 0x7fc12345 },
        increased_leech_rate: ObservedRate { raw_bits: 0x80000000 },
    })
}
fn writable() -> Arc<Shared> {
    let shared = Shared::new();
    shared.attack_leech_enabled.store(true, Ordering::SeqCst);
    shared.rate_write_enabled.store(true, Ordering::SeqCst);
    *shared.attack_leech_read.lock().unwrap() = observed();
    shared
}
async fn prepare(handle: &BackendHandle, target: AttackLeechTarget, value: f64) -> AttackLeechPlan {
    command(handle, Command::ReadAttackLeech).await.unwrap();
    match handle.snapshot().attack_leech_plan(target, value).unwrap() {
        AttackLeechDecision::Change(plan) => plan,
        AttackLeechDecision::NoOp => panic!("test needs a change"),
    }
}
#[test]
fn only_explicit_narrow_command_writes_and_full_confirmed_observation_gets_next_sequence() {
    run(async {
        for (target, value) in [(AttackLeechTarget::AttackRate, 1.1), (AttackLeechTarget::IncreasedLeechRate, 0.0)] {
            let shared = writable();
            let (handle, worker) = start(&shared, options());
            connected(&handle).await;
            let plan = prepare(&handle, target, value).await;
            let state = handle.snapshot();
            assert!(state.capabilities.attack_leech_rate);
            assert_eq!(shared.count("rate-write"), 0);
            command(&handle, Command::Write(plan.clone().into())).await.unwrap();
            let state = handle.snapshot();
            let fresh = state.attack_leech.unwrap();
            assert_eq!(fresh.read, AttackLeechRead::Available(*plan.projected()));
            assert_eq!(fresh.sequence, plan.before().sequence + 1);
            assert_eq!(fresh.generation, plan.before().generation);
            assert_eq!(fresh.process, plan.before().process);
            assert_eq!(state.last_rate_write.unwrap().status, RateWriteStatus::Confirmed(*plan.projected()));
            assert!(state.last_write.is_none());
            assert_eq!(shared.count("rate-write"), 1);
            assert_eq!(shared.count("rate-confirmation-read"), 1);
            let refreshes = shared.count("refresh");
            assert_eq!(command(&handle, Command::Write(plan.into())).await.unwrap_err().kind, ErrorKind::InvalidEdit);
            assert_eq!(shared.count("refresh"), refreshes);
            assert_eq!(shared.count("rate-write"), 1);
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn unavailable_capability_and_noop_intent_do_not_perform_status_or_write_io() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        shared.rate_write_enabled.store(false, Ordering::SeqCst);
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(!handle.snapshot().capabilities.attack_leech_rate);
        let n = shared.count("refresh");
        assert_eq!(command(&handle, Command::Write(plan.into())).await.unwrap_err().kind, ErrorKind::NotReady);
        assert_eq!(shared.count("refresh"), n);
        assert!(handle.snapshot().last_rate_write.is_none());
        assert_eq!(
            handle.snapshot().attack_leech_plan(AttackLeechTarget::IncreasedLeechRate, -0.0).unwrap(),
            AttackLeechDecision::NoOp
        );
        assert_eq!(shared.count("rate-write"), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn every_stale_snapshot_component_rejects_before_status_or_write_io() {
    run(async {
        for part in 0..14 {
            let shared = writable();
            let (handle, worker) = start(&shared, options());
            connected(&handle).await;
            command(&handle, Command::ReadAttackLeech).await.unwrap();
            let mut before = handle.snapshot().attack_leech.unwrap();
            match part {
                0 => before.process.pid += 1,
                1 => before.process.created += 1,
                2 => before.process.executable = PathBuf::from(r"C:\other\fixture.exe"),
                3 => before.generation += 1,
                4 => before.sequence += 1,
                5 => {
                    before.manager += 2;
                    if let AttackLeechRead::Available(ref mut o) = before.read {
                        o.manager = ObservedObjectIdentity::new(before.manager, o.manager.native_token()).unwrap();
                    }
                }
                n => {
                    let AttackLeechRead::Available(ref mut o) = before.read else { unreachable!() };
                    let id = |i: ObservedObjectIdentity, native: bool| {
                        ObservedObjectIdentity::new(i.wrapper_token() ^ u64::from(!native), i.native_token() ^ u64::from(native)).unwrap()
                    };
                    match n - 6 {
                        0 => {
                            before.manager = o.manager.wrapper_token() ^ 1;
                            o.manager = id(o.manager, false)
                        }
                        1 => o.manager = id(o.manager, true),
                        2 => o.actor = id(o.actor, false),
                        3 => o.actor = id(o.actor, true),
                        4 => o.stats = id(o.stats, false),
                        5 => o.stats = id(o.stats, true),
                        6 => o.attack_rate.raw_bits ^= 1,
                        7 => o.increased_leech_rate.raw_bits ^= 1,
                        _ => unreachable!(),
                    }
                }
            }
            let AttackLeechDecision::Change(plan) = AttackLeechPlan::prepare(&before, AttackLeechTarget::AttackRate, 1.1).unwrap() else {
                panic!()
            };
            let n = shared.count("refresh");
            assert_eq!(command(&handle, Command::Write(plan.into())).await.unwrap_err().kind, ErrorKind::InvalidEdit);
            assert_eq!(shared.count("refresh"), n);
            assert_eq!(shared.count("rate-write"), 0);
            assert!(handle.snapshot().last_rate_write.is_none());
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn new_publication_with_equal_bits_and_reconnect_invalidate_prior_plan_before_io() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        let n = shared.count("refresh");
        assert_eq!(command(&handle, Command::Write(plan.clone().into())).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("refresh"), n);
        command(&handle, Command::Disconnect).await.unwrap();
        command(&handle, Command::Connect).await.unwrap();
        let n = shared.count("refresh");
        assert_eq!(command(&handle, Command::Write(plan.into())).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("refresh"), n);
        assert_eq!(shared.count("rate-write"), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn missing_log_blocks_refresh_and_online_status_blocks_write() {
    run(async {
        for fault in 0..3 {
            let shared = writable();
            let config = options();
            let (handle, worker) = start(&shared, config);
            connected(&handle).await;
            let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
            let n = shared.count("refresh");
            if fault == 0 {
                *shared.log.lock().unwrap() = GameLogState::missing();
            } else if fault == 1 {
                shared.status.lock().unwrap().online = true;
            } else {
                shared.status.lock().unwrap().manager = 0x9999;
            }
            let error = command(&handle, Command::Write(plan.into())).await.unwrap_err();
            assert_eq!(error.kind, if fault == 2 { ErrorKind::Identity } else { ErrorKind::Gate });
            if fault == 0 {
                assert_eq!(shared.count("refresh"), n);
            }
            assert_eq!(shared.count("rate-write"), 0);
            assert!(handle.snapshot().last_rate_write.is_none());
            assert!(handle.snapshot().attack_leech.is_none());
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn pending_watch_remains_pending_through_ack_and_independent_confirmation_read() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        shared.rate_write_block.arm();
        shared.rate_readback_block.arm();
        let receipt = handle.try_submit(Command::Write(plan.clone().into())).unwrap();
        shared.rate_write_block.wait_started().await;
        let state = handle.snapshot();
        assert_eq!(state.activity, Some(Activity::WritingAttackLeech));
        assert_eq!(state.last_rate_write.unwrap().status, RateWriteStatus::Pending);
        assert!(!state.capabilities.attack_leech_rate);
        shared.rate_write_block.finish();
        shared.rate_readback_block.wait_started().await;
        assert_eq!(shared.count("rate-ack"), 1);
        assert_eq!(handle.snapshot().last_rate_write.unwrap().status, RateWriteStatus::Pending);
        assert_eq!(handle.snapshot().attack_leech.unwrap().sequence, plan.before().sequence);
        shared.rate_readback_block.finish();
        receipt.wait().await.unwrap();
        assert!(matches!(handle.snapshot().last_rate_write.unwrap().status, RateWriteStatus::Confirmed(_)));
        assert_eq!(shared.maximum.load(Ordering::SeqCst), 1);
        shutdown(&handle, worker).await;
    });
}

fn both_writable() -> Arc<Shared> {
    use eg_trainer::backend::ItemStatus;
    let shared = Shared::new();
    shared.write_enabled.store(true, Ordering::SeqCst);
    let mut snapshot = ForgeSnapshot {
        item: 0x1234,
        individual_id: 37,
        fingerprint: 0,
        name: "authored mutable item".into(),
        item_type: 3,
        sub_type: 1,
        rarity: 2,
        class_requirement: 0,
        affixes: vec![],
        implicit_rolls: vec![],
        identity: None,
        status: ItemStatus {
            forging_potential: 1,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 20,
        },
    };
    snapshot.fingerprint = snapshot.legacy_fingerprint();
    *shared.forge_item.lock().unwrap() = Some(snapshot);
    shared.attack_leech_enabled.store(true, Ordering::SeqCst);
    shared.rate_write_enabled.store(true, Ordering::SeqCst);
    *shared.attack_leech_read.lock().unwrap() = observed();
    shared
}

#[test]
fn independent_forge_and_rate_records_survive_success_and_uncertainty_in_either_order() {
    run(async {
        for rate_first in [false, true] {
            let shared = both_writable();
            let (handle, worker) = start(&shared, options());
            connected(&handle).await;
            if rate_first {
                let rate = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
                command(&handle, Command::Write(rate.into())).await.unwrap();
                let kept = handle.snapshot().last_rate_write.unwrap();
                let forge = handle.snapshot().level_requirement_plan(17).unwrap();
                command(&handle, Command::Write(forge.into())).await.unwrap();
                assert_eq!(handle.snapshot().last_rate_write, Some(kept.clone()));
                assert!(handle.snapshot().undo_forge.is_some());
                *shared.write_result.lock().unwrap() = Some(WriteStatus::uncertain("forge uncertainty"));
                let forge = handle.snapshot().level_requirement_plan(19).unwrap();
                assert_eq!(command(&handle, Command::Write(forge.into())).await.unwrap_err().kind, ErrorKind::WriteUncertain);
                assert_eq!(handle.snapshot().last_rate_write, Some(kept));
            } else {
                let forge = handle.snapshot().level_requirement_plan(17).unwrap();
                command(&handle, Command::Write(forge.into())).await.unwrap();
                let kept = handle.snapshot().last_write.unwrap();
                let undo = handle.snapshot().undo_forge.unwrap();
                let rate = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
                command(&handle, Command::Write(rate.into())).await.unwrap();
                assert_eq!(handle.snapshot().last_write, Some(kept.clone()));
                assert_eq!(handle.snapshot().undo_forge, Some(undo));
                *shared.rate_write_result.lock().unwrap() = Some(Ok(RateWriteStatus::uncertain("rate uncertainty")));
                let rate = prepare(&handle, AttackLeechTarget::AttackRate, 1.2).await;
                assert_eq!(command(&handle, Command::Write(rate.into())).await.unwrap_err().kind, ErrorKind::WriteUncertain);
                assert_eq!(handle.snapshot().last_write, Some(kept));
            }
            let state = handle.snapshot();
            assert_eq!(state.phase, Phase::Blocked);
            assert!(state.undo_forge.is_none());
            assert_eq!(state.forge, ForgeState::Unobserved);
            assert!(state.attack_leech.is_none());
            assert!(state.last_rate_write.is_some() && state.last_write.is_some());
            shutdown(&handle, worker).await;
        }
    });
}
