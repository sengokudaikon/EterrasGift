use eg_core::settings::{Locations, SettingsStore};
use eg_core::settings_service::{Change, SaveOutcome, SettingsService};
use serde_json::json;
use std::fs;
use std::sync::{LazyLock, mpsc};
use std::time::Duration;
use task_runtime::{Runtime, Tasks};

const DEADLINE: Duration = Duration::from_secs(5);

fn tasks() -> Tasks {
    static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::start().unwrap());
    RUNTIME.tasks()
}

#[test]
fn an_idle_close_gate_atomically_stops_admission_and_can_resume_after_failure() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let (release, waiting) = mpsc::channel();
    let revision = service
        .module("fixture")
        .update(move |_| {
            waiting.recv_timeout(DEADLINE).unwrap();
            Ok(())
        })
        .unwrap();
    assert!(!service.pause_when_idle());
    release.send(()).unwrap();
    service.wait(revision);
    assert!(service.pause_when_idle());
    let other = service.clone();
    assert!(std::thread::spawn(move || other.module("fixture").set_bool("enabled", true)).join().unwrap().is_err());
    assert_eq!(service.snapshot().submitted, revision);
    service.resume();
    let revision = service.module("fixture").set_bool("enabled", true).unwrap();
    service.wait(revision);
    service.close();
    service.resume(); // Final shutdown cannot be undone by the reversible pause.
    assert!(service.submit(Change::Retry).is_err());
}

#[test]
fn listeners_hear_each_completion_after_it_is_observable() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let (heard, hearing) = mpsc::channel();
    let observer = service.clone();
    // A listener may read the snapshot (not submit): the completion it hears is already there.
    service.on_completed(move || heard.send(observer.snapshot().completed).unwrap());
    let first = service.module("fixture").set_bool("enabled", true).unwrap();
    let second = service.module("fixture").set_bool("enabled", false).unwrap();
    assert_eq!(hearing.recv_timeout(DEADLINE).unwrap(), first);
    assert_eq!(hearing.recv_timeout(DEADLINE).unwrap(), second);
    assert!(hearing.try_recv().is_err(), "nothing is heard without a completion");
    service.close();
}

fn service(temp: &std::path::Path) -> SettingsService {
    SettingsService::new(SettingsStore::open(&temp.join("app"), &temp.join("settings.json"), &[]), &tasks())
}

#[test]
fn the_runtime_finishes_accepted_saves_of_a_service_nobody_closed() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = Runtime::start().unwrap();
    let service = SettingsService::new(SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]), &runtime.tasks());
    let ticket = service.module("editor").set_bool("automatic", true).unwrap();
    assert!(!runtime.shutdown());
    assert_eq!(service.snapshot().completed, ticket);
    assert_eq!(service.snapshot().outcome, SaveOutcome::Persisted);
    // Refused, and a waiter is not left hanging.
    let late = service.module("editor").set_bool("automatic", false).unwrap();
    assert!(matches!(service.wait(late).outcome, SaveOutcome::Faulted(_)));
}

#[test]
fn a_subscriber_sees_each_completion() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let mut snapshots = service.subscribe();
    let ticket = service.module("editor").set_bool("automatic", true).unwrap();
    tasks()
        .block_on(async { snapshots.wait_for(|snapshot| snapshot.completed == ticket).await.map(|_| ()) })
        .unwrap();
    service.close();
}

#[test]
fn overlapping_module_writes_patch_the_latest_slice_and_keep_unknown_data() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("settings.json"),
        json!({
            "schemaVersion": 2, "future": {"keep": [1,2]},
            "moduleSettings": {"editor": {"AutoConnect": true, "future": {"nested": 9}}, "absent-module": {"keep": true}}
        })
        .to_string(),
    )
    .unwrap();
    let service = service(temp.path());
    let module = service.module("editor");
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    module
        .update(move |fields| {
            entered.send(()).unwrap();
            wait.recv_timeout(DEADLINE).unwrap();
            fields.insert("values".into(), json!([12, 34]));
            Ok(())
        })
        .unwrap();
    started.recv_timeout(DEADLINE).unwrap();
    let before = service.snapshot();
    assert!(before.saving());
    assert_eq!(before.completed, 0); // Reads do not acquire the worker's IO/mutation lock.
    let ticket = module.set_bool("autoConnect", false).unwrap();
    let snapshot = service.snapshot();
    assert_eq!(snapshot.submitted, ticket);
    assert_eq!(snapshot.completed, 0);
    release.send(()).unwrap();
    let snapshot = service.wait(ticket);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    let fields = &snapshot.store.saved().module_settings["editor"];
    assert_eq!(fields["values"], json!([12, 34]));
    assert_eq!(fields["autoConnect"], false);
    assert!(fields.get("AutoConnect").is_none());
    assert_eq!(fields["future"]["nested"], 9);
    assert_eq!(snapshot.store.saved().module_settings["absent-module"]["keep"], true);
    assert_eq!(snapshot.store.saved().extra["future"], json!({"keep": [1,2]}));
    assert_eq!(snapshot.persisted_revision, ticket);
}

#[test]
fn failed_writes_keep_intent_separate_and_an_explicit_retry_persists_it() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let module = service.module("editor");
    let first = module.set_bool("automatic", true).unwrap();
    service.wait(first);
    let file = temp.path().join("settings.json");
    let old = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap(); // Atomic replacement fails deterministically on all platforms.
    let failed = module.set_bool("automatic", false).unwrap();
    let snapshot = service.wait(failed);
    assert!(matches!(snapshot.outcome, SaveOutcome::Failed(_)));
    assert_eq!(snapshot.persisted_revision, first);
    assert_eq!(snapshot.store.persisted().unwrap().module_settings["editor"]["automatic"], true);
    assert_eq!(snapshot.store.desired().module_settings["editor"]["automatic"], false);
    assert!(snapshot.store.has_pending_write());
    assert!(fs::read_dir(&file).unwrap().next().is_none());
    fs::remove_dir(&file).unwrap();
    fs::write(&file, old).unwrap();
    let retry = service.submit(Change::Retry).unwrap();
    let snapshot = service.wait(retry);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    assert_eq!(snapshot.persisted_revision, retry);
    assert!(!snapshot.store.has_pending_write());
    assert_eq!(snapshot.store.saved().module_settings["editor"]["automatic"], false);
    let reopened = SettingsStore::open(&temp.path().join("app"), &file, &[]);
    assert_eq!(reopened.saved(), snapshot.store.saved());
}

#[test]
fn a_successful_later_write_includes_failed_pending_changes() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let file = temp.path().join("settings.json");
    fs::create_dir(&file).unwrap();
    let failed = service.module("one").set_bool("enabled", true).unwrap();
    assert!(matches!(service.wait(failed).outcome, SaveOutcome::Failed(_)));
    fs::remove_dir(&file).unwrap();
    let next = service.module("two").set_bool("enabled", false).unwrap();
    let snapshot = service.wait(next);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    assert_eq!(snapshot.store.saved().module_settings["one"]["enabled"], true);
    assert_eq!(snapshot.store.saved().module_settings["two"]["enabled"], false);
}

#[test]
fn unreadable_files_keep_session_choices_until_an_explicit_location_save() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("settings.json");
    fs::write(&file, b"damaged original").unwrap();
    let service = service(temp.path());
    let ticket = service.module("editor").set_bool("automatic", false).unwrap();
    let snapshot = service.wait(ticket);
    assert_eq!(snapshot.outcome, SaveOutcome::MemoryOnly);
    assert!(snapshot.store.persisted().is_none());
    assert_eq!(fs::read(&file).unwrap(), b"damaged original");
    let ticket = service.submit(Change::Locations(Locations::default())).unwrap();
    let snapshot = service.wait(ticket);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    assert_eq!(snapshot.store.saved().module_settings["editor"]["automatic"], false);
}

#[test]
fn a_module_validator_cannot_partially_mutate_the_document() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let ticket = service
        .module("one")
        .update(|fields| {
            fields.insert("partial".into(), true.into());
            Err("Invalid value".into())
        })
        .unwrap();
    let snapshot = service.wait(ticket);
    assert_eq!(snapshot.outcome, SaveOutcome::Failed("Invalid value".into()));
    assert!(snapshot.store.desired().module_settings.is_empty());
    assert!(!temp.path().join("settings.json").exists());
    let ticket = service.module("two").set_bool("valid", true).unwrap();
    assert_eq!(service.wait(ticket).outcome, SaveOutcome::Persisted);
}

#[test]
fn rapid_switches_keep_the_last_intent_without_activating_a_module_mid_run() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let mut ticket = 0;
    for enabled in [false, true, false, true, false, true] {
        ticket = service
            .submit(Change::ModuleEnabled {
                id: "editor".into(),
                enabled,
                default: true,
            })
            .unwrap();
    }
    let snapshot = service.wait(ticket);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    assert!(!snapshot.store.saved().modules.contains_key("editor"));
    assert!(snapshot.store.startup_modules().is_empty());
}

#[test]
fn close_drains_accepted_writes_and_rejects_new_work() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    service
        .module("editor")
        .update(move |fields| {
            entered.send(()).unwrap();
            wait.recv_timeout(DEADLINE).unwrap();
            fields.insert("first".into(), true.into());
            Ok(())
        })
        .unwrap();
    started.recv_timeout(DEADLINE).unwrap();
    let ticket = service.module("editor").set_bool("second", true).unwrap();
    let closer = service.clone();
    let (finished, done) = mpsc::channel();
    let thread = std::thread::spawn(move || finished.send(closer.close()).unwrap());
    assert!(matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.send(()).unwrap();
    let snapshot = done.recv_timeout(DEADLINE).unwrap();
    thread.join().unwrap();
    assert_eq!(snapshot.completed, ticket);
    assert_eq!(snapshot.store.saved().module_settings["editor"], json!({"first":true,"second":true}));
    assert!(service.module("editor").set_bool("late", true).is_err());
}

#[test]
fn a_worker_panic_is_terminal_and_does_not_leave_waiters_hanging() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let ticket = service.module("editor").update(|_| panic!("fixture panic")).unwrap();
    let snapshot = service.wait(ticket);
    assert!(matches!(snapshot.outcome, SaveOutcome::Faulted(_)));
    assert!(service.submit(Change::Retry).is_err());
    assert_eq!(service.close().completed, ticket);
    assert!(!temp.path().join("settings.json").exists());
}

#[test]
fn location_acknowledgement_is_not_replaced_by_a_later_switch_result() {
    let temp = tempfile::tempdir().unwrap();
    let service = service(temp.path());
    let location = service
        .submit(Change::Locations(Locations {
            game_executable: "relative.exe".into(),
            ..Default::default()
        }))
        .unwrap();
    let switch = service.module("editor").set_bool("automatic", true).unwrap();
    let snapshot = service.wait(switch);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    let (revision, result) = snapshot.locations.unwrap();
    assert_eq!(revision, location);
    assert!(result.is_err());
}

#[test]
fn retry_preserves_the_explicit_request_to_replace_an_unreadable_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("settings.json");
    fs::write(&file, b"damaged original").unwrap();
    let service = service(temp.path());
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    let location = service.submit(Change::Locations(Locations::default())).unwrap();
    assert!(matches!(service.wait(location).outcome, SaveOutcome::Failed(_)));
    fs::remove_dir(&file).unwrap();
    fs::write(&file, b"damaged original").unwrap();
    let retry = service.submit(Change::Retry).unwrap();
    let snapshot = service.wait(retry);
    assert_eq!(snapshot.outcome, SaveOutcome::Persisted);
    assert!(!snapshot.store.unreadable());
    assert!(snapshot.pending_location.is_none());
    let (revision, result) = snapshot.locations.unwrap();
    assert_eq!(revision, location);
    assert!(result.is_ok());
    let parsed: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(parsed["schemaVersion"], 2);
}

#[cfg(windows)]
#[test]
fn failed_migration_bookkeeping_keeps_the_marker_for_the_next_start() {
    use std::os::windows::fs::OpenOptionsExt;
    for library in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app");
        let source = temp.path().join("old");
        let destination = temp.path().join("new");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("first.txt"), b"original").unwrap();
        let file = temp.path().join("settings.json");
        let mut document = json!({"schemaVersion": 2});
        let (target_key, marker) = if library {
            ("modLibraryDirectory", "modLibraryMigrationSource")
        } else {
            ("userDataDirectory", "migrationSource")
        };
        document[target_key] = destination.to_string_lossy().as_ref().into();
        document[marker] = source.to_string_lossy().as_ref().into();
        fs::write(&file, document.to_string()).unwrap();
        // Reads remain allowed; atomic replacement cannot complete while this fixture handle is held.
        let locked = fs::OpenOptions::new().read(true).share_mode(1).open(&file).unwrap();
        let store = SettingsStore::open(&root, &file, &[]);
        assert!(store.startup_error().is_some());
        assert!(!store.has_pending_write());
        assert_eq!(fs::read(destination.join("first.txt")).unwrap(), b"original");
        drop(locked);
        let service = SettingsService::new(store, &tasks());
        let ticket = service.module("editor").set_bool("automatic", false).unwrap();
        assert_eq!(service.wait(ticket).outcome, SaveOutcome::Persisted);
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved[marker], source.to_string_lossy().as_ref());
        fs::write(source.join("later.txt"), b"written after the failed migration").unwrap();
        service.close();
        let reopened = SettingsStore::open(&root, &file, &[]);
        assert!(reopened.startup_error().is_none());
        assert_eq!(fs::read(destination.join("later.txt")).unwrap(), b"written after the failed migration");
        let active = if library {
            &reopened.paths().mod_library_directory
        } else {
            &reopened.paths().user_data_directory
        };
        assert_eq!(active, &destination);
    }
}
