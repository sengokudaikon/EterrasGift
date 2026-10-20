use super::*;
use eg_game_access::protocol::ResponseCode;
use eg_trainer::writes::{WriteObservation, WriteOutcome, WritePlan};

#[test]
fn failed_write_evidence_is_persisted_without_event_subscribers_even_during_cleanup() {
    run(async {
        for uncertain in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let faults = eg_core::fault::FaultLog::with_fallbacks(directory.path(), Some(directory.path()), vec![]);
            let shared = writable();
            let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options(), None);
            let worker = tokio::spawn(worker.with_fault_log(faults).run());
            connected(&handle).await;
            let plan = handle.snapshot().level_requirement_plan(17).unwrap();
            let mut different = plan.projected().clone();
            different.status.level_requirement = 18;
            different.fingerprint = different.legacy_fingerprint();
            *shared.write_result.lock().unwrap() = Some(if uncertain {
                WriteStatus::uncertain("lost reply")
            } else {
                WriteStatus::NotKept(different)
            });
            shared.write_block.arm();
            let receipt = handle.try_submit(Command::Write(plan.into())).unwrap();
            shared.write_block.wait_started().await;
            let shutdown = handle.try_submit(Command::Shutdown).unwrap();
            shared.write_block.finish();
            assert_eq!(receipt.wait().await.unwrap_err().kind, ErrorKind::Superseded);
            shutdown.wait().await.unwrap();
            worker.await.unwrap();
            let reports = std::fs::read_dir(directory.path())
                .unwrap()
                .map(|e| serde_json::from_slice::<serde_json::Value>(&std::fs::read(e.unwrap().path()).unwrap()).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(reports.len(), 1);
            let report = &reports[0];
            assert_eq!(report["operation"], "Forge write");
            assert_eq!(report["code"], if uncertain { "WriteUncertain" } else { "EditNotKept" });
            assert_eq!(report["edit"]["value"], 17);
            assert!(report["expected"]["item"]["status"].is_object());
            assert!(report["readback"]["differences"].is_array());
            assert_eq!(report["readback"]["item"].is_null(), uncertain);
            assert_eq!(shared.count("write"), 1);
        }
    });
}

#[test]
fn queued_override_revocation_refuses_old_plans_and_undo_before_io() {
    use eg_trainer::edits::{EditError, StatusEdit, StatusField};
    run(async {
        let shared = writable();
        {
            let mut item = shared.forge_item.lock().unwrap();
            let item = item.as_mut().unwrap();
            item.item_type = 25;
            item.fingerprint = item.legacy_fingerprint();
        }
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let edit = StatusEdit {
            field: StatusField::WeaversTouch,
            value: 10,
            unrestricted: true,
        };
        assert_eq!(handle.snapshot().status_edit_plan(edit.clone()), Err(EditError::OverrideRequired));
        command(&handle, Command::SetOverride(true)).await.unwrap();
        let plan = handle.snapshot().status_edit_plan(edit).unwrap();
        command(&handle, Command::SetOverride(false)).await.unwrap();
        let reads = shared.count("refresh");
        assert_eq!(command(&handle, Command::Write(plan.clone().into())).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("refresh"), reads);
        assert_eq!(shared.count("write"), 0);
        command(&handle, Command::SetOverride(true)).await.unwrap();
        command(&handle, Command::Write(plan.into())).await.unwrap();
        let undo = handle.snapshot().undo_forge.clone().unwrap();
        command(&handle, Command::SetOverride(false)).await.unwrap();
        let reads = shared.count("refresh");
        assert_eq!(command(&handle, Command::UndoForge(Box::new(undo.clone()))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("refresh"), reads);
        assert_eq!(shared.count("write"), 1);
        command(&handle, Command::SetOverride(true)).await.unwrap();
        command(&handle, Command::UndoForge(Box::new(undo))).await.unwrap();
        assert_eq!(shared.forge_item.lock().unwrap().as_ref().unwrap().status.weavers_touch, 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn reducing_above_limit_legendary_will_does_not_offer_an_illegal_increase_as_undo() {
    use eg_trainer::edits::{StatusEdit, StatusField};
    run(async {
        let shared = writable();
        {
            let mut item = shared.forge_item.lock().unwrap();
            let item = item.as_mut().unwrap();
            item.rarity = 9;
            item.identity = Some(eg_game_access::forge::UniqueIdentity { id: 911, rolls: vec![] });
            item.status.weavers_will = 255;
            item.fingerprint = item.legacy_fingerprint();
        }
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        command(&handle, Command::SetOverride(true)).await.unwrap();
        let plan = handle
            .snapshot()
            .status_edit_plan(StatusEdit {
                field: StatusField::WeaversWill,
                value: 80,
                unrestricted: true,
            })
            .unwrap();
        command(&handle, Command::Write(plan.into())).await.unwrap();
        assert!(handle.snapshot().undo_forge.is_none());
        assert!(matches!(handle.snapshot().last_write.as_ref().map(|r| &r.status), Some(WriteStatus::Confirmed(item)) if item.status.weavers_will == 80));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn status_family_uses_shared_write_outcomes_and_conditional_undo() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        for field in [eg_trainer::edits::StatusField::Corrupted, eg_trainer::edits::StatusField::Ruined] {
            let before = handle.snapshot();
            assert!(before.capabilities.status_fields[field as usize]);
            let plan = before.status_plan(field, 1).unwrap();
            command(&handle, Command::Write(plan.clone().into())).await.unwrap();
            assert_eq!(field.value(handle.snapshot().expected_observation().unwrap().snapshot()), 1);
            let undo = handle.snapshot().undo_forge.clone().unwrap();
            command(&handle, Command::UndoForge(Box::new(undo))).await.unwrap();
            assert_eq!(field.value(handle.snapshot().expected_observation().unwrap().snapshot()), 0);
            assert_eq!(handle.snapshot().generation, before.generation);
        }
        assert_eq!(shared.count("connect"), 1);
        assert_eq!(shared.count("write"), 4);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn generic_forge_write_publishes_known_different_state_without_undo_or_reconnect() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let original = handle.snapshot();
        let plan = original.level_requirement_plan(17).unwrap();
        let mut actual = plan.projected().clone();
        actual.status.level_requirement = 18;
        actual.name.push_str(" changed by the game");
        actual.fingerprint = actual.legacy_fingerprint();
        *shared.write_result.lock().unwrap() = Some(WriteStatus::NotKept(actual.clone()));
        let generic = WritePlan::from(plan);
        assert_eq!(command(&handle, Command::Write(generic.clone())).await.unwrap_err().kind, ErrorKind::WriteNotKept);
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::Ready);
        assert_eq!(state.generation, original.generation);
        assert_eq!(
            state.write_outcome(&generic),
            Some(WriteOutcome::NotKept(WriteObservation::Forge(Box::new(actual.clone()))))
        );
        assert!(matches!(state.forge, ForgeState::Item { snapshot, .. } if *snapshot == actual));
        assert!(state.undo_forge.is_none());
        let next = handle.snapshot().level_requirement_plan(16).unwrap();
        command(&handle, Command::Write(next.into())).await.unwrap();
        assert_eq!(shared.count("connect"), 1);
        assert_eq!(shared.count("write"), 2);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn ordinary_forge_refusals_keep_the_connection_but_runtime_refusals_release_it() {
    run(async {
        for code in [
            ResponseCode::NotReady,
            ResponseCode::StaleItem,
            ResponseCode::Refused,
            ResponseCode::OfflineRequired,
            ResponseCode::RestartRequired,
        ] {
            let shared = writable();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = handle.snapshot().level_requirement_plan(17).unwrap();
            *shared.write_result.lock().unwrap() = Some(WriteStatus::NotApplied {
                code,
                message: "trusted refusal".into(),
            });
            let error = command(&handle, Command::Write(plan.into())).await.unwrap_err();
            if matches!(code, ResponseCode::NotReady | ResponseCode::StaleItem | ResponseCode::Refused) {
                assert_eq!(error.kind, ErrorKind::WriteNotApplied);
                assert_eq!(handle.snapshot().phase, Phase::Ready);
                command(&handle, Command::ReadForge).await.unwrap();
                assert_eq!(shared.count("connect"), 1);
            } else {
                assert_eq!(
                    error.kind,
                    if code == ResponseCode::OfflineRequired {
                        ErrorKind::Gate
                    } else {
                        ErrorKind::RestartRequired
                    }
                );
                assert!(handle.snapshot().attached.is_none());
            }
            assert_eq!(shared.count("write"), 1);
            assert_eq!(shared.count("disconnect"), 0);
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn forge_confirmation_survives_later_gate_loss_without_publishing_a_live_item() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = WritePlan::from(handle.snapshot().level_requirement_plan(17).unwrap());
        shared.write_block.arm();
        let receipt = handle.try_submit(Command::Write(plan.clone())).unwrap();
        shared.write_block.wait_started().await;
        assert_eq!(handle.snapshot().write_outcome(&plan), Some(WriteOutcome::Pending));
        *shared.log.lock().unwrap() = log(GameReadiness::Online);
        shared.write_block.finish();
        assert_eq!(receipt.wait().await.unwrap_err().kind, ErrorKind::Gate);
        let state = handle.snapshot();
        assert_eq!(state.write_outcome(&plan), Some(WriteOutcome::Confirmed(plan.projected())));
        assert!(matches!(state.forge, ForgeState::Unobserved));
        assert!(state.undo_forge.is_none());
        assert_eq!(shared.count("write"), 1);
        assert_eq!(shared.count("disconnect"), 0);
        shutdown(&handle, worker).await;
    });
}

pub(super) fn writable() -> Arc<Shared> {
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
    shared
}
#[test]
fn only_explicit_write_is_submitted_and_full_confirmation_is_published() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let state = handle.snapshot();
        assert!(state.capabilities.level_requirement);
        assert!(!state.capabilities.write_forge);
        let plan = state.level_requirement_plan(17).unwrap();
        assert_eq!(shared.count("write"), 0);
        command(&handle, Command::ApplyForge(Box::new(plan.clone()))).await.unwrap();
        assert_eq!(handle.snapshot().last_write.unwrap().status, WriteStatus::Confirmed(plan.projected().clone()));
        assert!(matches!(handle.snapshot().forge, ForgeState::Item { snapshot, .. } if snapshot.as_ref() == plan.projected()));
        command(&handle, Command::ReadForge).await.unwrap();
        assert_eq!(shared.count("write"), 1);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn stale_full_observation_and_generation_are_refused_before_write_or_status_io() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        shared.forge_item.lock().unwrap().as_mut().unwrap().name.push_str(" changed");
        command(&handle, Command::ReadForge).await.unwrap();
        let refreshes = shared.count("refresh");
        assert_eq!(
            command(&handle, Command::ApplyForge(Box::new(plan.clone()))).await.unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
        assert_eq!(shared.count("refresh"), refreshes);
        command(&handle, Command::Disconnect).await.unwrap();
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("write"), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn manager_changed_during_fresh_status_revokes_session_before_submission() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        shared.status.lock().unwrap().manager = 0x5678;
        assert_eq!(command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap_err().kind, ErrorKind::Identity);
        assert_eq!(shared.count("write"), 0);
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(handle.snapshot().forge, ForgeState::Unobserved);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn cleanup_drains_submitted_write_and_retains_its_outcome_without_publishing_item() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        shared.write_block.arm();
        let write = handle.try_submit(Command::ApplyForge(Box::new(plan.clone()))).unwrap();
        shared.write_block.wait_started().await;
        assert_eq!(handle.snapshot().last_write.unwrap().status, WriteStatus::Pending);
        let disable = handle.try_submit(Command::Disable).unwrap();
        assert_eq!(shared.count("disconnect"), 0);
        shared.write_block.finish();
        assert_eq!(write.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disable.wait().await.unwrap();
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::Disabled);
        assert_eq!(state.forge, ForgeState::Unobserved);
        assert_eq!(state.last_write.unwrap().status, WriteStatus::Confirmed(plan.projected().clone()));
        assert_eq!(shared.maximum.load(Ordering::SeqCst), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        assert_eq!(shared.count("write"), 1);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn dropped_write_receipt_still_finishes_and_uncertainty_blocks_without_replay() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        *shared.write_result.lock().unwrap() = Some(WriteStatus::uncertain("lost reply"));
        shared.write_block.arm();
        drop(handle.try_submit(Command::ApplyForge(Box::new(plan))).unwrap());
        shared.write_block.wait_started().await;
        shared.write_block.finish();
        wait_phase(&handle, Phase::Blocked).await;
        assert!(matches!(handle.snapshot().last_write.unwrap().status, WriteStatus::Uncertain { .. }));
        assert_eq!(handle.snapshot().error.unwrap().delivery, Some(Delivery::MayHaveRun));
        command(&handle, Command::StartMonitoring).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(shared.count("write"), 1);
        command(&handle, Command::Retry).await.unwrap();
        assert_eq!(shared.count("write"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn mismatched_connection_confirmation_is_uncertain_even_if_legacy_hash_matches() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        let mut wrong = plan.projected().clone();
        wrong.name.push_str(" unexpected");
        assert_eq!(wrong.fingerprint, plan.projected().fingerprint);
        *shared.write_result.lock().unwrap() = Some(WriteStatus::Confirmed(wrong));
        assert_eq!(command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap_err().kind, ErrorKind::WriteUncertain);
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(shared.count("write"), 1);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn success_or_failure_code_cannot_be_reported_as_definite_level_nonexecution() {
    run(async {
        for code in [eg_game_access::protocol::ResponseCode::Done, eg_game_access::protocol::ResponseCode::Failed] {
            let shared = writable();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = handle.snapshot().level_requirement_plan(17).unwrap();
            *shared.write_result.lock().unwrap() = Some(WriteStatus::NotApplied {
                code,
                message: "adapter mislabeled a possibly executed operation".into(),
            });
            let error = command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap_err();
            assert_eq!(error.kind, ErrorKind::WriteUncertain);
            assert_eq!(error.delivery, Some(Delivery::MayHaveRun));
            let state = handle.snapshot();
            assert_eq!(state.phase, Phase::Blocked);
            assert!(matches!(state.last_write.unwrap().status, WriteStatus::Uncertain { .. }));
            assert_eq!(state.forge, ForgeState::Unobserved);
            assert!(state.undo_forge.is_none());
            command(&handle, Command::Retry).await.unwrap();
            assert_eq!(shared.count("write"), 1);
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn write_preflight_refresh_failure_releases_connection_instead_of_retaining_ready() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        shared
            .read_errors
            .lock()
            .unwrap()
            .push_back(BackendError::blocked(ErrorKind::Transport, "fresh status unavailable"));
        assert!(command(&handle, Command::ApplyForge(Box::new(plan))).await.is_err());
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert!(!handle.snapshot().capabilities.level_requirement);
        assert_eq!(shared.count("write"), 0);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn gate_or_not_ready_refresh_failure_revokes_old_ready_state_without_submitting() {
    run(async {
        for kind in [ErrorKind::Gate, ErrorKind::NotReady, ErrorKind::NotConnected] {
            let shared = writable();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let plan = handle.snapshot().level_requirement_plan(17).unwrap();
            shared.read_errors.lock().unwrap().push_back(BackendError::blocked(kind, "fresh status refused"));
            assert_eq!(command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap_err().kind, kind);
            let state = handle.snapshot();
            assert_eq!(state.phase, if kind == ErrorKind::Gate { Phase::WaitingForCharacter } else { Phase::Blocked });
            assert!(state.attached.is_none() && state.runtime_status.is_none());
            assert_eq!(state.forge, ForgeState::Unobserved);
            assert!(!state.capabilities.level_requirement);
            assert_eq!(shared.count("write"), 0);
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn explicit_undo_uses_a_fresh_reverse_plan_and_consumes_the_last_apply() {
    run(async {
        let shared = writable();
        let original = shared.forge_item.lock().unwrap().clone().unwrap();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap();
        let undo = handle.snapshot().undo_forge.unwrap();
        assert!(matches!(undo.reverse_edit(), eg_trainer::edits::ItemEdit::Status(edit) if edit.value == 20));
        let fresh = handle.snapshot().forge_undo_plan(&undo).unwrap();
        assert_eq!(fresh.before().snapshot().status.level_requirement, 17);
        assert_eq!(fresh.projected().status.level_requirement, 20);
        command(&handle, Command::UndoForge(Box::new(undo.clone()))).await.unwrap();
        assert!(matches!(handle.snapshot().forge, ForgeState::Item { snapshot, .. } if *snapshot == original));
        assert!(handle.snapshot().undo_forge.is_none());
        assert_eq!(shared.count("write"), 2);
        assert_eq!(command(&handle, Command::UndoForge(Box::new(undo))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("write"), 2);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn next_apply_replaces_undo_and_old_ui_intent_is_refused_before_io() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap();
        let old = handle.snapshot().undo_forge.unwrap();
        let plan = handle.snapshot().level_requirement_plan(19).unwrap();
        command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap();
        let refreshes = shared.count("refresh");
        assert_eq!(command(&handle, Command::UndoForge(Box::new(old))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("refresh"), refreshes);
        assert!(matches!(handle.snapshot().undo_forge.unwrap().reverse_edit(), eg_trainer::edits::ItemEdit::Status(edit) if edit.value == 17));
        assert_eq!(shared.count("write"), 2);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn observed_change_outside_legacy_hash_discards_undo_and_disconnect_cannot_reuse_it() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap();
        let undo = handle.snapshot().undo_forge.unwrap();
        shared.forge_item.lock().unwrap().as_mut().unwrap().name.push_str(" external drift");
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(handle.snapshot().undo_forge.is_none());
        assert_eq!(command(&handle, Command::UndoForge(Box::new(undo.clone()))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        command(&handle, Command::Disconnect).await.unwrap();
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(command(&handle, Command::UndoForge(Box::new(undo))).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("write"), 1);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn uncertain_undo_discards_its_token_and_releases_live_item_without_replay() {
    run(async {
        let shared = writable();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        command(&handle, Command::ApplyForge(Box::new(plan))).await.unwrap();
        let undo = handle.snapshot().undo_forge.unwrap();
        *shared.write_result.lock().unwrap() = Some(WriteStatus::uncertain("reverse readback unavailable"));
        assert_eq!(command(&handle, Command::UndoForge(Box::new(undo))).await.unwrap_err().kind, ErrorKind::WriteUncertain);
        let state = handle.snapshot();
        assert!(state.undo_forge.is_none());
        assert_eq!(state.forge, ForgeState::Unobserved);
        assert_eq!(state.phase, Phase::Blocked);
        assert_eq!(shared.count("write"), 2);
        shutdown(&handle, worker).await;
    });
}
