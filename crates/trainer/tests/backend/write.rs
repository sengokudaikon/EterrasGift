use super::*;

fn writable() -> Arc<Shared> {
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
        command(&handle, Command::ApplyLevelRequirement(Box::new(plan.clone()))).await.unwrap();
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
            command(&handle, Command::ApplyLevelRequirement(Box::new(plan.clone()))).await.unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
        assert_eq!(shared.count("refresh"), refreshes);
        command(&handle, Command::Disconnect).await.unwrap();
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(
            command(&handle, Command::ApplyLevelRequirement(Box::new(plan))).await.unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
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
        assert_eq!(
            command(&handle, Command::ApplyLevelRequirement(Box::new(plan))).await.unwrap_err().kind,
            ErrorKind::Identity
        );
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
        let write = handle.try_submit(Command::ApplyLevelRequirement(Box::new(plan.clone()))).unwrap();
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
        drop(handle.try_submit(Command::ApplyLevelRequirement(Box::new(plan))).unwrap());
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
        assert_eq!(
            command(&handle, Command::ApplyLevelRequirement(Box::new(plan))).await.unwrap_err().kind,
            ErrorKind::WriteUncertain
        );
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(shared.count("write"), 1);
        shutdown(&handle, worker).await;
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
        assert!(command(&handle, Command::ApplyLevelRequirement(Box::new(plan))).await.is_err());
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
            assert_eq!(command(&handle, Command::ApplyLevelRequirement(Box::new(plan))).await.unwrap_err().kind, kind);
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
