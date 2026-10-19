use super::*;
use eg_trainer::{
    catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    edits::{CatalogProvenance, EditError, ExpectedObservation},
    writes::{LevelRequirementPlan, WriteControl, WriteStatus},
};

fn plan(fixture: &Fixture) -> LevelRequirementPlan {
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
    let provenance = CatalogProvenance::RuntimeRead {
        generation: 1,
        process: fixture.target.clone(),
    };
    LevelRequirementPlan::new(&expected, 0x1234, &catalog, &provenance, 17).unwrap()
}
fn ready(fixture: &Fixture, wire: &FakeTransport, log: &FakeLog) -> Session<FakeTransport, FakeLog> {
    let mut session = fixture.session(wire, log, GatePolicy::Release);
    session.enable();
    run(session.connect(now())).unwrap();
    session
}
#[test]
fn explicit_write_requires_exact_readback_and_retains_confirmed_result_after_disconnect() {
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(
        f.target.clone(),
        [f.discover(), status(false, 0x1234, 0), done(forge::encode(Some(plan.projected())).unwrap()), clear()],
    );
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    assert_eq!(run(session.set_level_requirement(now(), &plan)).unwrap(), WriteStatus::Confirmed(plan.projected().clone()));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 3);
    assert!(matches!(&wire.requests()[2], Request::SetLevelRequirement(request) if request.expected() == plan.before().snapshot() && request.expected_manager() == 0x1234));
    run(session.disconnect());
    assert_eq!(session.last_write().unwrap().status, WriteStatus::Confirmed(plan.projected().clone()));
    assert_eq!(wire.requests().len(), 4);
}
#[test]
fn malformed_mismatched_or_failed_write_is_uncertain_and_never_replayed() {
    for fault in 0..5 {
        let f = Fixture::new();
        let plan = plan(&f);
        let mut changed = plan.projected().clone();
        changed.name.push_str(" changed outside legacy fingerprint");
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
        assert!(matches!(run(session.set_level_requirement(now(), &plan)).unwrap(), WriteStatus::Uncertain { .. }));
        assert_eq!(session.state(), SessionState::Blocked);
        assert!(run(session.set_level_requirement(now(), &plan)).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
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
        let result = run(session.set_level_requirement(now(), &plan)).unwrap();
        assert_eq!(matches!(result, WriteStatus::Uncertain { .. }), fault == 1);
        assert!(run(session.set_level_requirement(now(), &plan)).is_err());
        assert_eq!(wire.requests().len(), 3);
    }
}
#[test]
fn successful_reply_after_log_or_artifact_loss_is_uncertain_without_more_process_io() {
    for change_file in [false, true] {
        let f = Fixture::new();
        let plan = plan(&f);
        let log = FakeLog::new([offline()]);
        let changed_log = log.clone();
        let metadata = f.metadata.clone();
        let wire = FakeTransport::new(
            f.target.clone(),
            [
                f.discover(),
                status(false, 0x1234, 0),
                Step::Action(
                    Box::new(move || {
                        if change_file {
                            fs::write(metadata, b"changed while write was pending").unwrap();
                        } else {
                            changed_log.set(online());
                        }
                    }),
                    Response {
                        code: ResponseCode::Done,
                        payload: forge::encode(Some(plan.projected())).unwrap(),
                    },
                ),
            ],
        );
        let mut session = ready(&f, &wire, &log);
        assert!(matches!(run(session.set_level_requirement(now(), &plan)).unwrap(), WriteStatus::Uncertain { .. }));
        assert_ne!(session.state(), SessionState::Ready);
        assert_eq!(wire.requests().len(), 3);
    }
}
#[test]
fn preflight_refuses_wrong_runtime_manager_or_artifacts_without_write_submission() {
    for fault in 0..3 {
        let f = Fixture::new();
        let plan = if fault == 0 { plan(&Fixture::new()) } else { plan(&f) };
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, if fault == 1 { 0x5678 } else { 0x1234 }, 0)]);
        let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
        if fault == 2 {
            fs::write(&f.metadata, b"changed before write").unwrap();
        }
        assert!(matches!(run(session.set_level_requirement(now(), &plan)), Err(SessionError::Identity(_))));
        assert_eq!(wire.requests().len(), 2);
        assert!(session.last_write().is_none());
    }
}
#[test]
fn development_probe_permission_cannot_write_and_disabled_session_does_not_read_log() {
    let f = Fixture::new();
    let plan = plan(&f);
    let log = FakeLog::new([GameLogState::missing()]);
    let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0)]);
    let mut session = f.session(&wire, &log, GatePolicy::Development);
    assert!(matches!(run(session.set_level_requirement(now(), &plan)), Err(SessionError::Disabled)));
    assert!(log.reads().is_empty());
    session.enable();
    run(session.connect(now())).unwrap();
    assert!(matches!(run(session.set_level_requirement(now(), &plan)), Err(SessionError::Gate(_))));
    assert_eq!(wire.requests().len(), 2);
}
#[test]
fn abandoned_write_remains_pending_and_busy_until_cleanup_records_uncertainty() {
    let f = Fixture::new();
    let plan = plan(&f);
    let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), Step::Pending, clear()]);
    let mut session = ready(&f, &wire, &FakeLog::new([offline()]));
    let mut pending = Box::pin(session.set_level_requirement(now(), &plan));
    assert!(pending.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(pending);
    assert_eq!(session.last_write().unwrap().status, WriteStatus::Pending);
    assert!(matches!(run(session.set_level_requirement(now(), &plan)), Err(SessionError::Busy)));
    assert!(matches!(run(session.read_forge(now())), Err(SessionError::Busy)));
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
    assert!(matches!(run(session.set_level_requirement_controlled(now(), &plan, &mut Stop)), Err(SessionError::Busy)));
    assert_eq!(session.state(), SessionState::Ready);
    assert!(session.last_write().is_none());
    assert_eq!(wire.requests().len(), 2);
}
#[test]
fn plans_recheck_full_observation_and_manager_including_fields_outside_legacy_hash() {
    let f = Fixture::new();
    let plan = plan(&f);
    assert_eq!(plan.recheck(plan.before(), 0x5678), Err(EditError::IdentityChanged));
    let mut changed = plan.before().snapshot().clone();
    changed.name.push_str(" drift");
    assert_eq!(changed.fingerprint, plan.before().snapshot().fingerprint);
    let expected = ExpectedObservation::new(1, f.target, plan.before().catalog_identity().clone(), changed).unwrap();
    assert_eq!(plan.recheck(&expected, 0x1234), Err(EditError::StaleItem));
}
