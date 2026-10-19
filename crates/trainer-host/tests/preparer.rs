#![cfg(all(windows, target_arch = "x86_64"))]
//! Authored artifact bytes and authority failures; no target process/pipe is accessed.
mod support;
use eg_game_access::{process::ProcessIdentity, transport::Delivery};
use eg_trainer::{
    backend::{BackendError, ErrorKind},
    connection::{AdmissionCheck, HelperPreparer, PreparationFailure, PreparationResidency},
    profile::BuildProfile,
};
use eg_trainer_host::windows::{AuthorityJournal, NativePreparer, RuntimeAttestation, RuntimeAuthority};
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
    async fn attest(
        &mut self,
        target: &ProcessIdentity,
        profile: &BuildProfile,
        _: &mut dyn AdmissionCheck,
        _: &mut AuthorityJournal,
    ) -> Result<RuntimeAttestation, PreparationFailure> {
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

struct SuspendedAuthority {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    panic: bool,
}
impl RuntimeAuthority for SuspendedAuthority {
    async fn attest(&mut self, _: &ProcessIdentity, _: &BuildProfile, _: &mut dyn AdmissionCheck, _: &mut AuthorityJournal) -> Result<RuntimeAttestation, PreparationFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic, "authored authority unwind before any process access");
        std::future::pending().await
    }
}
#[test]
fn canceled_authority_reserves_target_before_await_and_refuses_replay() {
    let fixture = Fixture::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut preparer = NativePreparer::new(
        fixture.artifact.clone(),
        SuspendedAuthority {
            calls: calls.clone(),
            panic: false,
        },
        Logs::default(),
    );
    let mut admission = Admission::new(true);
    let mut future = Box::pin(preparer.prepare(&fixture.target, &fixture.profile, &mut admission));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(future.as_mut().poll(&mut context).is_pending());
    drop(future);
    assert_eq!(preparer.retained_targets(), 1);
    assert_eq!(preparer.uncertain_targets(), 1);
    run(async {
        let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
        assert_eq!(failure.residency, PreparationResidency::MayBeLoaded);
        assert_eq!(failure.error.kind, ErrorKind::RestartRequired);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn unwound_authority_reservation_refuses_replay() {
    let fixture = Fixture::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut preparer = NativePreparer::new(
        fixture.artifact.clone(),
        SuspendedAuthority {
            calls: calls.clone(),
            panic: true,
        },
        Logs::default(),
    );
    let mut admission = Admission::new(true);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut future = std::pin::pin!(preparer.prepare(&fixture.target, &fixture.profile, &mut admission));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let _ = future.as_mut().poll(&mut context);
    }));
    assert!(panic.is_err());
    assert_eq!(preparer.retained_targets(), 1);
    run(async {
        let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
        assert_eq!(failure.residency, PreparationResidency::MayBeLoaded);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct CountOwnerDrop(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for CountOwnerDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct RegisteredAuthority {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    owner: Option<Arc<CountOwnerDrop>>,
    result: Option<PreparationResidency>,
    panic: bool,
}
impl RuntimeAuthority for RegisteredAuthority {
    async fn attest(
        &mut self,
        _: &ProcessIdentity,
        _: &BuildProfile,
        _: &mut dyn AdmissionCheck,
        journal: &mut AuthorityJournal,
    ) -> Result<RuntimeAttestation, PreparationFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The only authority-owned clone moves into the journal before suspension,
        // unwind or completion. Nothing is submitted to an actual target.
        journal.retain_owner(self.owner.take().unwrap());
        assert!(!self.panic, "authored unwind after journal registration");
        if let Some(residency) = self.result {
            Err(failure(residency))
        } else {
            std::future::pending().await
        }
    }
}
fn reclaim_authored_pin(owner: Arc<CountOwnerDrop>, drops: &Arc<std::sync::atomic::AtomicUsize>) {
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(Arc::strong_count(&owner), 2, "one external owner and exactly one retained journal pin");
    // SAFETY: this no-process fake registered exactly one clone in the journal,
    // which deliberately forgot it on Drop. Reclaim only that known clone.
    unsafe {
        Arc::decrement_strong_count(Arc::as_ptr(&owner));
    }
    drop(owner);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn registered_owners_survive_canceled_and_unwound_authority_and_preparer_drop() {
    for panic in [false, true] {
        let fixture = Fixture::new();
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let owner = Arc::new(CountOwnerDrop(drops.clone()));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let authority = RegisteredAuthority {
            calls: calls.clone(),
            owner: Some(owner.clone()),
            result: None,
            panic,
        };
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, Logs::default());
        let mut admission = Admission::new(true);
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut future = std::pin::pin!(preparer.prepare(&fixture.target, &fixture.profile, &mut admission));
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(future.as_mut().poll(&mut context).is_pending());
        }));
        assert_eq!(observed.is_err(), panic);
        assert_eq!(preparer.retained_targets(), 1);
        assert_eq!(preparer.uncertain_targets(), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        run(async {
            let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
            assert_eq!(failure.error.kind, ErrorKind::RestartRequired);
            assert_eq!(failure.residency, PreparationResidency::MayBeLoaded);
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(preparer);
        reclaim_authored_pin(owner, &drops);
    }
}
#[test]
fn completed_registered_authority_refusals_preserve_residency_and_never_replay() {
    for residency in [PreparationResidency::NotLoaded, PreparationResidency::MayBeLoaded, PreparationResidency::Loaded] {
        let fixture = Fixture::new();
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let owner = Arc::new(CountOwnerDrop(drops.clone()));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let authority = RegisteredAuthority {
            calls: calls.clone(),
            owner: Some(owner.clone()),
            result: Some(residency),
            panic: false,
        };
        let logs = Logs::default();
        let log_calls = logs.calls.clone();
        let mut preparer = NativePreparer::new(fixture.artifact.clone(), authority, logs);
        let expected_residency = if residency == PreparationResidency::NotLoaded {
            PreparationResidency::MayBeLoaded
        } else {
            residency
        };
        run(async {
            for _ in 0..2 {
                let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.err().unwrap();
                assert_eq!(failure.residency, expected_residency);
                if residency == PreparationResidency::NotLoaded {
                    assert_eq!(failure.error.delivery, Some(Delivery::MayHaveRun));
                    assert!(!failure.error.retry_when_waiting);
                }
            }
            // Gate and helper-source refusals still expose prior residency without
            // calling authority again or pretending no target work could exist.
            let gate = preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(false)).await.err().unwrap();
            assert_eq!(gate.error.kind, ErrorKind::Gate);
            assert_eq!(gate.residency, expected_residency);
            let other_source = fixture.profile_with_source(&"a".repeat(64));
            let source = preparer.prepare(&fixture.target, &other_source, &mut Admission::new(true)).await.err().unwrap();
            assert_eq!(source.error.kind, ErrorKind::Identity);
            assert_eq!(source.residency, expected_residency);
        });
        assert_eq!(preparer.retained_targets(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(log_calls.load(Ordering::SeqCst), 0);
        drop(preparer);
        reclaim_authored_pin(owner, &drops);
    }
}
#[test]
fn never_polled_future_creates_no_reservation_or_registration() {
    let fixture = Fixture::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut preparer = NativePreparer::new(
        fixture.artifact.clone(),
        SuspendedAuthority {
            calls: calls.clone(),
            panic: false,
        },
        Logs::default(),
    );
    let mut admission = Admission::new(true);
    drop(preparer.prepare(&fixture.target, &fixture.profile, &mut admission));
    assert_eq!(preparer.retained_targets(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(admission.checks.load(Ordering::SeqCst), 0);
}
#[test]
fn canceled_authority_binds_profile_process_birth_and_eight_exact_slots() {
    let fixture = Fixture::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut preparer = NativePreparer::new(
        fixture.artifact.clone(),
        SuspendedAuthority {
            calls: calls.clone(),
            panic: false,
        },
        Logs::default(),
    );
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    // Same PID and path with a different birth identity is a distinct target.
    for generation in 0..8 {
        let target = ProcessIdentity {
            created: fixture.target.created + generation,
            ..fixture.target.clone()
        };
        let mut admission = Admission::new(true);
        let mut future = Box::pin(preparer.prepare(&target, &fixture.profile, &mut admission));
        assert!(future.as_mut().poll(&mut context).is_pending());
        drop(future);
        assert_eq!(preparer.retained_targets(), generation as usize + 1);
    }
    run(async {
        let ninth = ProcessIdentity {
            created: fixture.target.created + 8,
            ..fixture.target.clone()
        };
        let limit = preparer.prepare(&ninth, &fixture.profile, &mut Admission::new(true)).await.err().unwrap();
        assert_eq!(limit.error.kind, ErrorKind::RestartRequired);
        assert_eq!(limit.residency, PreparationResidency::NotLoaded);
        let metadata = fixture.temp.path().join("Authored_Data/il2cpp_data/Metadata/global-metadata.dat");
        std::fs::write(&metadata, b"authored changed metadata profile").unwrap();
        let changed = fixture.profile_with_source(SOURCE);
        let drift = preparer.prepare(&fixture.target, &changed, &mut Admission::new(true)).await.err().unwrap();
        assert_eq!(drift.error.kind, ErrorKind::Identity);
        assert_eq!(drift.residency, PreparationResidency::MayBeLoaded);
        let original = preparer.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.err().unwrap();
        assert_eq!(original.error.kind, ErrorKind::RestartRequired);
        assert_eq!(original.residency, PreparationResidency::MayBeLoaded);
    });
    assert_eq!(preparer.retained_targets(), 8);
    assert_eq!(preparer.uncertain_targets(), 8);
    assert_eq!(calls.load(Ordering::SeqCst), 8);
}

struct SynchronousPanicker {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    panic: bool,
}
impl RuntimeAuthority for SynchronousPanicker {
    fn attest(
        &mut self,
        _: &ProcessIdentity,
        _: &BuildProfile,
        _: &mut dyn AdmissionCheck,
        _: &mut AuthorityJournal,
    ) -> impl Future<Output = Result<RuntimeAttestation, PreparationFailure>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic, "authored panic before constructing authority future");
        std::future::ready(Err(failure(PreparationResidency::NotLoaded)))
    }
}
#[test]
fn reservation_precedes_even_synchronous_authority_future_construction() {
    let fixture = Fixture::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut preparer = NativePreparer::new(
        fixture.artifact.clone(),
        SynchronousPanicker {
            calls: calls.clone(),
            panic: true,
        },
        Logs::default(),
    );
    let mut admission = Admission::new(true);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut future = std::pin::pin!(preparer.prepare(&fixture.target, &fixture.profile, &mut admission));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let _ = future.as_mut().poll(&mut context);
    }));
    assert!(panic.is_err());
    assert_eq!(preparer.retained_targets(), 1);
    run(async {
        let failure = preparer.prepare(&fixture.target, &fixture.profile, &mut admission).await.err().unwrap();
        assert_eq!(failure.residency, PreparationResidency::MayBeLoaded);
        assert_eq!(failure.error.kind, ErrorKind::RestartRequired);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
