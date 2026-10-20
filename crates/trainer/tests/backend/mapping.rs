use super::*;
use eg_trainer::{
    edits::ExpectedObservation,
    mapping::{CapturedDisplayParser, MappingBuilder, MappingRequest, ValueTable},
};
use eg_trainer_wire::mapping::{MappingAddress, MappingStart, MappingTarget, RawMappingPage, RawValueSample};

pub(super) fn table(expected: &ExpectedObservation, request: &MappingRequest) -> Result<ValueTable, BackendError> {
    let page = |start| RawMappingPage {
        language: expected.catalog_identity().locale.language.clone(),
        context: vec![1, 2, 3],
        address: request.address,
        start,
        samples: match start {
            MappingStart::ContextOnly => vec![],
            MappingStart::Samples(start) => (u16::from(start)..u16::from(start) + 16)
                .map(|roll| RawValueSample {
                    raw_quantity: f64::from(roll) / 100.,
                    display: format!("{}%", 1000 + roll),
                    unit: "%".into(),
                    effects: vec![format!("Effect {roll}")],
                })
                .collect(),
        },
    };
    let mut builder = MappingBuilder::new(expected.clone(), request.clone(), page(MappingStart::ContextOnly), None).unwrap();
    let mut parser = CapturedDisplayParser::new(&expected.catalog_identity().locale.language).unwrap();
    for start in (0..=240).step_by(16) {
        builder.push(expected, request, page(MappingStart::Samples(start)), &mut parser).unwrap();
    }
    Ok(builder.finish().unwrap())
}
fn request() -> MappingRequest {
    MappingRequest {
        address: MappingAddress {
            target: MappingTarget::Affix,
            index: 0,
            line: None,
        },
        draft: None,
    }
}
fn command_for(snapshot: &backend::BackendSnapshot) -> Command {
    Command::ResolveMapping {
        expected: Box::new(snapshot.expected_observation().unwrap()),
        request: request(),
    }
}
fn shared() -> Arc<Shared> {
    let shared = writes::writable();
    {
        let mut item = shared.forge_item.lock().unwrap();
        let item = item.as_mut().unwrap();
        item.affixes.push(backend::AffixSnapshot {
            slot: 0,
            id: 1,
            tier: 1,
            roll: 100,
            kind: 0,
            seal: 0,
            name: "Health".into(),
            special: 0,
        });
        item.fingerprint = item.legacy_fingerprint();
    }
    shared
}
#[test]
fn optional_mapping_is_observation_bound_and_never_changes_write_or_undo_history() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let plan = handle.snapshot().level_requirement_plan(17).unwrap();
        command(&handle, Command::Write(plan.into())).await.unwrap();
        let before = handle.snapshot();
        command(&handle, command_for(&before)).await.unwrap();
        let state = handle.snapshot();
        let result = state.mapping(&before.expected_observation().unwrap(), &request()).unwrap().as_ref().unwrap();
        assert_eq!(result.sample(100).display, "1100%");
        assert_eq!(state.last_write, before.last_write);
        assert_eq!(state.undo_forge, before.undo_forge);
        assert_eq!(state.forge, before.forge);
        assert_eq!(state.generation, before.generation);
        command(&handle, command_for(&state)).await.unwrap();
        assert_eq!(shared.count("mapping"), 1, "same complete observation/request reuses an in-memory result");
        command(&handle, Command::ReadForge).await.unwrap();
        assert_eq!(handle.snapshot().forge_read_sequence, state.forge_read_sequence + 1);
        assert_eq!(handle.snapshot().mappings, state.mappings, "equal-item refresh preserves complete successful tables");
        *shared.forge_item.lock().unwrap() = None;
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(handle.snapshot().mappings.is_empty());
        assert_eq!(command(&handle, command_for(&before)).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("mapping"), 1);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn local_mapping_unavailability_is_memoized_but_transport_and_protocol_failures_release() {
    run(async {
        for kind in [
            ErrorKind::MappingUnavailable,
            ErrorKind::MappingRetryable,
            ErrorKind::Transport,
            ErrorKind::Protocol,
            ErrorKind::Gate,
        ] {
            let shared = shared();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            shared.mapping_errors.lock().unwrap().push_back(BackendError::blocked(kind, "mapping fault"));
            let before = handle.snapshot();
            let result = command(&handle, command_for(&before)).await;
            let after = handle.snapshot();
            if matches!(kind, ErrorKind::MappingUnavailable | ErrorKind::MappingRetryable) {
                result.unwrap();
                assert_eq!(after.phase, Phase::Ready);
                assert_eq!(after.generation, before.generation);
                assert_eq!(after.error, before.error);
                assert_eq!(after.mapping(&before.expected_observation().unwrap(), &request()), Some(&Err("mapping fault".into())));
                assert_eq!(after.mappings[0].retry_on_read, kind == ErrorKind::MappingRetryable);
                command(&handle, command_for(&before)).await.unwrap();
                assert_eq!(shared.count("mapping"), 1);
                command(&handle, Command::ReadForge).await.unwrap();
                assert_eq!(handle.snapshot().forge_read_sequence, after.forge_read_sequence + 1);
                assert!(handle.snapshot().mappings.is_empty(), "a successful refresh permits another optional read attempt");
                command(&handle, command_for(&handle.snapshot())).await.unwrap();
                assert_eq!(shared.count("mapping"), 2);
                assert!(handle.snapshot().mappings[0].result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().kind, kind);
                assert!(after.attached.is_none());
                assert!(after.mappings.is_empty());
                assert!(after.generation > before.generation);
            }
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn mapping_from_another_observation_revokes_the_connection_and_is_never_published() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        let before = handle.snapshot();
        let expected = before.expected_observation().unwrap();
        let other = ExpectedObservation::new(
            expected.generation() + 1,
            expected.process().clone(),
            expected.catalog_identity().clone(),
            expected.snapshot().clone(),
        )
        .unwrap();
        *shared.mapping_result.lock().unwrap() = Some(table(&other, &request()).unwrap());
        let error = command(&handle, command_for(&before)).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Protocol);
        let after = handle.snapshot();
        assert!(after.attached.is_none());
        assert!(after.mappings.is_empty());
        assert!(after.generation > before.generation);
        shutdown(&handle, worker).await;
    });
}
async fn wait_for_read(handle: &BackendHandle, after: u64) {
    let mut snapshots = handle.subscribe();
    while snapshots.borrow().forge_read_sequence <= after {
        snapshots.changed().await.unwrap();
    }
}
#[test]
fn monitor_polls_keep_stable_mapping_failures_until_explicit_refresh() {
    run(async {
        for reason in ["Fixed value", "Non-monotonic display", "Game refused this numeric capability"] {
            let shared = shared();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            shared
                .mapping_errors
                .lock()
                .unwrap()
                .push_back(BackendError::blocked(ErrorKind::MappingUnavailable, reason));
            command(&handle, command_for(&handle.snapshot())).await.unwrap();
            let initial = handle.snapshot();
            command(&handle, Command::StartMonitoring).await.unwrap();
            for _ in 0..3 {
                let sequence = handle.snapshot().forge_read_sequence;
                wait_for_read(&handle, sequence).await;
                assert_eq!(handle.snapshot().mapping(&initial.expected_observation().unwrap(), &request()), Some(&Err(reason.into())));
                command(&handle, command_for(&handle.snapshot())).await.unwrap();
            }
            assert_eq!(shared.count("mapping"), 1, "monitoring must not resample a known unavailable table");
            assert!(shared.count("forge") >= 4);
            command(&handle, Command::ReadForge).await.unwrap();
            assert!(handle.snapshot().mappings.is_empty());
            command(&handle, command_for(&handle.snapshot())).await.unwrap();
            assert_eq!(shared.count("mapping"), 2, "explicit Refresh permits a new capability attempt");
            assert!(handle.snapshot().mappings[0].result.is_ok());
            assert_eq!(shared.count("connect"), 1);
            assert_eq!(shared.count("write"), 0);
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn transient_mapping_failure_retries_only_after_a_successful_owned_monitor_read() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        shared
            .mapping_errors
            .lock()
            .unwrap()
            .push_back(BackendError::blocked(ErrorKind::MappingRetryable, "formatter not ready"));
        command(&handle, command_for(&handle.snapshot())).await.unwrap();
        let failed = handle.snapshot();
        assert!(failed.mappings[0].retry_on_read);
        command(&handle, command_for(&failed)).await.unwrap();
        assert_eq!(shared.count("mapping"), 1, "unrelated commands and lookup repetitions cannot immediately retry");
        command(&handle, Command::SetOverride(false)).await.unwrap();
        assert_eq!(handle.snapshot().mappings, failed.mappings);
        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_for_read(&handle, failed.forge_read_sequence).await;
        assert!(handle.snapshot().mappings.is_empty());
        command(&handle, command_for(&handle.snapshot())).await.unwrap();
        assert_eq!(shared.count("mapping"), 2);
        assert!(handle.snapshot().mappings[0].result.is_ok());
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        assert_eq!(handle.snapshot().generation, failed.generation);
        assert_eq!(shared.count("connect"), 1);
        shutdown(&handle, worker).await;
    });
}
#[test]
fn cleanup_waits_for_mapping_but_suppresses_old_publication_and_receipt_disposal_does_not_cancel() {
    run(async {
        for cleanup in [true, false] {
            let shared = shared();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            shared.mapping_block.arm();
            let pending = handle.try_submit(command_for(&handle.snapshot())).unwrap();
            shared.mapping_block.wait_started().await;
            assert!(handle.snapshot().mappings.is_empty());
            assert!(handle.snapshot().capabilities.level_requirement);
            if cleanup {
                let disconnect = handle.try_submit(Command::Disconnect).unwrap();
                assert_eq!(shared.count("disconnect"), 0);
                shared.mapping_block.finish();
                assert_eq!(pending.wait().await.unwrap_err().kind, ErrorKind::Superseded);
                disconnect.wait().await.unwrap();
                assert!(handle.snapshot().mappings.is_empty());
                assert!(handle.snapshot().attached.is_none());
            } else {
                drop(pending);
                shared.mapping_block.finish();
                command(&handle, Command::SetOverride(false)).await.unwrap();
                assert_eq!(handle.snapshot().mappings.len(), 1);
                assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
            }
            assert_eq!(shared.maximum.load(Ordering::SeqCst), 1);
            shutdown(&handle, worker).await;
        }
    });
}
#[test]
fn mapping_result_cannot_reopen_an_offline_gate_lost_while_sampling() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        shared.mapping_block.arm();
        let pending = handle.try_submit(command_for(&handle.snapshot())).unwrap();
        shared.mapping_block.wait_started().await;
        *shared.log.lock().unwrap() = log(GameReadiness::Online);
        shared.mapping_block.finish();
        assert_eq!(pending.wait().await.unwrap_err().kind, ErrorKind::Gate);
        assert!(handle.snapshot().mappings.is_empty());
        assert!(handle.snapshot().attached.is_none());
        assert_eq!(shared.count("disconnect"), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn language_change_invalidates_successes_and_failures_before_cached_lookup() {
    run(async {
        for cached_failure in [false, true] {
            let shared = shared();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            if cached_failure {
                shared
                    .mapping_errors
                    .lock()
                    .unwrap()
                    .push_back(BackendError::blocked(ErrorKind::MappingUnavailable, "old language failure"));
            }
            let old = handle.snapshot();
            command(&handle, command_for(&old)).await.unwrap();
            assert_eq!(handle.snapshot().mappings.len(), 1);
            shared.locale.lock().unwrap().language = "fr".into();
            assert_eq!(command(&handle, command_for(&old)).await.unwrap_err().kind, ErrorKind::MappingRetryable);
            let fresh = handle.snapshot();
            assert_eq!(fresh.phase, Phase::Ready);
            assert_eq!(fresh.generation, old.generation);
            assert_eq!(fresh.expected_observation().unwrap().catalog_identity().locale.language, "fr");
            assert!(fresh.mappings.is_empty());
            command(&handle, command_for(&fresh)).await.unwrap();
            assert!(handle.snapshot().mappings[0].result.is_ok());
            assert_eq!(shared.count("mapping"), 2);
            assert_eq!(shared.count("connect"), 1);
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn monitor_refreshes_locale_even_when_no_new_value_requests_are_submitted() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        command(&handle, command_for(&handle.snapshot())).await.unwrap();
        let old = handle.snapshot();
        shared.locale.lock().unwrap().language = "fr".into();
        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_for_read(&handle, old.forge_read_sequence).await;
        let fresh = handle.snapshot();
        assert_eq!(fresh.expected_observation().unwrap().catalog_identity().locale.language, "fr");
        assert!(fresh.mappings.is_empty());
        assert_eq!(shared.count("mapping"), 1);
        assert_eq!(shared.count("connect"), 1);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn context_change_mid_sample_refreshes_locale_without_caching_the_old_failure() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        shared.mapping_block.arm();
        shared
            .mapping_errors
            .lock()
            .unwrap()
            .push_back(BackendError::blocked(ErrorKind::MappingRetryable, "WrongContext"));
        let old = handle.snapshot();
        let pending = handle.try_submit(command_for(&old)).unwrap();
        shared.mapping_block.wait_started().await;
        shared.locale.lock().unwrap().language = "fr".into();
        shared.mapping_block.finish();
        assert_eq!(pending.wait().await.unwrap_err().kind, ErrorKind::MappingRetryable);
        let fresh = handle.snapshot();
        assert!(fresh.mappings.is_empty());
        assert_eq!(fresh.phase, Phase::Ready);
        assert_eq!(fresh.expected_observation().unwrap().catalog_identity().locale.language, "fr");
        command(&handle, command_for(&fresh)).await.unwrap();
        assert!(handle.snapshot().mappings[0].result.is_ok());
        shutdown(&handle, worker).await;
    });
}

#[test]
fn disposable_mapping_receipts_cancel_queued_and_running_reads_without_canceling_writes() {
    run(async {
        for queued in [false, true] {
            let shared = shared();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            let expected = handle.snapshot().expected_observation().unwrap();
            shared.mapping_block.arm();
            let first = handle.try_resolve_mapping(expected.clone(), request()).unwrap();
            shared.mapping_block.wait_started().await;
            if queued {
                let second = handle.try_resolve_mapping(expected, request()).unwrap();
                drop(second);
            }
            drop(first);
            shared.mapping_block.finish();
            command(&handle, Command::SetOverride(false)).await.unwrap();
            assert!(handle.snapshot().mappings.is_empty());
            assert_eq!(shared.count("mapping"), 1, "a canceled queued read never reaches the connection");
            assert_eq!(handle.snapshot().phase, Phase::Ready);
            assert_eq!(shared.aborted.load(Ordering::SeqCst), 0, "in-flight I/O drained normally");
            let plan = handle.snapshot().level_requirement_plan(17).unwrap();
            drop(handle.try_submit(Command::Write(plan.into())).unwrap());
            command(&handle, Command::SetOverride(false)).await.unwrap();
            assert_eq!(shared.count("write"), 1, "dropping an ordinary write receipt still cannot cancel it");
            assert!(matches!(handle.snapshot().last_write.as_ref().unwrap().status, WriteStatus::Confirmed(_)));
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn canceling_a_value_request_does_not_hide_fatal_connection_errors() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        shared
            .mapping_errors
            .lock()
            .unwrap()
            .push_back(BackendError::blocked(ErrorKind::Protocol, "malformed page"));
        shared.mapping_block.arm();
        let read = handle.try_resolve_mapping(handle.snapshot().expected_observation().unwrap(), request()).unwrap();
        shared.mapping_block.wait_started().await;
        drop(read);
        shared.mapping_block.finish();
        command(&handle, Command::SetOverride(false)).await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(handle.snapshot().error.as_ref().unwrap().kind, ErrorKind::Protocol);
        assert!(handle.snapshot().attached.is_none());
        shutdown(&handle, worker).await;
    });
}
