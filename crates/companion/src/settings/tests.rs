//! The page's tests. The whole file is for tests, which also keeps its words out of the catalog.
#![cfg(all(test, windows))]

use super::*;
use eg_core::settings::SettingsStore;
use phrases::Phrase;
use spiderland::{TestStore, step};
use std::fs;

fn trainer(default_on: bool) -> ModuleChoice {
    ModuleChoice {
        id: "trainer",
        title: Phrase::new("Trainer"),
        description: Phrase::new("Edits the running game."),
        name: Phrase::new("the trainer"),
        default_on,
        problem: None,
        sections: Vec::new(),
    }
}

/// A module with one of each section, as a real one describes itself.
fn with_sections(id: &'static str, default_on: bool) -> ModuleChoice {
    ModuleChoice {
        id,
        sections: vec![
            ModuleSection::Status {
                title: Phrase::new("Data"),
                description: Phrase::new("Its data."),
                text: Phrase::new("No data"),
                action: Phrase::new("Read again"),
                tip: Phrase::new("Read it again"),
                unavailable: Phrase::new("Not found."),
            },
            ModuleSection::Toggle {
                key: "autoConnect",
                title: Phrase::new("Connection"),
                label: Phrase::new("Connect automatically"),
                description: Phrase::new("Connect when ready."),
                note: None,
                default: true,
                on_text: Phrase::new("Automatic"),
                off_text: Phrase::new("Manual"),
            },
            ModuleSection::Toggle {
                key: "iKnowWhatImDoing",
                title: Phrase::new("Override"),
                label: Phrase::new("Override"),
                description: Phrase::new("Allows more."),
                note: None,
                default: false,
                on_text: Phrase::new("On"),
                off_text: Phrase::new("Off"),
            },
        ],
        ..trainer(default_on)
    }
}

/// The feature on a real store over a real settings service, its effects performed as the window's host performs them,
/// those it starts with included.
struct Page {
    store: TestStore<SettingsPage>,
    service: SettingsService,
    /// What a linked module was told.
    switched: Vec<(String, bool)>,
    closed: bool,
    shown: bool,
    restart: bool,
}

impl Page {
    fn new(service: SettingsService, modules: Vec<ModuleChoice>) -> Self {
        let mut page = Page {
            store: TestStore::new(State::new(service.snapshot(), modules, None, None)),
            service,
            switched: Vec::new(),
            closed: false,
            shown: false,
            restart: false,
        };
        page.perform();
        page
    }

    fn send(&mut self, intent: Intent) {
        self.store.send(intent);
        self.perform();
    }

    /// Performs what the page asked for, and what that leads to, until it asks for nothing more.
    fn perform(&mut self) {
        let Page {
            store,
            service,
            switched,
            closed,
            shown,
            restart,
        } = self;
        store.answer(|effect| -> Vec<Intent> {
            match effect {
                Effect::Switched { key, on, .. } => switched.push((key.into(), on)),
                Effect::Submit(save, submission) => return submit(service, save, submission).into(),
                Effect::Observe => return vec![Intent::Observed(Box::new(service.snapshot()))],
                Effect::TryPause => return vec![try_pause(service)],
                Effect::Resume => service.resume(),
                Effect::Close => *closed = true,
                Effect::RequestRestart(on) => *restart = on,
                Effect::ReadStatus { .. } | Effect::Act { .. } | Effect::Browse(_) | Effect::Quit => {}
            }
            Vec::new()
        });
        // No page goes first in these tests' closes.
        *shown |= store.take_outputs().contains(&Output::ShowPage);
    }

    fn refresh(&mut self) {
        let snapshot = self.service.snapshot();
        self.send(Intent::Observed(Box::new(snapshot)));
    }

    fn edit(&mut self, location: Location, text: &str) {
        self.send(Intent::Edit(location, text.to_owned()));
    }

    fn read<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        self.store.read(read)
    }

    /// Sets up what no intent reaches, behind the reducer's back.
    fn change(&self, change: impl FnOnce(&mut State)) {
        self.store.store().with_state(change);
    }

    fn kind(&self) -> MessageKind {
        self.read(State::kind)
    }

    fn message(&self) -> String {
        self.read(|state| state.message().to_owned())
    }

    fn location(&self, location: Location) -> String {
        self.read(|state| state.location(location).to_owned())
    }

    fn placeholder(&self, location: Location) -> String {
        self.read(|state| state.placeholder(location).to_owned())
    }
}

/// A service saving on the spot to a settings file in `temp`.
fn inline(temp: &Path) -> SettingsService {
    let root = temp.join("app");
    fs::create_dir_all(&root).unwrap();
    // As the app opens it, with the companion's checks of the tool folders.
    SettingsService::inline(SettingsStore::open(&root, &temp.join("settings.json"), &[]).checking(FOLDER_CHECKS))
}

fn page(temp: &Path) -> Page {
    Page::new(inline(temp), vec![trainer(true)])
}

#[test]
fn switches_reach_the_running_module_as_they_flip_whatever_the_save_does() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("settings.json");
    let store = SettingsStore::open(&temp.path().join("app"), &file, &[]);
    let mut page = Page::new(SettingsService::inline(store), vec![with_sections("trainer", true)]);
    page.send(Intent::ToggleSetting(0, 1, false));
    assert_eq!(page.switched, [("autoConnect".to_owned(), false)]);
    // A save that fails still leaves the session with the choice: the session takes it before the save.
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    page.send(Intent::ToggleSetting(0, 2, true));
    assert_eq!(page.kind(), MessageKind::Error);
    assert_eq!(page.switched.last(), Some(&("iKnowWhatImDoing".to_owned(), true)));
    // A status section is not a switch.
    page.send(Intent::ToggleSetting(0, 0, true));
    assert_eq!(page.switched.len(), 2);
}

#[test]
fn a_module_that_did_not_start_hears_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
    let mut page = Page::new(SettingsService::inline(store), vec![with_sections("trainer", false)]);
    page.send(Intent::ToggleSetting(0, 1, false));
    assert!(page.switched.is_empty());
}

#[test]
fn a_status_section_follows_its_module() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
    let mut page = Page::new(SettingsService::inline(store), vec![with_sections("trainer", true)]);
    let status = |page: &Page| match &page.read(State::sections)[0].body {
        SectionBody::Status {
            status,
            action_enabled,
            action_tip,
            unavailable,
            ..
        } => (status.clone(), *action_enabled, *action_tip, *unavailable),
        SectionBody::Toggle { .. } => panic!("a status section comes first"),
    };
    assert_eq!(status(&page), ("No data".to_owned(), false, "", "Not found."));
    let live = |can_act, busy| LiveStatus {
        text: "42 affixes".into(),
        can_act,
        busy,
    };
    page.send(Intent::Status(0, 0, Some(live(true, false))));
    assert_eq!(status(&page), ("42 affixes".to_owned(), true, "Read it again", ""));
    page.send(Intent::Status(0, 0, Some(live(true, true))));
    assert_eq!(status(&page), ("42 affixes".to_owned(), false, "", ""), "a running action waits without a reason");
    let version = page.read(State::rows_version);
    page.send(Intent::Status(0, 0, Some(live(true, true))));
    assert_eq!(page.read(State::rows_version), version, "the same status leaves the rows alone");
}

#[test]
fn a_location_save_says_so_and_a_switch_save_says_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    let sources = temp.path().join("sources");
    fs::create_dir(&sources).unwrap();
    page.edit(Location::ModSources, &sources.display().to_string());
    page.send(Intent::SaveLocations);
    assert_eq!((page.kind(), page.message().as_str()), (MessageKind::Saved, "Locations saved."));
    page.send(Intent::ToggleModule(0, false));
    page.send(Intent::ToggleModule(0, true));
    assert_eq!((page.kind(), page.message().as_str()), (MessageKind::Saved, ""));
}

fn tasks() -> task_runtime::Tasks {
    static RUNTIME: std::sync::LazyLock<task_runtime::Runtime> = std::sync::LazyLock::new(|| task_runtime::Runtime::start().unwrap());
    RUNTIME.tasks()
}

#[test]
fn restart_stays_offered_while_a_save_runs() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
    let service = SettingsService::new(store, &tasks());
    let mut page = Page::new(service.clone(), vec![trainer(true)]);
    page.send(Intent::ToggleModule(0, false));
    service.wait(service.snapshot().submitted);
    page.refresh();
    assert!(page.read(State::can_restart));
    let (release, waiting) = std::sync::mpsc::channel::<()>();
    service
        .module("fixture")
        .update(move |_| {
            waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
    page.refresh();
    assert!(page.read(|state| state.saving() && state.can_restart()));
    release.send(()).unwrap();
    service.wait(service.snapshot().submitted);
}

#[test]
fn restart_now_waits_for_the_save_then_closes_and_asks_for_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
    let service = SettingsService::new(store, &tasks());
    let mut page = Page::new(service.clone(), vec![trainer(true)]);
    let (release, waiting) = std::sync::mpsc::channel::<()>();
    service
        .module("fixture")
        .update(move |_| {
            waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
    page.refresh();
    page.send(Intent::Restart);
    assert!(!page.restart && !page.closed, "a save is in flight");
    release.send(()).unwrap();
    service.wait(service.snapshot().submitted);
    page.refresh();
    assert!(page.restart && page.closed);
}

#[test]
fn a_switch_saves_as_it_flips_and_waits_for_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    assert!(page.read(|state| state.module_enabled(0)));
    page.send(Intent::ToggleModule(0, false));
    assert_eq!(page.message(), "Saved. Restart Eterra's Gift to turn the trainer off.");
    assert!(page.read(State::can_restart));
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(temp.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(saved["modules"]["trainer"], false);
    // Back to the default: the choice leaves the file, and nothing waits any more.
    page.send(Intent::ToggleModule(0, true));
    assert_eq!(page.message(), "");
    assert_eq!(page.kind(), MessageKind::Saved);
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(temp.path().join("settings.json")).unwrap()).unwrap();
    assert!(saved.get("modules").is_none());
}

#[test]
fn locations_save_on_request_and_show_as_saved() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    assert!(!page.read(State::can_save_locations));
    page.edit(Location::UserData, "data");
    assert!(page.read(State::can_save_locations));
    page.send(Intent::SaveLocations);
    assert_eq!(page.kind(), MessageKind::Error);
    assert!(page.message().starts_with("Locations were not saved: Enter the full path of the user-data folder"));
    let moved = temp.path().join("moved");
    page.edit(Location::UserData, &format!(" {}\\ ", moved.display()));
    page.send(Intent::SaveLocations);
    assert_eq!(page.message(), "Saved. Restart Eterra's Gift to use the new locations.");
    assert_eq!(page.location(Location::UserData), moved.display().to_string());
    assert!(!page.read(State::can_save_locations));
    // Upper or lower case, a box that says what is saved is not a change.
    page.edit(Location::UserData, &moved.display().to_string().to_uppercase());
    assert!(!page.read(State::can_save_locations));
}

#[test]
fn a_switch_does_not_clear_an_error_it_did_not_cause() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    page.edit(Location::Dotnet, &temp.path().display().to_string());
    page.send(Intent::SaveLocations);
    let refused = page.message().to_owned();
    page.send(Intent::ToggleModule(0, false));
    assert_eq!(page.message(), refused);
}

#[test]
fn unrelated_saves_preserve_startup_errors_until_locations_are_corrected() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("settings.json"),
        serde_json::json!({
            "schemaVersion": 2, "gameExecutable": temp.path().join("missing.exe")
        })
        .to_string(),
    )
    .unwrap();
    let mut page = page(temp.path());
    let original = page.message().to_owned();
    assert!(original.contains("not found"));
    page.send(Intent::ToggleModule(0, false));
    assert_eq!(page.message(), original);
    assert_eq!(page.kind(), MessageKind::Error);
    page.edit(Location::GameExecutable, "");
    page.send(Intent::SaveLocations);
    assert_eq!(page.kind(), MessageKind::RestartRequired);
}

#[test]
fn empty_boxes_suggest_their_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let page = page(temp.path());
    assert_eq!(page.placeholder(Location::GameExecutable), "");
    assert_eq!(page.placeholder(Location::UserData), temp.path().join("app").join("data").display().to_string());
    assert_eq!(page.placeholder(Location::ModSources), "None");
    assert_eq!(page.placeholder(Location::Dotnet), "Not found");
    assert_eq!(page.placeholder(Location::MelonLoader), "The game folder");
}

#[test]
fn typing_during_an_async_location_save_survives_its_acknowledgement() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
    let service = SettingsService::new(store, &tasks());
    let mut page = Page::new(service.clone(), vec![trainer(true)]);
    let (entered, started) = std::sync::mpsc::channel();
    let (release, waiting) = std::sync::mpsc::channel();
    service
        .module("fixture")
        .update(move |_| {
            entered.send(()).unwrap();
            waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
    started.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let first = temp.path().join("first");
    let later = temp.path().join("later");
    page.edit(Location::UserData, &first.display().to_string());
    page.send(Intent::SaveLocations);
    // No transient message: what was said before stays while the save runs.
    assert_eq!((page.kind(), page.message().as_str()), (MessageKind::Saved, ""));
    assert!(page.read(State::saving));
    page.edit(Location::UserData, &later.display().to_string());
    // A newer switch is queued too; neither acknowledgement may erase the newer location edit.
    page.send(Intent::ToggleModule(0, false));
    let ticket = service.snapshot().submitted;
    release.send(()).unwrap();
    service.wait(ticket);
    page.refresh();
    assert_eq!(page.location(Location::UserData), later.display().to_string());
    assert!(page.read(State::can_save_locations));
    assert_eq!(service.snapshot().store.saved().user_data_directory, Some(first));
    assert!(!page.read(|state| state.module_enabled(0)));
}

#[test]
fn retry_acknowledges_and_normalizes_a_failed_location_write() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    let file = temp.path().join("settings.json");
    fs::create_dir(&file).unwrap();
    let moved = temp.path().join("moved");
    page.edit(Location::UserData, &format!(" {}\\ ", moved.display()));
    page.send(Intent::SaveLocations);
    assert_eq!(page.kind(), MessageKind::Error);
    assert!(page.read(State::can_retry));
    fs::remove_dir(&file).unwrap();
    page.send(Intent::Retry);
    assert_eq!(page.kind(), MessageKind::RestartRequired);
    assert_eq!(page.location(Location::UserData), moved.display().to_string());
    assert!(!page.read(State::can_retry));
    assert!(!page.read(State::can_save_locations));
}

#[test]
fn module_settings_preserve_unknown_fields_and_invalid_modules_are_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("settings.json");
    fs::write(
        &file,
        serde_json::json!({"schemaVersion": 2, "moduleSettings": {
            "trainer": { "autoConnect": "invalid", "unknown": 12 }, "second": { "future": [1,2] }
        }})
        .to_string(),
    )
    .unwrap();
    let service = SettingsService::inline(SettingsStore::open(&temp.path().join("app"), &file, &[]));
    let mut page = Page::new(service.clone(), vec![with_sections("trainer", true), with_sections("second", true)]);
    assert!(page.read(|state| state.module_problem(0).is_some()));
    assert!(!page.read(|state| state.modules[0].enabled_at_start));
    assert!(page.read(|state| state.modules[1].enabled_at_start));
    page.send(Intent::ToggleSetting(0, 1, false)); // Invalid module cannot overwrite its malformed slice.
    page.send(Intent::ToggleSetting(1, 1, false));
    let saved = service.snapshot();
    assert_eq!(saved.store.saved().module_settings["trainer"]["autoConnect"], "invalid");
    assert_eq!(saved.store.saved().module_settings["second"]["future"], serde_json::json!([1, 2]));
    assert_eq!(saved.store.saved().module_settings["second"]["autoConnect"], false);
}

#[test]
fn unreadable_settings_are_never_presented_as_durable_success() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("settings.json"), b"invalid").unwrap();
    let mut page = page(temp.path());
    let startup = page.message().to_owned();
    assert!(!startup.is_empty());
    // The startup error stays first, as a save that did not cause it never clears it.
    page.send(Intent::ToggleModule(0, false));
    assert_eq!((page.kind(), page.message().as_str()), (MessageKind::Error, startup.as_str()));
    assert!(!page.read(State::can_restart));
    // Once nothing else is said, the switch is said to hold for this run only.
    page.change(|state| state.external_error = None);
    page.send(Intent::ToggleModule(0, true));
    assert_eq!(page.kind(), MessageKind::MemoryOnly);
    assert!(page.message().contains("this run only"));
}

#[test]
fn closing_waits_for_accepted_saves_and_shows_a_failure_before_exiting() {
    for fails in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("settings.json");
        let service = SettingsService::new(SettingsStore::open(&temp.path().join("app"), &file, &[]), &tasks());
        let mut page = Page::new(service.clone(), vec![trainer(true)]);
        if fails {
            fs::create_dir(&file).unwrap();
        }
        let (release, waiting) = std::sync::mpsc::channel();
        service
            .module("fixture")
            .update(move |_| {
                waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                Ok(())
            })
            .unwrap();
        page.send(Intent::ToggleModule(0, false));
        page.send(Intent::CloseRequested);
        assert!(!page.closed && !page.shown, "a close waits for saves in flight");
        let revision = service.snapshot().submitted;
        release.send(()).unwrap();
        service.wait(revision);
        // Another callback can observe the failure while a close is pending.
        page.refresh();
        assert_eq!(page.closed, !fails);
        assert_eq!(page.shown, fails);
        if fails {
            page.send(Intent::CloseRequested);
            assert!(page.closed, "the second close goes ahead once the failure was shown");
        }
    }
}

#[test]
fn the_page_starts_by_reading_the_saves_and_what_started_modules_say() {
    let temp = tempfile::tempdir().unwrap();
    let modules = vec![with_sections("trainer", true), with_sections("off", false)];
    let store = TestStore::<SettingsPage>::new(State::new(inline(temp.path()).snapshot(), modules, None, None));
    let effects = store.take_effects();
    assert!(matches!(effects[..], [Effect::Observe, Effect::ReadStatus { module: 0, section: 0 }]), "{effects:?}");
}

#[test]
fn a_close_waits_for_the_pages_that_go_first() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = State::new(inline(temp.path()).snapshot(), vec![trainer(true)], None, None);
    state.set_close_first(1);
    let told = step::<SettingsPage>(&mut state, Intent::CloseRequested);
    assert_eq!(told.outputs, [Output::Lifecycle(Lifecycle::CloseRequested)]);
    assert!(told.effects.is_empty(), "saves pause only once the page before has closed");
    assert!(
        step::<SettingsPage>(&mut state, Intent::CloseRequested).outputs.is_empty(),
        "a second click asks no one again"
    );
    state.restart_requested = true;
    let told = step::<SettingsPage>(&mut state, Intent::CloseAnswered(0, CloseAnswer::StayingOpen));
    assert!(matches!(told.effects[..], [Effect::RequestRestart(false)]), "the window stays, and nothing restarts");
    assert_eq!(told.outputs, [Output::Lifecycle(Lifecycle::CloseCalledOff)], "the pages that went first hear it");
    step::<SettingsPage>(&mut state, Intent::CloseRequested);
    assert!(
        step::<SettingsPage>(&mut state, Intent::CloseAnswered(1, CloseAnswer::Closed)).effects.is_empty(),
        "a page not asked"
    );
    let told = step::<SettingsPage>(&mut state, Intent::CloseAnswered(0, CloseAnswer::Closed));
    assert!(matches!(told.effects[..], [Effect::TryPause]), "{:?}", told.effects);
    let late = step::<SettingsPage>(&mut state, Intent::CloseAnswered(0, CloseAnswer::Closed));
    assert!(late.effects.is_empty(), "an answer nothing asked for");
}

#[test]
fn a_failed_close_takes_the_restart_back() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("settings.json");
    let service = SettingsService::new(SettingsStore::open(&temp.path().join("app"), &file, &[]), &tasks());
    let mut page = Page::new(service.clone(), vec![trainer(true)]);
    fs::create_dir(&file).unwrap();
    let (release, waiting) = std::sync::mpsc::channel();
    service
        .module("fixture")
        .update(move |_| {
            waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
    page.send(Intent::CloseRequested);
    page.change(|state| state.restart_requested = true);
    page.restart = true;
    let revision = service.snapshot().submitted;
    release.send(()).unwrap();
    service.wait(revision);
    page.refresh();
    assert!(page.shown && !page.closed);
    assert!(!page.restart, "the failure is shown instead; nothing restarts");
}

#[test]
fn rows_change_version_only_when_they_could_have_changed() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    let (rows, boxes) = (page.read(State::rows_version), page.read(State::boxes_version));
    page.edit(Location::Dotnet, "typing");
    assert_eq!(page.read(State::rows_version), rows, "a keystroke leaves the module rows alone");
    assert_ne!(page.read(State::boxes_version), boxes);
    let boxes = page.read(State::boxes_version);
    page.send(Intent::ToggleModule(0, false));
    assert_ne!(page.read(State::rows_version), rows);
    assert_eq!(page.read(State::boxes_version), boxes, "a switch leaves the boxes alone");
}
