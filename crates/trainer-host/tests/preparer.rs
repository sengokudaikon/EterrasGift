//! Offline preparation failures: no live process or executable fixture is used.
mod support;
use eg_trainer::{
    backend::ErrorKind,
    connection::{HelperPreparer, PreparationResidency},
};
use eg_trainer_host::windows::NativePreparer;
use std::sync::atomic::Ordering;
use support::{Admission, Fixture, Logs};

fn preparer(fixture: &Fixture, _logs: Logs) -> NativePreparer {
    // SAFETY: these tests supply inert authored files and refuse before loading
    // any process. No helper/runtime ABI or callback is exercised.
    NativePreparer::new(fixture.artifact.clone())
}

#[test]
fn construction_is_inert_and_offline_denial_precedes_target_or_file_access() {
    support::run(async {
        let fixture = Fixture::new();
        let logs = Logs::default();
        let mut native = preparer(&fixture, logs.clone());
        assert_eq!(logs.calls.load(Ordering::SeqCst), 0);
        let mut denied = Admission::new(false);
        let failure = native.prepare(&fixture.target, &fixture.profile, &mut denied).await.err().unwrap();
        assert_eq!(failure.error.kind, ErrorKind::Gate);
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert_eq!(native.retained_targets(), 0);
        assert_eq!(logs.calls.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn mismatched_source_is_refused_before_process_access() {
    support::run(async {
        let fixture = Fixture::new();
        let profile = fixture.profile_with_source(&"a".repeat(64));
        let mut native = preparer(&fixture, Logs::default());
        let failure = native.prepare(&fixture.target, &profile, &mut Admission::new(true)).await.err().unwrap();
        assert_eq!(failure.error.kind, ErrorKind::Identity);
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert_eq!(native.retained_targets(), 0);
    });
}

#[test]
fn local_preflight_failure_is_retryable_without_reserving_a_loaded_generation() {
    support::run(async {
        let fixture = Fixture::new();
        let mut native = preparer(&fixture, Logs::default());
        for _ in 0..2 {
            // No UnityPlayer.dll exists: refusal happens before any OpenProcess.
            let failure = native.prepare(&fixture.target, &fixture.profile, &mut Admission::new(true)).await.err().unwrap();
            assert_eq!(failure.error.kind, ErrorKind::Identity);
            assert_eq!(failure.residency, PreparationResidency::NotLoaded);
            assert_eq!(native.retained_targets(), 0);
        }
    });
}
