//! Offline preparation failures: no live process or executable fixture is used.
#[path = "host_support/mod.rs"]
mod support;
use eg_trainer::connect::host::NativePreparer;
use eg_trainer::{
    backend::ErrorKind,
    connect::admission::{HelperPreparer, PreparationResidency},
};
use support::{Admission, Fixture};

fn preparer(fixture: &Fixture) -> NativePreparer {
    // SAFETY: these tests supply inert authored files and refuse before loading
    // any process. No helper/runtime ABI or callback is exercised.
    NativePreparer::new(fixture.artifact.clone())
}

#[test]
fn construction_is_inert_and_offline_denial_precedes_target_or_file_access() {
    support::run(async {
        let fixture = Fixture::new();
        let mut native = preparer(&fixture);
        let mut denied = Admission::new(false);
        let failure = native.prepare(&fixture.target, &fixture.build, &mut denied).await.err().unwrap();
        assert_eq!(failure.error.kind, ErrorKind::Gate);
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert_eq!(native.retained_targets(), 0);
    });
}

#[test]
fn mismatched_source_is_refused_before_process_access() {
    support::run(async {
        let fixture = Fixture::new();
        let build = fixture.profile_with_source(&"a".repeat(64));
        let mut native = preparer(&fixture);
        let failure = native.prepare(&fixture.target, &build, &mut Admission::new(true)).await.err().unwrap();
        assert_eq!(failure.error.kind, ErrorKind::Identity);
        assert_eq!(failure.residency, PreparationResidency::NotLoaded);
        assert_eq!(native.retained_targets(), 0);
    });
}

#[test]
fn local_preflight_failure_is_retryable_without_reserving_a_loaded_generation() {
    support::run(async {
        let fixture = Fixture::new();
        let mut native = preparer(&fixture);
        for _ in 0..2 {
            // No UnityPlayer.dll exists: refusal happens before any OpenProcess.
            let failure = native.prepare(&fixture.target, &fixture.build, &mut Admission::new(true)).await.err().unwrap();
            assert_eq!(failure.error.kind, ErrorKind::Identity);
            assert_eq!(failure.residency, PreparationResidency::NotLoaded);
            assert_eq!(native.retained_targets(), 0);
        }
    });
}
