use super::*;
use eg_trainer::{
    catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    edits::{EditError, ExpectedObservation, ItemEdit, StatusEdit, StatusField},
    writes::{ForgePlan, WriteControl, WriteObservation, WriteOutcome, WriteStatus},
};

fn plan(fixture: &Fixture) -> ForgePlan {
    plan_with_version(fixture, EXTRACTION_VERSION)
}
fn plan_with_version(fixture: &Fixture, extraction_version: u32) -> ForgePlan {
    let profile = fixture.profile();
    let catalog = GameCatalog {
        identity: CatalogIdentity {
            binary_hash: profile.binary_hash().into(),
            metadata_hash: profile.metadata_hash().into(),
            locale: GameLocale {
                language: "en".into(),
                game_version: "authored".into(),
            },
            extraction_version,
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
fn ready(fixture: &Fixture, wire: &FakeTransport, log: &FakeLog) -> Session<FakeTransport> {
    let mut session = fixture.session(wire, log, GatePolicy::Release);
    session.enable();
    run(session.connect()).unwrap();
    session
}
#[test]
fn obsolete_catalog_write_is_refused_before_submission_without_recording_a_write() {
    let f = Fixture::new();
    let plan = plan_with_version(&f, EXTRACTION_VERSION + 1);
    let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0)]);
    let mut s = ready(&f, &wire, &FakeLog::new([offline()]));
    assert!(matches!(run(s.write(&plan.into())), Err(SessionError::Identity(_))));
    assert_eq!(wire.requests().len(), 2);
    assert!(s.last_write().is_none());
    assert_eq!(s.state(), SessionState::Ready);
}
#[test]
fn explicit_write_requires_exact_readback_and_retains_confirmed_result_after_disconnect() {
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(
        f.target.clone(),
        [
            f.discover(),
            status(false, 0x1234, 0),
            done(forge::encode(Some(plan.projected())).unwrap()),
            done(forge::encode(Some(plan.projected())).unwrap()),
            clear(),
        ],
    );
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    assert_eq!(
        run(session.write(&plan.clone().into())).unwrap(),
        WriteOutcome::Confirmed(WriteObservation::Forge(Box::new(plan.projected().clone())))
    );
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 4);
    assert!(matches!(&wire.requests()[2], Request::EditForge(request) if request.expected() == plan.before().snapshot() && request.expected_manager() == 0x1234));
    run(session.disconnect());
    assert_eq!(session.last_write().unwrap().status, WriteStatus::Confirmed(plan.projected().clone()));
    assert_eq!(wire.requests().len(), 5);
}
#[test]
fn readback_of_a_replaced_forge_item_is_unknown_even_with_equal_values() {
    for changed_address in [false, true] {
        let f = Fixture::new();
        let plan = plan(&f);
        let mut actual = plan.projected().clone();
        if changed_address {
            actual.item += 1;
        } else {
            actual.individual_id += 1;
        }
        let wire = FakeTransport::new(
            f.target.clone(),
            [
                f.discover(),
                status(false, 0x1234, 0),
                done(forge::encode(Some(plan.projected())).unwrap()),
                done(forge::encode(Some(&actual)).unwrap()),
            ],
        );
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        assert!(matches!(run(session.write(&plan.into())).unwrap(), WriteOutcome::Uncertain { .. }));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(wire.requests().len(), 4);
    }
}
#[test]
fn malformed_replaced_or_failed_write_is_uncertain_and_never_replayed() {
    for fault in 0..5 {
        let f = Fixture::new();
        let plan = plan(&f);
        let mut changed = plan.projected().clone();
        changed.item += 1;
        let response = match fault {
            0 => Response {
                code: ResponseCode::Done,
                payload: vec![1],
            },
            1 => Response {
                code: ResponseCode::Done,
                payload: forge::encode(None).unwrap(),
            },
            2 => Response {
                code: ResponseCode::Done,
                payload: forge::encode(Some(&changed)).unwrap(),
            },
            3 => Response {
                code: ResponseCode::Failed,
                payload: b"store attempted".to_vec(),
            },
            _ => Response {
                code: ResponseCode::Refused,
                payload: vec![0xff],
            },
        };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), Step::Reply(Ok(response))]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        assert!(matches!(run(session.edit_forge(&plan)).unwrap(), WriteStatus::Uncertain { .. }));
        assert_eq!(session.state(), SessionState::Disconnected);
        assert!(run(session.edit_forge(&plan)).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
}

#[test]
fn differing_native_ack_still_requires_independent_observation() {
    let f = Fixture::new();
    let plan = plan(&f);
    let unchanged = plan.before().snapshot().clone();
    let wire = FakeTransport::new(
        f.target.clone(),
        [
            f.discover(),
            status(false, 0x1234, 0),
            done(forge::encode(Some(&unchanged)).unwrap()),
            done(forge::encode(Some(&unchanged)).unwrap()),
        ],
    );
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    assert_eq!(run(session.edit_forge(&plan)).unwrap(), WriteStatus::NotKept(unchanged));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 4);
    assert!(matches!(wire.requests()[3], Request::ReadForge { .. }));
}
#[test]
fn delivery_and_trusted_prestore_refusals_preserve_certainty_without_retry() {
    for fault in 0..7 {
        let f = Fixture::new();
        let plan = plan(&f);
        let step = if fault < 2 {
            Step::Reply(Err(TransportError {
                delivery: if fault == 0 { Delivery::NotSent } else { Delivery::MayHaveRun },
                failure: TransportFailure::Timeout,
            }))
        } else {
            Step::Reply(Ok(Response {
                code: [
                    ResponseCode::NotReady,
                    ResponseCode::StaleItem,
                    ResponseCode::Refused,
                    ResponseCode::OfflineRequired,
                    ResponseCode::RestartRequired,
                ][fault - 2],
                payload: b"pre-store refusal".to_vec(),
            }))
        };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), step]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        let result = run(session.edit_forge(&plan)).unwrap();
        assert_eq!(matches!(result, WriteStatus::Uncertain { .. }), fault == 1);
        if (2..=4).contains(&fault) {
            assert_eq!(session.state(), SessionState::Ready);
            wire.push(done(forge::encode(Some(plan.before().snapshot())).unwrap()));
            assert!(run(session.read_forge()).unwrap().is_some());
            assert_eq!(wire.requests().len(), 4);
        } else {
            assert!(run(session.edit_forge(&plan)).is_err());
            assert_eq!(wire.requests().len(), 3);
        }
        assert_eq!(wire.requests().iter().filter(|r| matches!(r, Request::EditForge(_))).count(), 1);
    }
}
#[test]
fn successful_reply_after_artifact_loss_is_uncertain_without_more_process_io() {
    for change_file in [false, true] {
        let f = Fixture::rehashing();
        let plan = plan(&f);
        let log = FakeLog::new([offline()]);
        let metadata = if change_file { f.binary.clone() } else { f.metadata.clone() };
        let wire = FakeTransport::new(
            f.target.clone(),
            [
                f.discover(),
                status(false, 0x1234, 0),
                Step::Action(
                    Box::new(move || {
                        fs::write(metadata, b"changed while write was pending").unwrap();
                    }),
                    Response {
                        code: ResponseCode::Done,
                        payload: forge::encode(Some(plan.projected())).unwrap(),
                    },
                ),
            ],
        );
        let mut session = ready(&f, &wire, &log);
        assert!(matches!(run(session.edit_forge(&plan)).unwrap(), WriteStatus::Uncertain { .. }));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(wire.requests().len(), 3);
    }
}
#[test]
fn preflight_refuses_wrong_runtime_manager_or_artifacts_without_write_submission() {
    for fault in 0..3 {
        let f = Fixture::rehashing();
        let plan = if fault == 0 { plan(&Fixture::rehashing()) } else { plan(&f) };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, if fault == 1 { 0x5678 } else { 0x1234 }, 0)]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        if fault == 2 {
            fs::write(&f.metadata, b"changed before write").unwrap();
        }
        assert!(matches!(run(session.edit_forge(&plan)), Err(SessionError::Identity(_))));
        assert_eq!(wire.requests().len(), 2);
        assert!(session.last_write().is_none());
    }
}

#[test]
fn abandoned_write_remains_pending_and_busy_until_cleanup_records_uncertainty() {
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), Step::Pending, clear()]);
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    let mut pending = Box::pin(session.edit_forge(&plan));
    assert!(pending.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(pending);
    assert_eq!(session.last_write().unwrap().status, WriteStatus::Pending);
    assert!(matches!(run(session.edit_forge(&plan)), Err(SessionError::Busy)));
    assert!(matches!(run(session.read_forge()), Err(SessionError::Busy)));
    assert_eq!(wire.requests().len(), 3);
    run(session.disconnect());
    assert!(matches!(session.last_write().unwrap().status, WriteStatus::Uncertain { .. }));
    assert_eq!(wire.requests().len(), 4);
}
#[test]
fn cleanup_control_can_stop_validated_write_before_transport_is_entered() {
    struct Stop;
    impl WriteControl for Stop {
        fn allowed(&mut self) -> bool {
            false
        }
    }
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0)]);
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    assert!(matches!(run(session.edit_forge_controlled(&plan, &mut Stop)), Err(SessionError::Busy)));
    assert_eq!(session.state(), SessionState::Ready);
    assert!(session.last_write().is_none());
    assert_eq!(wire.requests().len(), 2);
}
#[test]
fn plans_recheck_full_observation_and_manager_including_fields_outside_legacy_hash() {
    let f = Fixture::new();
    let plan = plan(&f);
    assert_eq!(plan.recheck(plan.before(), 0x5678, false), Err(EditError::IdentityChanged));
    let mut changed = plan.before().snapshot().clone();
    changed.name.push_str(" drift");
    assert_eq!(changed.fingerprint, plan.before().snapshot().fingerprint);
    let expected = ExpectedObservation::new(1, f.target, plan.before().catalog_identity().clone(), changed).unwrap();
    assert_eq!(plan.recheck(&expected, 0x1234, false), Err(EditError::StaleItem));
}

#[test]
fn separate_readback_retains_known_drift_but_malformed_missing_or_failed_reads_are_uncertain() {
    for fault in 0..11 {
        let f = Fixture::new();
        let plan = plan(&f);
        let mut changed = plan.projected().clone();
        changed.name.push_str(" next-frame change");
        let last = match fault {
            0 => done(vec![1]),
            1 => done(forge::encode(None).unwrap()),
            2 => done(forge::encode(Some(&changed)).unwrap()),
            3 | 4 => Step::Reply(Err(TransportError {
                delivery: if fault == 3 { Delivery::NotSent } else { Delivery::MayHaveRun },
                failure: TransportFailure::Timeout,
            })),
            _ => Step::Reply(Ok(Response {
                code: [
                    ResponseCode::Failed,
                    ResponseCode::NotReady,
                    ResponseCode::StaleItem,
                    ResponseCode::Refused,
                    ResponseCode::OfflineRequired,
                    ResponseCode::RestartRequired,
                ][fault - 5],
                payload: b"readback refused after prior write".to_vec(),
            })),
        };
        let wire = FakeTransport::new(
            f.target.clone(),
            [f.discover(), status(false, 0x1234, 0), done(forge::encode(Some(plan.projected())).unwrap()), last],
        );
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        let result = run(session.edit_forge(&plan)).unwrap();
        if fault == 2 {
            assert_eq!(result, WriteStatus::NotKept(changed));
            assert_eq!(session.state(), SessionState::Ready);
        } else {
            assert!(matches!(result, WriteStatus::Uncertain { .. }));
            assert!(run(session.edit_forge(&plan)).is_err());
        }
        assert!(matches!(wire.requests()[3], Request::ReadForge { .. }));
        assert_eq!(wire.requests().len(), 4);
    }
}
#[test]
fn separate_readback_cannot_confirm_after_either_artifact_changes() {
    for change_file in [false, true] {
        let f = Fixture::rehashing();
        let plan = plan(&f);
        let log = FakeLog::new([offline()]);
        let metadata = if change_file { f.binary.clone() } else { f.metadata.clone() };
        let reply = forge::encode(Some(plan.projected())).unwrap();
        let wire = FakeTransport::new(
            f.target.clone(),
            [
                f.discover(),
                status(false, 0x1234, 0),
                done(reply.clone()),
                Step::Action(
                    Box::new(move || {
                        fs::write(metadata, b"changed during separate readback").unwrap();
                    }),
                    Response {
                        code: ResponseCode::Done,
                        payload: reply,
                    },
                ),
            ],
        );
        let mut session = ready(&f, &wire, &log);
        assert!(matches!(run(session.edit_forge(&plan)).unwrap(), WriteStatus::Uncertain { .. }));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(wire.requests().len(), 4);
    }
}
#[test]
fn native_ack_does_not_clear_pending_while_the_separate_read_is_in_flight() {
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(
        f.target.clone(),
        [f.discover(), status(false, 0x1234, 0), done(forge::encode(Some(plan.projected())).unwrap()), Step::Pending],
    );
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    let mut pending = Box::pin(session.edit_forge(&plan));
    assert!(pending.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(pending);
    assert_eq!(session.last_write().unwrap().status, WriteStatus::Pending);
    assert_eq!(wire.requests().len(), 4);
    assert!(matches!(run(session.edit_forge(&plan)), Err(SessionError::Busy)));
    wire.push(clear());
    run(session.disconnect());
    assert!(matches!(session.last_write().unwrap().status, WriteStatus::Uncertain { .. }));
    assert_eq!(wire.requests().len(), 5);
}
#[test]
fn cleanup_after_native_ack_stops_before_separate_read_without_claiming_no_store() {
    struct StopAfterFirst(bool);
    impl WriteControl for StopAfterFirst {
        fn allowed(&mut self) -> bool {
            std::mem::replace(&mut self.0, false)
        }
    }
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(
        f.target.clone(),
        [f.discover(), status(false, 0x1234, 0), done(forge::encode(Some(plan.projected())).unwrap())],
    );
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    assert!(matches!(
        run(session.edit_forge_controlled(&plan, &mut StopAfterFirst(true))).unwrap(),
        WriteStatus::Uncertain { .. }
    ));
    assert_eq!(wire.requests().len(), 3);
}
