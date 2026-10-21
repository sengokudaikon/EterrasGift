use super::*;
use eg_trainer::backend::Activity;
use eg_trainer::writes::{WriteObservation, WriteOutcome, WritePlan};
use eg_trainer_wire::fields::{AttackLeechObservation, AttackLeechRead, ObservedObjectIdentity, ObservedRate};

#[test]
fn field_write_faults_keep_raw_expected_and_readback_bits_without_an_event_consumer() {
    run(async {
        for uncertain in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let faults = eg_core::fault::FaultLog::with_fallbacks(directory.path(), Some(directory.path()), vec![]);
            let shared = writable();
            let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options(), None);
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
        let (handle, worker) = start(&shared, options(), None);
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
            let (handle, worker) = start(&shared, options(), None);
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
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = prepare(&handle, target, value).await;
            let state = handle.snapshot();
            assert!(state.capabilities.attack_leech_rate);
            assert_eq!(shared.count("rate-write"), 0);
            command(&handle, Command::SetAttackLeechRate(Box::new(plan.clone()))).await.unwrap();
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
            assert_eq!(
                command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err().kind,
                ErrorKind::InvalidEdit
            );
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
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        shared.rate_write_enabled.store(false, Ordering::SeqCst);
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(!handle.snapshot().capabilities.attack_leech_rate);
        let n = shared.count("refresh");
        assert_eq!(command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err().kind, ErrorKind::NotReady);
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
            let (handle, worker) = start(&shared, options(), None);
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
            assert_eq!(
                command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err().kind,
                ErrorKind::InvalidEdit
            );
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
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        let n = shared.count("refresh");
        assert_eq!(
            command(&handle, Command::SetAttackLeechRate(Box::new(plan.clone()))).await.unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
        assert_eq!(shared.count("refresh"), n);
        command(&handle, Command::Disconnect).await.unwrap();
        command(&handle, Command::Connect).await.unwrap();
        let n = shared.count("refresh");
        assert_eq!(
            command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
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
            let (handle, worker) = start(&shared, config, None);
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
            let error = command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err();
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
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        shared.rate_write_block.arm();
        shared.rate_readback_block.arm();
        let receipt = handle.try_submit(Command::SetAttackLeechRate(Box::new(plan.clone()))).unwrap();
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
#[test]
fn queued_cleanup_drains_once_retains_uncertainty_after_ack_and_suppresses_new_observation() {
    run(async {
        for during_read in [false, true] {
            let shared = writable();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
            let block = if during_read { &shared.rate_readback_block } else { &shared.rate_write_block };
            block.arm();
            let receipt = handle.try_submit(Command::SetAttackLeechRate(Box::new(plan.clone()))).unwrap();
            block.wait_started().await;
            let disable = handle.try_submit(Command::Disable).unwrap();
            assert_eq!(shared.count("disconnect"), 0);
            block.finish();
            assert_eq!(receipt.wait().await.unwrap_err().kind, ErrorKind::Superseded);
            disable.wait().await.unwrap();
            let state = handle.snapshot();
            assert_eq!(state.phase, Phase::Disabled);
            assert!(state.attack_leech.is_none());
            let status = state.last_rate_write.unwrap().status;
            if during_read {
                assert_eq!(status, RateWriteStatus::Confirmed(*plan.projected()));
            } else {
                assert!(matches!(status, RateWriteStatus::Uncertain { .. }));
                assert_eq!(shared.count("rate-confirmation-read"), 0);
            }
            assert_eq!(shared.count("rate-write"), 1);
            assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn dropped_receipt_drains_uncertainty_blocks_and_monitor_retry_never_replays() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        *shared.rate_write_result.lock().unwrap() = Some(Ok(RateWriteStatus::uncertain("reply lost")));
        shared.rate_write_block.arm();
        drop(handle.try_submit(Command::SetAttackLeechRate(Box::new(plan))).unwrap());
        shared.rate_write_block.wait_started().await;
        shared.rate_write_block.finish();
        wait_phase(&handle, Phase::Blocked).await;
        assert!(matches!(handle.snapshot().last_rate_write.unwrap().status, RateWriteStatus::Uncertain { .. }));
        assert_eq!(handle.snapshot().error.unwrap().delivery, Some(Delivery::MayHaveRun));
        command(&handle, Command::StartMonitoring).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        command(&handle, Command::Retry).await.unwrap();
        assert_eq!(shared.count("rate-write"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn mismatched_or_pending_connection_results_are_uncertain_and_not_sent_is_definitive() {
    run(async {
        for fault in 0..7 {
            let shared = writable();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
            let mut wrong = *plan.projected();
            wrong.increased_leech_rate.raw_bits ^= 1;
            *shared.rate_write_result.lock().unwrap() = Some(match fault {
                0 => Ok(RateWriteStatus::Confirmed(wrong)),
                1 => Ok(RateWriteStatus::Pending),
                2 => Err(BackendError {
                    kind: ErrorKind::Transport,
                    message: "may have run".into(),
                    delivery: Some(Delivery::MayHaveRun),
                    retry_when_waiting: true,
                }),
                3 => Err(BackendError::blocked(ErrorKind::NotReady, "proven pre-send refusal")),
                6 => Err(BackendError {
                    kind: ErrorKind::NotReady,
                    message: "not-ready diagnostic cannot override uncertain delivery".into(),
                    delivery: Some(Delivery::MayHaveRun),
                    retry_when_waiting: true,
                }),
                _ => Ok(RateWriteStatus::NotApplied {
                    code: if fault == 4 {
                        eg_trainer_wire::protocol::ResponseCode::Failed
                    } else {
                        eg_trainer_wire::protocol::ResponseCode::Done
                    },
                    message: "invalid alleged refusal".into(),
                }),
            });
            let e = command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err();
            assert_eq!(
                e.kind,
                match fault {
                    2 => ErrorKind::Transport,
                    3 | 6 => ErrorKind::NotReady,
                    _ => ErrorKind::WriteUncertain,
                }
            );
            assert_eq!(handle.snapshot().phase, if fault == 3 { Phase::Ready } else { Phase::Blocked });
            assert_eq!(shared.count("rate-write"), 1);
            assert_eq!(handle.snapshot().attack_leech.is_some(), fault == 3);
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn log_loss_after_confirmation_withholds_publication_and_retains_historical_confirmation() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
        shared.rate_readback_block.arm();
        let receipt = handle.try_submit(Command::SetAttackLeechRate(Box::new(plan))).unwrap();
        shared.rate_readback_block.wait_started().await;
        *shared.log.lock().unwrap() = log(GameReadiness::Online);
        shared.rate_readback_block.finish();
        assert_eq!(receipt.wait().await.unwrap_err().kind, ErrorKind::Gate);
        let state = handle.snapshot();
        assert!(state.attack_leech.is_none());
        assert!(matches!(state.last_rate_write.unwrap().status, RateWriteStatus::Confirmed(_)));
        assert_eq!(shared.count("disconnect"), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn fresh_status_failure_or_cleanup_barrier_prevents_submission_and_releases_old_ready() {
    run(async {
        for barrier in [false, true] {
            let shared = writable();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
            if barrier {
                shared.refresh_block.arm();
                let receipt = handle.try_submit(Command::SetAttackLeechRate(Box::new(plan))).unwrap();
                shared.refresh_block.wait_started().await;
                let disable = handle.try_submit(Command::Disable).unwrap();
                shared.refresh_block.finish();
                assert_eq!(receipt.wait().await.unwrap_err().kind, ErrorKind::Superseded);
                disable.wait().await.unwrap();
            } else {
                shared.read_errors.lock().unwrap().push_back(BackendError::blocked(ErrorKind::NotReady, "status refused"));
                assert_eq!(command(&handle, Command::SetAttackLeechRate(Box::new(plan))).await.unwrap_err().kind, ErrorKind::NotReady);
                assert_eq!(handle.snapshot().phase, Phase::Blocked);
            }
            assert_eq!(shared.count("rate-write"), 0);
            assert!(handle.snapshot().last_rate_write.is_none());
            assert!(handle.snapshot().attack_leech.is_none());
            shutdown(&handle, worker).await;
        }
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
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            if rate_first {
                let rate = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
                command(&handle, Command::SetAttackLeechRate(Box::new(rate))).await.unwrap();
                let kept = handle.snapshot().last_rate_write.unwrap();
                let forge = handle.snapshot().level_requirement_plan(17).unwrap();
                command(&handle, Command::ApplyForge(Box::new(forge))).await.unwrap();
                assert_eq!(handle.snapshot().last_rate_write, Some(kept.clone()));
                assert!(handle.snapshot().undo_forge.is_some());
                *shared.write_result.lock().unwrap() = Some(WriteStatus::uncertain("forge uncertainty"));
                let forge = handle.snapshot().level_requirement_plan(19).unwrap();
                assert_eq!(command(&handle, Command::ApplyForge(Box::new(forge))).await.unwrap_err().kind, ErrorKind::WriteUncertain);
                assert_eq!(handle.snapshot().last_rate_write, Some(kept));
            } else {
                let forge = handle.snapshot().level_requirement_plan(17).unwrap();
                command(&handle, Command::ApplyForge(Box::new(forge))).await.unwrap();
                let kept = handle.snapshot().last_write.unwrap();
                let undo = handle.snapshot().undo_forge.unwrap();
                let rate = prepare(&handle, AttackLeechTarget::AttackRate, 1.1).await;
                command(&handle, Command::SetAttackLeechRate(Box::new(rate))).await.unwrap();
                assert_eq!(handle.snapshot().last_write, Some(kept.clone()));
                assert_eq!(handle.snapshot().undo_forge, Some(undo));
                *shared.rate_write_result.lock().unwrap() = Some(Ok(RateWriteStatus::uncertain("rate uncertainty")));
                let rate = prepare(&handle, AttackLeechTarget::AttackRate, 1.2).await;
                assert_eq!(
                    command(&handle, Command::SetAttackLeechRate(Box::new(rate))).await.unwrap_err().kind,
                    ErrorKind::WriteUncertain
                );
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
