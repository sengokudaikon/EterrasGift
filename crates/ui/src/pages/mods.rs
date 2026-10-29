//! Mods keeps interactive drafts in memory. All library reads and writes enter one ordered worker;
//! completions are observed on the UI thread without waiting for disk or MSBuild.

use crate::models::update_rows;
use crate::shell::{Page, ShellHandle};
use crate::{MainWindow, MessageTone, ModLogRow, ModRow, ModSettingRow, ModsState, SettingChoiceRow, SourceRow, background};
use eg_companion::launch::host::LaunchMode;
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::mods::{
    inventory::{self, Inventory},
    library::{InstalledMod, ModLibrary},
    loader_state::LoaderPreparation,
    page::{by_hand, follow_preparation, installed_message, keep_each, running_state},
    preferences::{ModSetting, PreferenceKind},
    sources::{ModSources, SourceError, SourceSnapshot},
    worker::ModWorker,
};
use eg_core::operations::Cancellation;
use eg_core::phrase::Phrase;
use eg_core::settings_service::SettingsService;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, VecDeque},
    io,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, mpsc},
    time::Duration,
};

/// How often the page looks for the worker's answers while it waits on one.
const POLL: Duration = Duration::from_millis(40);

pub struct ModsPage {
    pub worker: ModWorker,
    pub launcher: Arc<GameLauncher>,
    pub settings: SettingsService,
    pub sources: Arc<ModSources>,
}

/// Cancels active compiler work even when the event loop exits due to a fault rather than a close request.
pub fn cancel_work(window: &MainWindow) {
    window.global::<ModsState>().invoke_stop_work();
}

#[derive(Default)]
struct ChangeResult {
    message: Option<String>,
    select: Option<String>,
    removed: Option<(InstalledMod, usize)>,
}
struct Observation {
    inventory: Inventory,
    version: Option<String>,
    change: ChangeResult,
    running: bool,
    modded: bool,
    undo_held: bool,
}
type SettingKey = (String, String);
struct Pending {
    revision: u64,
    receive: mpsc::Receiver<io::Result<Observation>>,
    settings: Vec<SettingKey>,
    clears_build: bool,
    failure: Failure,
}
/// How a failed edit is said. An edit of the library's own records (on or off, a setting, Remove,
/// Undo, Revert) says the change could not be kept; adding a DLL or installing a build says the failure as it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Failure {
    Kept,
    Raw,
}
const KEPT_FAILED: Phrase = Phrase::new("The change could not be kept: {error}");
const READING: Phrase = Phrase::new("Reading the mods…");
const OPEN_FOLDER: Phrase = Phrase::new("Open the Mods folder");
const NO_GAME: Phrase = Phrase::new("Last Epoch wasn't found. Choose the game executable in Settings.");
const OPEN_LOG: Phrase = Phrase::new("Open MelonLoader's log from the last game with mods");
const NO_LOG: Phrase = Phrase::new("No log yet. MelonLoader writes one when you play with mods.");
const CHANGES_APPLY: Phrase = Phrase::new("Changes apply the next time you play with mods.");
const NO_VERSION: Phrase = Phrase::new("No version");
const DID_NOT_LOAD: Phrase = Phrase::new("Didn't load");
const CANT_LOAD: Phrase = Phrase::new("Can't load");
const VERSION: Phrase = Phrase::new("Version {version}");
const VERSION_BY: Phrase = Phrase::new("Version {version} by {author}");
const NO_VERSION_BY: Phrase = Phrase::new("No version by {author}");
const DID_NOT_LOAD_LAST_TIME: Phrase = Phrase::new("Didn't load last time. {problem}");
const BUILDING: Phrase = Phrase::new("Building…");
const UPDATE: Phrase = Phrase::new("Update");
const REBUILD: Phrase = Phrase::new("Rebuild");
const INSTALL_VERSION: Phrase = Phrase::new("Install {version}");
const INSTALL_BUILD: Phrase = Phrase::new("Install build");
const FROM_SOURCES: Phrase = Phrase::new("From sources in {folder}");
const BUILD_NEWER: Phrase = Phrase::new("Build the newer version and install it");
const BUILD_FROM_PROJECT: Phrase = Phrase::new("Build it from its project and install it");
const UPDATE_AVAILABLE: Phrase = Phrase::new("Update available.");
const NOT_BUILT: Phrase = Phrase::new("Not built yet.");
const LATEST_NOT_INSTALLED: Phrase = Phrase::new("The latest build isn't installed.");
const LATEST_NOT_INSTALLED_CHANGED: Phrase = Phrase::new("The latest build isn't installed. Its sources changed since.");
const LATEST_VERSION_NOT_INSTALLED: Phrase = Phrase::new("The latest build ({version}) isn't installed.");
const LATEST_VERSION_NOT_INSTALLED_CHANGED: Phrase = Phrase::new("The latest build ({version}) isn't installed. Its sources changed since.");
const SOURCES_CHANGED: Phrase = Phrase::new("Its sources changed since this build.");
const OTHER_SETTINGS: Phrase = Phrase::new("Other settings");
const REPEATED: Phrase = Phrase::new("Repeated {count} times, last at {time}");
const LIBRARY_STOPPED: Phrase = Phrase::new("The mod library stopped.");
const NO_SDK: Phrase =
    Phrase::new("Building mods from their sources needs the .NET SDK, which was not found. Install it, or choose its folder in Settings, to see and build the mod projects here.");
const SOURCES_UNREADABLE: Phrase = Phrase::new("The mod sources in {folder} could not be read: {error}");
const REMOVED: Phrase = Phrase::new("{name} is removed. Its DLL leaves the Mods folder the next time you play with mods; its settings stay in MelonPreferences.cfg.");
const ADDED: Phrase = Phrase::new("Added {name}. It loads the next time you play with mods.");
const PICKER_TITLE: Phrase = Phrase::new("Add a mod");
const PICKER_FILTER: Phrase = Phrase::new("Mod DLL");
const PICKER_FAILED: Phrase = Phrase::new("The file picker could not be opened: {error}");
const BUILD_NEEDS_GAME: Phrase = Phrase::new("{name} is built against the game's MelonLoader, and Last Epoch was not found. Choose the game executable in Settings.");
const BUILDING_NAME: Phrase = Phrase::new("Building {name}…");

impl Failure {
    fn message(self, error: &io::Error) -> String {
        match self {
            Failure::Kept => KEPT_FAILED.with(&[("error", error)]),
            Failure::Raw => error.to_string(),
        }
    }
}
struct View {
    rows: Rc<VecModel<ModRow>>,
    source_rows: Rc<VecModel<SourceRow>>,
    setting_rows: Rc<VecModel<ModSettingRow>>,
    log_rows: Rc<VecModel<ModLogRow>>,
    services: ModsPage,
    window: slint::Weak<MainWindow>,
    inventory: RefCell<Inventory>,
    version: RefCell<Option<String>>,
    sources: RefCell<SourceSnapshot>,
    selection: Cell<usize>,
    revision: Cell<u64>,
    action_revision: Cell<u64>,
    preferred_selection: RefCell<Option<String>>,
    pending: RefCell<VecDeque<Pending>>,
    unsaved: RefCell<BTreeSet<(String, String)>>,
    editing: RefCell<BTreeSet<(String, String)>>,
    removed: RefCell<Option<(InstalledMod, usize)>>,
    message: RefCell<Option<(String, bool)>>,
    read: Cell<bool>,
    visible: Cell<bool>,
    running: Cell<bool>,
    modded_run: Cell<bool>,
    launching: Cell<bool>,
    building: RefCell<Option<String>>,
    build_cancel: RefCell<Option<Cancellation>>,
    sdk_said: Cell<bool>,
    watch_pending: Cell<bool>,
    watch_generation: Cell<u64>,
    follow: Cell<u8>,
    adopted: RefCell<Vec<String>>,
    deleted: RefCell<Vec<String>>,
    close_error: Cell<bool>,
    closing: Cell<bool>,
    /// Takes the worker's answers while any is awaited, and stops once none is (`keep_polling`).
    polling: slint::Timer,
    /// Looks at the game every `GameLauncher::WATCH_INTERVAL` while the page shows, and not otherwise.
    watching: slint::Timer,
}

pub(crate) fn bind(window: &MainWindow, page: ModsPage, shell: &ShellHandle) -> Rc<dyn Fn() -> crate::pages::settings::CloseDecision> {
    let view = Rc::new(View {
        rows: Rc::new(VecModel::default()),
        source_rows: Rc::new(VecModel::default()),
        setting_rows: Rc::new(VecModel::default()),
        log_rows: Rc::new(VecModel::default()),
        services: page,
        window: window.as_weak(),
        inventory: RefCell::new(Inventory::default()),
        version: RefCell::new(None),
        sources: RefCell::new(SourceSnapshot::default()),
        selection: Cell::new(0),
        revision: Cell::new(0),
        action_revision: Cell::new(0),
        preferred_selection: RefCell::new(None),
        pending: RefCell::new(VecDeque::new()),
        unsaved: RefCell::new(BTreeSet::new()),
        editing: RefCell::new(BTreeSet::new()),
        removed: RefCell::new(None),
        message: RefCell::new(None),
        read: Cell::new(false),
        visible: Cell::new(shell.current() == Page::Mods),
        running: Cell::new(false),
        modded_run: Cell::new(false),
        launching: Cell::new(false),
        building: RefCell::new(None),
        build_cancel: RefCell::new(None),
        close_error: Cell::new(false),
        closing: Cell::new(false),
        sdk_said: Cell::new(false),
        watch_pending: Cell::new(false),
        watch_generation: Cell::new(0),
        follow: Cell::new(0),
        adopted: RefCell::new(Vec::new()),
        deleted: RefCell::new(Vec::new()),
        polling: slint::Timer::default(),
        watching: slint::Timer::default(),
    });
    let state = window.global::<ModsState>();
    state.set_mods(ModelRc::from(view.rows.clone()));
    state.set_sources(ModelRc::from(view.source_rows.clone()));
    state.set_settings(ModelRc::from(view.setting_rows.clone()));
    state.set_log_lines(ModelRc::from(view.log_rows.clone()));
    view.bind_callbacks(window);
    shell.on_page_entered({
        let view = view.clone();
        move |page| {
            view.commit_settings();
            view.visible.set(page == Page::Mods);
            view.keep_watching();
            if page == Page::Mods {
                view.refresh();
            }
        }
    });
    // A task that panics ends the app at once, whoever queued it: a start's preparation from Home shares the worker.
    view.services.worker.on_fault(|| {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
    view.keep_watching();
    if view.visible.get() {
        view.refresh();
    }
    Rc::new(move || view.close_decision())
}

impl View {
    fn show(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<ModsState>();
        let inventory = self.inventory.borrow();
        let sources = self.sources.borrow();
        let building = self.building.borrow();
        let snapshot = self.services.settings.snapshot();
        let folder = snapshot
            .store
            .saved()
            .mod_sources_directory
            .clone()
            .or_else(|| snapshot.store.paths().mod_sources_directory());
        let readiness = inventory.readiness(
            self.version.borrow().as_deref(),
            self.running.get(),
            self.modded_run.get(),
            self.launching.get() || self.services.launcher.is_starting(),
            jiff::Timestamp::now(),
        );
        state.set_readiness(if self.read.get() { readiness.text.into() } else { READING.get().into() });
        state.set_readiness_facts(ModelRc::new(VecModel::from(if self.read.get() {
            readiness.facts.into_iter().map(SharedString::from).collect::<Vec<_>>()
        } else {
            Vec::new()
        })));
        state.set_readiness_tone(if readiness.warning { MessageTone::Warning } else { MessageTone::Info });
        state.set_can_play(self.read.get() && readiness.can_play && !self.closing.get());
        state.set_is_read(self.read.get());
        state.set_can_open_folder(inventory.mods_folder.is_some());
        state.set_can_open_log(inventory.loader_log.is_some());
        let folder_tip = if inventory.mods_folder.is_some() {
            OPEN_FOLDER.get()
        } else if self.read.get() {
            NO_GAME.get()
        } else {
            READING.get()
        };
        state.set_open_folder_tip(folder_tip.into());
        state.set_open_log_tip(
            if inventory.loader_log.is_some() {
                OPEN_LOG.get()
            } else if inventory.mods_folder.is_none() {
                folder_tip
            } else {
                NO_LOG.get()
            }
            .into(),
        );
        state.set_installed_count(inventory.mods.len() as i32);
        state.set_enabled_count(inventory.mods.iter().filter(|item| item.installed.enabled).count() as i32);
        state.set_selected_index(if inventory.mods.is_empty() { -1 } else { self.selection.get() as i32 });
        state.set_busy(!self.read.get());
        state.set_building(building.is_some());
        let draft_count = inventory.mods.iter().map(|item| item.draft_count()).sum::<usize>();
        state.set_draft_count(draft_count as i32);
        state.set_can_undo(self.removed.borrow().is_some());
        let message = self.message.borrow();
        state.set_change_is_draft(message.is_none() && draft_count > 0);
        let text = message
            .as_ref()
            .map(|(message, _)| message.as_str())
            .unwrap_or(if draft_count > 0 { CHANGES_APPLY.get() } else { "" });
        state.set_change_message(text.into());
        state.set_change_tone(if message.as_ref().is_some_and(|(_, error)| *error) {
            MessageTone::Error
        } else {
            MessageTone::Info
        });
        update_rows(
            &self.rows,
            inventory
                .mods
                .iter()
                .map(|item| ModRow {
                    name: item.name.as_str().into(),
                    version: item.version.as_deref().unwrap_or(NO_VERSION.get()).into(),
                    author: item.author.as_deref().unwrap_or_default().into(),
                    enabled: item.installed.enabled,
                    drafted: item.draft_count() > 0,
                    problem: if item.installed.enabled { item.problem.as_deref().unwrap_or_default() } else { "" }.into(),
                    problem_label: if item.installed.enabled && item.problem.is_some() {
                        if item.loaded_last_time == Some(false) { DID_NOT_LOAD.get() } else { CANT_LOAD.get() }
                    } else {
                        ""
                    }
                    .into(),
                })
                .collect(),
        );
        update_rows(
            &self.source_rows,
            sources
                .not_installed
                .iter()
                .map(|project| SourceRow {
                    name: project.name.as_str().into(),
                    project: breakable_path(&project.project_file.display().to_string()).into(),
                    version: project.version.as_deref().unwrap_or_default().into(),
                    folder: folder.as_deref().map(|folder| project.source_folder(folder)).unwrap_or_default().into(),
                    busy: building.as_ref().is_some_and(|file| file.eq_ignore_ascii_case(&project.file())),
                })
                .collect(),
        );
        let selected = inventory.mods.get(self.selection.get());
        state.set_selected_name(selected.map(|item| item.name.as_str()).unwrap_or_default().into());
        // The pane's line under the name; the DLL's file name has a line of its own.
        state.set_selected_detail(
            selected
                .map(|item| match (&item.version, &item.author) {
                    (Some(version), Some(author)) => VERSION_BY.with(&[("version", version), ("author", author)]),
                    (Some(version), None) => VERSION.with(&[("version", version)]),
                    (None, Some(author)) => NO_VERSION_BY.with(&[("author", author)]),
                    (None, None) => NO_VERSION.into(),
                })
                .unwrap_or_default()
                .into(),
        );
        state.set_selected_file(selected.map(|item| item.installed.file.as_str()).unwrap_or_default().into());
        state.set_selected_description(selected.and_then(|item| item.description.as_deref()).unwrap_or_default().into());
        state.set_selected_problem(
            selected
                .and_then(|item| {
                    item.problem.as_ref().map(|problem| {
                        if item.loaded_last_time == Some(false) {
                            DID_NOT_LOAD_LAST_TIME.with(&[("problem", problem)])
                        } else {
                            problem.clone()
                        }
                    })
                })
                .unwrap_or_default()
                .into(),
        );
        state.set_selected_state(selected.map(|item| item.state_text()).unwrap_or_default().into());
        state.set_selected_enabled(selected.is_some_and(|item| item.installed.enabled));
        state.set_previous_build(selected.is_some_and(|item| item.log_from_other_build));
        let build = selected.and_then(|item| sources.builds.get(&item.installed.file.to_lowercase()));
        // Its project now has a newer <Version> than the installed DLL was built with: Update builds and installs it.
        let update = build
            .zip(selected)
            .is_some_and(|(build, item)| build.project.update_available(item.project_version.as_deref()));
        state.set_rebuild_text(
            if selected.is_some_and(|item| building.as_ref().is_some_and(|file| file.eq_ignore_ascii_case(&item.installed.file))) {
                BUILDING.get()
            } else if update {
                UPDATE.get()
            } else {
                REBUILD.get()
            }
            .into(),
        );
        state.set_install_build_text(
            build
                .zip(selected)
                .and_then(|(build, item)| build.version.as_ref().filter(|version| Some(version.as_str()) != item.version.as_deref()))
                .map(|version| INSTALL_VERSION.with(&[("version", version)]))
                .unwrap_or_else(|| INSTALL_BUILD.into())
                .into(),
        );
        state.set_selected_source(
            build
                .zip(folder.as_deref())
                .map(|(build, folder)| FROM_SOURCES.with(&[("folder", &build.project.source_folder(folder))]))
                .unwrap_or_default()
                .into(),
        );
        state.set_selected_source_path(
            build
                .map(|build| breakable_path(&build.project.project_file.display().to_string()))
                .unwrap_or_default()
                .into(),
        );
        state.set_selected_has_sources(build.is_some());
        state.set_rebuild_tip(if update { BUILD_NEWER.get() } else { BUILD_FROM_PROJECT.get() }.into());
        state.set_can_install_build(
            build
                .zip(selected)
                .is_some_and(|(build, item)| build.can_install(&item.installed.hash, item.project_version.as_deref())),
        );
        state.set_selected_build_status(
            build
                .zip(selected)
                .map(|(build, item)| {
                    if update {
                        UPDATE_AVAILABLE.into()
                    } else if !build.is_built() {
                        NOT_BUILT.into()
                    } else if !build.is_installed(&item.installed.hash) {
                        let version = build.version.as_ref().filter(|version| Some(version.as_str()) != item.version.as_deref());
                        match (version, build.changed_since_build) {
                            (Some(version), true) => LATEST_VERSION_NOT_INSTALLED_CHANGED.with(&[("version", version)]),
                            (Some(version), false) => LATEST_VERSION_NOT_INSTALLED.with(&[("version", version)]),
                            (None, true) => LATEST_NOT_INSTALLED_CHANGED.into(),
                            (None, false) => LATEST_NOT_INSTALLED.into(),
                        }
                    } else if build.changed_since_build {
                        SOURCES_CHANGED.into()
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default()
                .into(),
        );
        let mut group: Option<&str> = None;
        let settings = selected
            .map(|item| {
                item.settings
                    .iter()
                    .enumerate()
                    .map(|(index, setting)| {
                        let next = setting.group.as_deref().or(group.map(|_| OTHER_SETTINGS.get()));
                        let heading = if next == group { "" } else { next.unwrap_or_default() };
                        group = next;
                        let choices = setting
                            .choices
                            .iter()
                            .map(|choice| SettingChoiceRow {
                                value: choice.value.as_str().into(),
                                label: choice.label.as_str().into(),
                            })
                            .collect::<Vec<_>>();
                        // The same choices keep their models, so an open choice list isn't rebuilt under the pointer.
                        let (choices, choice_labels) = self
                            .setting_rows
                            .row_data(index)
                            .filter(|old| old.category == setting.category && old.key == setting.key && old.choices.iter().eq(choices.iter().cloned()))
                            .map(|old| (old.choices, old.choice_labels))
                            .unwrap_or_else(|| {
                                let labels = choices.iter().map(|choice| choice.label.clone()).collect::<Vec<_>>();
                                (ModelRc::new(VecModel::from(choices)), ModelRc::new(VecModel::from(labels)))
                            });
                        ModSettingRow {
                            category: setting.category.as_str().into(),
                            key: setting.key.as_str().into(),
                            name: setting.name.as_str().into(),
                            description: setting.description.as_str().into(),
                            group: heading.into(),
                            kind: if setting.kind == PreferenceKind::Switch {
                                "switch"
                            } else if setting.is_choice() {
                                "choice"
                            } else {
                                "text"
                            }
                            .into(),
                            value: setting.value.as_str().into(),
                            is_on: setting.is_on,
                            unit: setting.unit.as_deref().unwrap_or_default().into(),
                            drafted: setting.is_drafted(),
                            was: setting.was_text().into(),
                            error: setting.error().unwrap_or_default().into(),
                            choice_label: setting
                                .choices
                                .iter()
                                .find(|choice| choice.value == setting.value)
                                .map(|choice| choice.label.as_str())
                                .unwrap_or(&setting.value)
                                .into(),
                            // A value the mod's list doesn't name shows as it is, with no choice in the list held.
                            choice_index: setting.choices.iter().position(|choice| choice.value == setting.value).map_or(-1, |index| index as i32),
                            choices,
                            choice_labels,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        update_rows(&self.setting_rows, settings);
        update_rows(
            &self.log_rows,
            selected
                .map(|item| {
                    item.log
                        .iter()
                        .map(|run| {
                            let seconds = run.line.time.as_secs();
                            let last = run.last.as_secs();
                            ModLogRow {
                                time: format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60).into(),
                                text: run.line.text.as_str().into(),
                                error: run.line.is_error,
                                count: run.count as i32,
                                count_tip: REPEATED
                                    .with(&[("count", &run.count), ("time", &format!("{:02}:{:02}:{:02}", last / 3600, last / 60 % 60, last % 60))])
                                    .into(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
}

impl View {
    fn say(&self, text: impl Into<String>, error: bool) {
        *self.message.borrow_mut() = Some((text.into(), error));
        self.removed.borrow_mut().take();
        if error && self.closing.get() {
            self.close_error.set(true);
        }
    }
    fn enqueue(self: &Rc<Self>, change: impl FnOnce(&ModLibrary) -> io::Result<ChangeResult> + Send + 'static) {
        self.enqueue_tracked(change, Vec::new(), false, true, Failure::Kept);
    }
    fn enqueue_tracked(
        self: &Rc<Self>,
        change: impl FnOnce(&ModLibrary) -> io::Result<ChangeResult> + Send + 'static,
        settings: Vec<SettingKey>,
        clears_build: bool,
        mutation: bool,
        failure: Failure,
    ) {
        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        if mutation {
            self.action_revision.set(revision);
        }
        let launcher = self.services.launcher.clone();
        let undo = self.removed.borrow().as_ref().map(|(item, _)| item.clone());
        let receive = self.services.worker.enqueue(move |library| {
            let result = change(library);
            // On write failure retain an error, then request a fresh observation after all later edits.
            let change = result?;
            let install = launcher.find_install();
            let root = install.as_ref().map(|install| launcher.loader_root(install));
            let version = install.as_ref().and_then(|install| launcher.read_game_version(install));
            let inventory = inventory::read(install.as_ref(), library, root.as_deref());
            let (running, modded) = running_state(&launcher, inventory.loader_log.as_deref());
            let undo_held = undo.as_ref().is_none_or(|item| library.holds(item));
            Ok(Observation {
                inventory,
                version,
                change,
                running,
                modded,
                undo_held,
            })
        });
        match receive {
            Ok(receive) => {
                self.pending.borrow_mut().push_back(Pending {
                    revision,
                    receive,
                    settings,
                    clears_build,
                    failure,
                });
                self.keep_polling();
            }
            Err(error) => {
                self.retain_failed_settings(settings);
                if clears_build {
                    self.finish_build();
                }
                self.say(failure.message(&error), true);
            }
        }
        self.show();
    }
    fn retain_failed_settings(&self, settings: Vec<SettingKey>) {
        self.editing.borrow_mut().extend(settings.iter().cloned());
        self.unsaved.borrow_mut().extend(settings);
    }
    fn finish_build(&self) {
        self.building.borrow_mut().take();
        self.build_cancel.borrow_mut().take();
    }
    fn refresh(self: &Rc<Self>) {
        self.commit_settings();
        self.editing.borrow_mut().clear();
        self.enqueue_tracked(|_| Ok(ChangeResult::default()), Vec::new(), false, false, Failure::Kept);
    }
    fn poll(self: &Rc<Self>) {
        let mut refresh = false;
        let mut arrived = false;
        loop {
            let next = {
                let mut pending = self.pending.borrow_mut();
                match pending.front().map(|job| job.receive.try_recv()) {
                    Some(Ok(result)) => Some((pending.pop_front().unwrap(), result)),
                    Some(Err(mpsc::TryRecvError::Disconnected)) => Some((pending.pop_front().unwrap(), Err(io::Error::other(LIBRARY_STOPPED.get())))),
                    _ => None,
                }
            };
            let Some((pending, result)) = next else {
                break;
            };
            arrived = true;
            let revision = pending.revision;
            if pending.clears_build {
                self.finish_build();
            }
            match result {
                Err(error) => {
                    self.retain_failed_settings(pending.settings);
                    self.say(pending.failure.message(&error), true);
                    refresh = true;
                }
                Ok(observation) => {
                    // What any read found changed in the Mods folder is said once, by the read that is shown.
                    self.adopted.borrow_mut().extend(observation.inventory.changes.adopted.iter().cloned());
                    self.deleted.borrow_mut().extend(observation.inventory.changes.removed.iter().cloned());
                    // A later read invalidates an old snapshot, not the action's receipt or its Undo offer.
                    if revision >= self.action_revision.get() {
                        if let Some(message) = observation.change.message {
                            self.say(message, false);
                        }
                        if let Some(removed) = observation.change.removed {
                            *self.removed.borrow_mut() = Some(removed);
                        }
                        if let Some(select) = observation.change.select {
                            *self.preferred_selection.borrow_mut() = Some(select);
                        }
                    }
                    if revision != self.revision.get() {
                        continue;
                    }
                    self.watch_generation.set(self.watch_generation.get() + 1);
                    self.running.set(observation.running);
                    self.modded_run.set(observation.modded);
                    if !observation.undo_held {
                        self.removed.borrow_mut().take();
                    }
                    let selected = self
                        .preferred_selection
                        .borrow_mut()
                        .take()
                        .or_else(|| self.inventory.borrow().mods.get(self.selection.get()).map(|item| item.installed.file.clone()));
                    let mut inventory = observation.inventory;
                    if inventory.error.is_some() && !self.unsaved.borrow().is_empty() {
                        // A failed profile read must not erase the only editable copy of a failed save.
                        inventory.mods = self.inventory.borrow().mods.clone();
                    }
                    // Preserve text that is still being typed; it has not been submitted to the library yet.
                    for item in &mut inventory.mods {
                        for setting in &mut item.settings {
                            if holds(&self.editing.borrow(), setting)
                                && let Some(old) = self
                                    .inventory
                                    .borrow()
                                    .mods
                                    .iter()
                                    .flat_map(|item| &item.settings)
                                    .find(|old| old.category == setting.category && old.key == setting.key)
                            {
                                setting.value.clone_from(&old.value);
                                setting.is_on = old.is_on;
                            }
                        }
                    }
                    self.selection.set(
                        selected
                            .and_then(|file| inventory.mods.iter().position(|item| item.installed.file.eq_ignore_ascii_case(&file)))
                            .unwrap_or(0),
                    );
                    let adopted = std::mem::take(&mut *self.adopted.borrow_mut());
                    let deleted = std::mem::take(&mut *self.deleted.borrow_mut());
                    if let Some(changed) = by_hand(&adopted, &deleted) {
                        self.say(changed, false);
                    } else if let Some(problem) = &inventory.settings_problem {
                        self.say(problem, true);
                    }
                    *self.inventory.borrow_mut() = inventory;
                    *self.version.borrow_mut() = observation.version;
                    self.read.set(true);
                    self.read_sources(revision);
                }
            }
        }
        if arrived {
            self.show();
        }
        if refresh && !self.services.worker.faulted() {
            self.enqueue_tracked(|_| Ok(ChangeResult::default()), Vec::new(), false, false, Failure::Kept);
        }
        self.keep_polling();
    }
    /// Polls every [`POLL`] while the worker owes an answer, and not otherwise: an idle page wakes nothing.
    fn keep_polling(self: &Rc<Self>) {
        let waiting = !self.pending.borrow().is_empty();
        if waiting && !self.polling.running() {
            let view = Rc::downgrade(self);
            self.polling.start(slint::TimerMode::Repeated, POLL, move || {
                if let Some(view) = view.upgrade() {
                    view.poll();
                }
            });
        } else if !waiting {
            self.polling.stop();
        }
    }
    /// Follows the game while the page shows: Home and the trainer follow it on their own.
    fn keep_watching(self: &Rc<Self>) {
        if self.visible.get() && !self.watching.running() {
            let view = Rc::downgrade(self);
            self.watching.start(slint::TimerMode::Repeated, GameLauncher::WATCH_INTERVAL, move || {
                if let Some(view) = view.upgrade() {
                    view.watch_game();
                }
            });
        } else if !self.visible.get() {
            self.watching.stop();
        }
    }
    fn select(self: &Rc<Self>, index: i32) {
        if index < 0 || index as usize >= self.inventory.borrow().mods.len() {
            return;
        }
        self.commit_settings();
        self.selection.set(index as usize);
        self.show();
    }
    fn toggle(self: &Rc<Self>, index: i32) {
        self.commit_settings();
        let Some((file, enabled)) = self.inventory.borrow_mut().mods.get_mut(index as usize).map(|item| {
            item.installed.enabled = !item.installed.enabled;
            (item.installed.file.clone(), item.installed.enabled)
        }) else {
            return;
        };
        self.message.borrow_mut().take();
        self.removed.borrow_mut().take();
        self.enqueue(move |library| {
            library.set_enabled(&file, enabled)?;
            Ok(ChangeResult::default())
        });
    }
    fn edit_setting(&self, index: i32, value: String, toggle: Option<bool>) {
        let mut inventory = self.inventory.borrow_mut();
        let Some(setting) = inventory.mods.get_mut(self.selection.get()).and_then(|item| item.settings.get_mut(index as usize)) else {
            return;
        };
        if let Some(on) = toggle {
            setting.is_on = on;
        } else {
            setting.value = value;
        }
        self.unsaved.borrow_mut().insert((setting.category.clone(), setting.key.clone()));
        self.editing.borrow_mut().insert((setting.category.clone(), setting.key.clone()));
        self.message.borrow_mut().take();
        self.removed.borrow_mut().take();
    }
    fn commit_settings(self: &Rc<Self>) {
        let unsaved = std::mem::take(&mut *self.unsaved.borrow_mut());
        if unsaved.is_empty() {
            return;
        }
        let changes: Vec<_> = self
            .inventory
            .borrow()
            .mods
            .iter()
            .flat_map(|item| &item.settings)
            .filter(|setting| holds(&unsaved, setting))
            .map(|setting| (setting.category.clone(), setting.key.clone(), setting.draft().map(|value| value.to_json()).transpose()))
            .collect();
        self.enqueue_tracked(
            move |library| {
                // One inaccessible key must not discard later independent edits. Retrying a failed batch is idempotent.
                keep_each(changes, |(category, key, value)| value.and_then(|value| library.set_setting(&category, &key, value)))?;
                Ok(ChangeResult::default())
            },
            unsaved.into_iter().collect(),
            false,
            true,
            Failure::Kept,
        );
    }
    fn close_decision(self: &Rc<Self>) -> crate::pages::settings::CloseDecision {
        use crate::pages::settings::CloseDecision;
        if !self.closing.replace(true) {
            if let Some(cancel) = self.build_cancel.borrow().as_ref() {
                cancel.cancel();
            }
            self.commit_settings();
            // Play goes while the page closes.
            self.show();
        }
        self.poll();
        if !self.pending.borrow().is_empty() || self.building.borrow().is_some() {
            return CloseDecision::Wait;
        }
        self.closing.set(false);
        if self.close_error.replace(false) {
            if let Some(window) = self.window.upgrade() {
                window.global::<crate::Nav>().invoke_navigate("mods".into());
            }
            CloseDecision::ShowFailure
        } else {
            CloseDecision::Close
        }
    }
    fn read_sources(self: &Rc<Self>, revision: u64) {
        let snapshot = self.services.settings.snapshot();
        let folder = snapshot
            .store
            .saved()
            .mod_sources_directory
            .clone()
            .or_else(|| snapshot.store.paths().mod_sources_directory());
        let sdk = snapshot.store.saved().dotnet_directory.clone();
        let files = self.inventory.borrow().mods.iter().map(|item| item.installed.file.clone()).collect::<Vec<_>>();
        let sources = self.services.sources.clone();
        let cancel = self.services.worker.cancellation();
        let view = self.clone();
        let shown = folder.clone();
        background::run(
            move || {
                let result = sources.read(folder.as_deref(), sdk.as_deref(), &files, &cancel);
                (result, sources.sdk_missing())
            },
            move |(result, sdk_missing)| {
                if view.revision.get() != revision || view.closing.get() {
                    return;
                }
                match result {
                    Ok(sources) => {
                        *view.sources.borrow_mut() = sources;
                        if sdk_missing && !view.sdk_said.replace(true) {
                            view.say(NO_SDK.get(), true);
                        }
                    }
                    // A read cut short by the app's close says nothing.
                    Err(SourceError::Cancelled) => {}
                    Err(error) => {
                        *view.sources.borrow_mut() = SourceSnapshot::default();
                        view.say(sources_unreadable(shown.as_deref(), &error), true);
                    }
                }
                view.show();
            },
        );
    }
    fn watch_game(self: &Rc<Self>) {
        if self.watch_pending.replace(true) {
            return;
        }
        let generation = self.watch_generation.get() + 1;
        self.watch_generation.set(generation);
        let launcher = self.services.launcher.clone();
        let log = self.inventory.borrow().loader_log.clone();
        let view = self.clone();
        background::run(
            move || running_state(&launcher, log.as_deref()),
            move |(running, modded)| {
                view.watch_pending.set(false);
                if generation != view.watch_generation.get() || view.closing.get() {
                    return;
                }
                let was_running = view.running.replace(running);
                view.modded_run.set(modded);
                let inventory = view.inventory.borrow();
                let preparing = inventory.loader.as_ref().is_some_and(|loader| loader.preparation == LoaderPreparation::WillPrepare);
                let typing = inventory
                    .mods
                    .iter()
                    .flat_map(|item| &item.settings)
                    .any(|setting| setting.kind != PreferenceKind::Switch && !setting.is_choice() && holds(&view.unsaved.borrow(), setting));
                drop(inventory);
                let (refresh, follow) = follow_preparation(was_running, running, modded, preparing, typing, view.follow.get());
                // Do not pile up full inventory reads behind a slow source copy or profile write.
                if refresh && view.pending.borrow().is_empty() {
                    view.follow.set(follow);
                    view.refresh();
                } else {
                    if !refresh {
                        view.follow.set(follow);
                    }
                    view.show();
                }
            },
        );
    }
}

/// Whether `keys` holds the setting's key, compared in place rather than built for each lookup: the sets hold only the
/// few keys being edited.
fn holds(keys: &BTreeSet<SettingKey>, setting: &ModSetting) -> bool {
    keys.iter().any(|(category, key)| *category == setting.category && *key == setting.key)
}

/// A failed read of the sources folder, said with the folder.
fn sources_unreadable(folder: Option<&std::path::Path>, error: &SourceError) -> String {
    match folder {
        Some(folder) => SOURCES_UNREADABLE.with(&[("folder", &folder.display()), ("error", error)]),
        None => error.to_string(),
    }
}

impl View {
    fn bind_callbacks(self: &Rc<Self>, window: &MainWindow) {
        let state = window.global::<ModsState>();
        state.on_stop_work({
            let view = self.clone();
            move || {
                if let Some(cancel) = view.build_cancel.borrow().as_ref() {
                    cancel.cancel();
                }
            }
        });
        state.on_select_mod({
            let view = self.clone();
            move |index| view.select(index)
        });
        state.on_toggle_mod({
            let view = self.clone();
            move |index| view.toggle(index)
        });
        state.on_refresh({
            let view = self.clone();
            move || view.refresh()
        });
        state.on_commit_settings({
            let view = self.clone();
            move || view.commit_settings()
        });
        state.on_setting_edited({
            let view = self.clone();
            move |index, value| {
                view.edit_setting(index, value.into(), None);
                view.show();
            }
        });
        state.on_setting_toggled({
            let view = self.clone();
            move |index, on| {
                view.edit_setting(index, String::new(), Some(on));
                view.commit_settings();
            }
        });
        state.on_setting_choice({
            let view = self.clone();
            move |index, value| {
                view.edit_setting(index, value.into(), None);
                view.commit_settings();
            }
        });
        state.on_remove_mod({
            let view = self.clone();
            move |index| view.remove(index)
        });
        state.on_undo_remove({
            let view = self.clone();
            move || {
                let removed = view.removed.borrow_mut().take();
                if let Some((item, index)) = removed {
                    // Put back and selected again; the removal's message goes with its Undo.
                    view.message.borrow_mut().take();
                    view.enqueue(move |library| {
                        library.restore(&item, index)?;
                        Ok(ChangeResult {
                            select: Some(item.file),
                            ..Default::default()
                        })
                    });
                }
            }
        });
        state.on_revert_drafts({
            let view = self.clone();
            move || {
                view.unsaved.borrow_mut().clear();
                view.editing.borrow_mut().clear();
                // The drafts going is the latest event: it takes the message and the removal's Undo with it.
                if view.message.borrow_mut().take().is_some() {
                    view.removed.borrow_mut().take();
                }
                let inventory = view.inventory.borrow();
                let Some(root) = inventory.mods_folder.as_ref().and_then(|folder| folder.parent()).map(PathBuf::from) else {
                    drop(inventory);
                    view.show();
                    return;
                };
                let visible: Vec<_> = inventory
                    .mods
                    .iter()
                    .flat_map(|item| &item.settings)
                    .map(|setting| (setting.category.clone(), setting.key.clone()))
                    .collect();
                drop(inventory);
                view.enqueue(move |library| {
                    library.revert(&root, &visible)?;
                    Ok(ChangeResult::default())
                });
            }
        });
        state.on_add_mod({
            let view = self.clone();
            move || view.pick_mod()
        });
        state.on_reveal_mod({
            let view = self.clone();
            move |index| view.reveal(index)
        });
        state.on_open_folder({
            let view = self.clone();
            move || {
                let path = view.inventory.borrow().mods_folder.clone();
                if let Some(path) = path {
                    view.open(path, false, true);
                }
            }
        });
        state.on_open_log({
            let view = self.clone();
            move || {
                let path = view.inventory.borrow().loader_log.clone();
                if let Some(path) = path {
                    view.open(path, false, false);
                }
            }
        });
        state.on_play({
            let view = self.clone();
            move || view.play()
        });
        state.on_rebuild({
            let view = self.clone();
            move |index| view.build(index, false, false)
        });
        state.on_install_build({
            let view = self.clone();
            move |index| view.build(index, false, true)
        });
        state.on_build_source({
            let view = self.clone();
            move |index| view.build(index, true, false)
        });
    }
    fn remove(self: &Rc<Self>, index: i32) {
        self.commit_settings();
        let Some((file, name)) = self
            .inventory
            .borrow()
            .mods
            .get(index as usize)
            .map(|item| (item.installed.file.clone(), item.name.clone()))
        else {
            return;
        };
        // The selection stays where the removed row was: on the next mod, or on the one before at the end of the list.
        let next = {
            let inventory = self.inventory.borrow();
            let index = index as usize;
            inventory
                .mods
                .get(index + 1)
                .or_else(|| index.checked_sub(1).and_then(|before| inventory.mods.get(before)))
                .map(|item| item.installed.file.clone())
        };
        *self.preferred_selection.borrow_mut() = next;
        self.enqueue(move |library| {
            Ok(ChangeResult {
                removed: library.remove(&file)?,
                message: Some(REMOVED.with(&[("name", &name)])),
                ..Default::default()
            })
        });
    }
    fn add(self: &Rc<Self>, path: PathBuf) {
        self.commit_settings();
        self.enqueue_tracked(
            move |library| {
                let item = library.add(&path, false)?;
                let name = eg_companion::mods::assembly::MelonAssembly::read(&library.path_of(&item))
                    .ok()
                    .flatten()
                    .map_or_else(|| item.file.clone(), |assembly| assembly.name);
                Ok(ChangeResult {
                    message: Some(ADDED.with(&[("name", &name)])),
                    select: Some(item.file),
                    ..Default::default()
                })
            },
            Vec::new(),
            false,
            true,
            Failure::Raw,
        );
    }
    fn pick_mod(self: &Rc<Self>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(PICKER_TITLE.get())
            .add_filter(PICKER_FILTER.get(), &["dll"])
            .set_parent(&window.window().window_handle());
        let view = self.clone();
        if let Err(error) = slint::spawn_local(async move {
            if let Some(file) = dialog.pick_file().await {
                view.add(file.path().to_owned());
            }
        }) {
            self.say(PICKER_FAILED.with(&[("error", &error)]), true);
            self.show();
        }
    }
    fn reveal(self: &Rc<Self>, index: i32) {
        let Some(item) = self.inventory.borrow().mods.get(index as usize).map(|item| item.installed.clone()) else {
            return;
        };
        let folder = self.inventory.borrow().mods_folder.clone();
        let receive = self.services.worker.enqueue(move |library| Ok(library.locate(&item, folder.as_deref())));
        let view = self.clone();
        background::run(
            move || receive?.recv().map_err(|_| io::Error::other(LIBRARY_STOPPED.get()))?,
            move |result: io::Result<PathBuf>| match result {
                Ok(path) => view.open(path, true, false),
                Err(error) => {
                    view.say(error.to_string(), true);
                    view.show();
                }
            },
        );
    }
    fn open(self: &Rc<Self>, path: PathBuf, reveal: bool, create: bool) {
        let view = self.clone();
        background::run(
            move || eg_companion::shell::open_path(&path, reveal, create),
            move |result| {
                if let Err(error) = result {
                    view.say(error.to_string(), true);
                    view.show();
                }
            },
        );
    }
    fn play(self: &Rc<Self>) {
        if self.launching.get() || self.running.get() {
            return;
        }
        self.commit_settings();
        self.launching.set(true);
        self.say(eg_companion::launch::launcher::STARTING_WITH_MODS.get(), false);
        self.show();
        let launcher = self.services.launcher.clone();
        let view = self.clone();
        // Admission of every draft above precedes the host's preparation on the same library worker.
        background::run(
            move || launcher.play(LaunchMode::OfflineWithMods),
            move |report| {
                view.launching.set(false);
                view.refresh();
                view.say(report.message, !report.started);
                view.show();
            },
        );
    }
    fn build(self: &Rc<Self>, index: i32, source: bool, install_only: bool) {
        if self.building.borrow().is_some() {
            return;
        }
        self.commit_settings();
        // A project not installed yet goes by its own name; a rebuild by the installed mod's, as its row shows it.
        let project = if source {
            self.sources
                .borrow()
                .not_installed
                .get(index as usize)
                .cloned()
                .map(|project| (project.name.clone(), project))
        } else {
            self.inventory.borrow().mods.get(index as usize).and_then(|item| {
                self.sources
                    .borrow()
                    .builds
                    .get(&item.installed.file.to_lowercase())
                    .map(|build| (item.name.clone(), build.project.clone()))
            })
        };
        let Some((name, project)) = project else {
            return;
        };
        let root = self.inventory.borrow().mods_folder.as_ref().and_then(|folder| folder.parent()).map(PathBuf::from);
        if root.is_none() && !install_only {
            self.say(BUILD_NEEDS_GAME.with(&[("name", &name)]), true);
            self.show();
            return;
        }
        let before = if source {
            None
        } else {
            self.inventory.borrow().mods.get(index as usize).map(|item| item.installed.hash.clone())
        };
        let file = project.file();
        *self.building.borrow_mut() = Some(file.clone());
        // An install only reads the mods again; a build says what it builds while the compiler runs.
        if !install_only {
            self.say(BUILDING_NAME.with(&[("name", &name)]), false);
        }
        self.show();
        let cancel = Cancellation::default();
        *self.build_cancel.borrow_mut() = Some(cancel.clone());
        let sdk = self.services.settings.snapshot().store.saved().dotnet_directory.clone();
        let sources = self.services.sources.clone();
        let view = self.clone();
        background::run(
            move || {
                if install_only {
                    Ok(project.target_path)
                } else {
                    sources
                        .build(&project, root.as_deref().expect("build requires game root"), sdk.as_deref(), &cancel)
                        .map_err(io::Error::other)
                }
            },
            move |result| match result {
                Ok(path) => view.enqueue_tracked(
                    move |library| {
                        let item = library.add(&path, !source)?;
                        let assembly = eg_companion::mods::assembly::MelonAssembly::read(&library.path_of(&item)).ok().flatten();
                        let name = assembly.as_ref().map(|assembly| assembly.name.as_str()).unwrap_or(&item.file);
                        let version = assembly.as_ref().map(|assembly| assembly.version.as_str());
                        let message = installed_message(name, version, before.as_ref().is_some_and(|hash| hash.eq_ignore_ascii_case(&item.hash)), item.enabled);
                        Ok(ChangeResult {
                            message: Some(message),
                            select: Some(item.file),
                            ..Default::default()
                        })
                    },
                    Vec::new(),
                    true,
                    true,
                    Failure::Raw,
                ),
                Err(error) => {
                    view.finish_build();
                    if !view.closing.get() {
                        view.say(error.to_string(), true);
                    }
                    view.show();
                }
            },
        );
    }
}

/// A path for a tip, which wraps at words: a zero-width space after each separator lets a long path break between its
/// folders rather than run past the bubble.
fn breakable_path(path: &str) -> String {
    path.replace('\\', "\\\u{200B}").replace('/', "/\u{200B}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_sources_folder_is_named() {
        let error = SourceError::Failed("Access is denied.".into());
        assert_eq!(
            sources_unreadable(Some(std::path::Path::new(r"C:\mods\src")), &error),
            r"The mod sources in C:\mods\src could not be read: Access is denied."
        );
        assert_eq!(sources_unreadable(None, &error), "Access is denied.");
    }

    #[test]
    fn only_edits_of_the_library_say_the_change_could_not_be_kept() {
        let error = io::Error::other("Access is denied.");
        assert_eq!(Failure::Kept.message(&error), "The change could not be kept: Access is denied.");
        assert_eq!(Failure::Raw.message(&error), "Access is denied.");
    }
}
