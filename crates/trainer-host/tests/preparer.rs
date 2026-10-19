#![cfg(all(windows, target_arch = "x86_64"))]
//! Authored artifact bytes and authority failures; no target process/pipe is accessed.
mod support;
use eg_game_access::{process::ProcessIdentity, transport::Delivery};
use eg_trainer::{
    backend::{BackendError, ErrorKind},
    connection::{AdmissionCheck, HelperPreparer, PreparationFailure, PreparationResidency},
    profile::BuildProfile,
};
use eg_trainer_host::windows::{NativePreparer, RuntimeAttestation, RuntimeAuthority};
use std::{
    future::Future,
    sync::{Arc, Mutex, atomic::Ordering},
};
use support::*;

struct Authority {
    seen: Observed,
    result: PreparationFailure,
}
impl Authority {
    fn new(residency: PreparationResidency) -> Self {
        Self {
            seen: Arc::new(Mutex::new(vec![])),
            result: failure(residency),
        }
    }
}
impl RuntimeAuthority for Authority {
    async fn attest(&mut self, target: &ProcessIdentity, profile: &BuildProfile, _: &mut dyn AdmissionCheck) -> Result<RuntimeAttestation, PreparationFailure> {
        self.seen.lock().unwrap().push((target.clone(), profile.clone()));
        Err(self.result.clone())
    }
}
fn assert_send<T: Send>() {}
fn assert_send_future(_: impl Future + Send) {}
#[test]
fn constructors_inert_and_traits_remain_send() {
    let fixture = Fixture::new();
    let authority = Authority::new(PreparationResidency::NotLoaded);
    let seen = authority.seen.clone();
    let logs = Logs::default();
    let calls = logs.calls.clone();
    let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, logs);
    assert_eq!(preparer.retained_targets(), 0);
    assert_eq!(preparer.uncertain_targets(), 0);
    assert_eq!(preparer.verified_targets(), 0);
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_send::<NativePreparer<Authority, Logs>>();
    let mut admission = Admission::new(true);
    assert_send_future(preparer.prepare(&fixture.target, &fixture.profile, &mut admission));
    assert!(seen.lock().unwrap().is_empty());
    assert_send::<<NativePreparer<Authority, Logs> as HelperPreparer>::Transport>();
}
#[test]
fn gate_denial_precedes_authority_and_log_creation() {
    run(async {
        let fixture = Fixture::new();
        let authority = Authority::new(PreparationResidency::MayBeLoaded);
        let seen = authority.seen.clone();
        let logs = Logs::default();
        let calls = logs.calls.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, logs);
        let mut admission = Admission::new(false);
        let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert_eq!(failure.error.kind, ErrorKind::Gate);
        assert_eq!(admission.checks.load(Ordering::SeqCst), 1);
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(preparer.retained_targets(), 0);
    })
}
#[test]
fn source_mismatch_refused_before_authority() {
    run(async {
        let fixture = Fixture::new();
        let authority = Authority::new(PreparationResidency::MayBeLoaded);
        let seen = authority.seen.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, Logs::default());
        let bad = fixture.profile_with_source(&"a".repeat(64));
        let failure = preparer.prepare(&fixture.target, &bad, &mut Admission::new(true)).await.err().unwrap();
        assert_eq!(failure.error.kind, ErrorKind::Identity);
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(preparer.retained_targets(), 0);
    })
}
#[test]
fn exact_identity_profile_reach_authority_and_notloaded_is_explicitly_retryable() {
    run(async {
        let fixture = Fixture::new();
        let authority = Authority::new(PreparationResidency::NotLoaded);
        let seen = authority.seen.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, Logs::default());
        let mut admission = Admission::new(true);
        for _ in 0..2 {
            let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
            assert_eq!(failure.residency, PreparationResidency::NotLoaded);
            assert_eq!(preparer.retained_targets(), 0);
        }
        assert_eq!(*seen.lock().unwrap(), vec![(fixture.target.clone(), fixture.profile.clone()); 2]);
        assert_eq!(admission.checks.load(Ordering::SeqCst), 2);
    })
}
#[test]
fn uncertain_or_loaded_authority_failure_is_retained_without_replay() {
    run(async {
        for residency in [PreparationResidency::MayBeLoaded, PreparationResidency::Loaded] {
            let fixture = Fixture::new();
            let authority = Authority::new(residency);
            let expected = authority.result.clone();
            let seen = authority.seen.clone();
            let logs = Logs::default();
            let calls = logs.calls.clone();
            let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, logs);
            for _ in 0..3 {
                assert_eq!(
                    preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.err().unwrap(),
                    expected
                );
            }
            assert_eq!(seen.lock().unwrap().len(), 1);
            assert_eq!(preparer.retained_targets(), 1);
            assert_eq!(preparer.uncertain_targets(), 1);
            assert_eq!(preparer.verified_targets(), 0);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    })
}
#[test]
fn lost_gate_still_denies_before_reusing_rejected_target() {
    run(async {
        let fixture = Fixture::new();
        let authority = Authority::new(PreparationResidency::MayBeLoaded);
        let seen = authority.seen.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, Logs::default());
        assert!(preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.is_err());
        let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(false)).await.err().unwrap();
        assert_eq!(failure.error.kind, ErrorKind::Gate);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(preparer.retained_targets(), 1);
    })
}
#[test]
fn retained_limit_refuses_new_target_before_authority_and_keeps_old_fences() {
    run(async {
        let fixture = Fixture::new();
        let authority = Authority::new(PreparationResidency::MayBeLoaded);
        let seen = authority.seen.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, Logs::default());
        for pid in 42..50 {
            assert!(
                preparer
                    .prepare(&other_target(&fixture.target, pid), &fixture.profile, &mut Admission::new(true))
                    .await
                    .is_err()
            );
        }
        assert_eq!(preparer.retained_targets(), 8);
        let failure = preparer
            .prepare(&other_target(&fixture.target, 50), &fixture.profile, &mut Admission::new(true))
            .await
            .err()
            .unwrap();
        assert_eq!(failure.error.kind, ErrorKind::RestartRequired);
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert_eq!(seen.lock().unwrap().len(), 8);
        assert_eq!(preparer.retained_targets(), 8);
        assert!(preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.is_err());
        assert_eq!(seen.lock().unwrap().len(), 8);
    })
}
#[test]
fn retained_rejection_does_not_claim_server_exists_or_call_transport() {
    run(async {
        let fixture = Fixture::new();
        let mut authority = Authority::new(PreparationResidency::MayBeLoaded);
        authority.result.error = BackendError {
            kind: ErrorKind::Transport,
            message: "startup acknowledgement lost".into(),
            delivery: Some(Delivery::MayHaveRun),
            retry_when_waiting: false,
        };
        let expected = authority.result.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, Logs::default());
        assert_eq!(
            preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.err().unwrap(),
            expected
        );
        assert_eq!(preparer.verified_targets(), 0);
    })
}
