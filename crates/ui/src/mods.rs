//! Mods keeps interactive drafts in memory. All library reads and writes enter one ordered worker;
//! completions are observed on the UI thread without waiting for disk or MSBuild.

use crate::shell::{Page, ShellHandle};
use crate::{MainWindow, MessageTone, ModLogRow, ModRow, ModSettingRow, ModsState, SettingChoiceRow, SourceRow, background};
use eg_companion::host::LaunchMode;
use eg_companion::launcher::GameLauncher;
use eg_companion::mods::{
    inventory::{self, Inventory},
    library::{InstalledMod, ModLibrary},
    loader_state::LoaderPreparation,
    preferences::PreferenceKind,
    sources::{ModSources, SourceSnapshot},
    worker::ModWorker,
};
use eg_core::operations::Cancellation;
use eg_core::settings_service::SettingsService;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, VecDeque},
    io,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

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
    adopted: Cell<usize>,
    deleted: Cell<usize>,
    close_error: Cell<bool>,
    closing: Cell<bool>,
    watch: RefCell<Instant>,
    timer: slint::Timer,
}

pub(crate) fn bind(window: &MainWindow, page: ModsPage, shell: &ShellHandle) -> Rc<dyn Fn() -> crate::settings::CloseDecision> {
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
        adopted: Cell::new(0),
        deleted: Cell::new(0),
        watch: RefCell::new(Instant::now()),
        timer: slint::Timer::default(),
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
            if page == Page::Mods {
                view.refresh();
            }
        }
    });
    view.timer.start(slint::TimerMode::Repeated, Duration::from_millis(40), {
        let view = Rc::downgrade(&view);
        move || {
            if let Some(view) = view.upgrade() {
                view.poll();
                if view.services.worker.faulted() {
                    let _ = slint::quit_event_loop();
                }
                if view.visible.get() && view.watch.borrow().elapsed() >= GameLauncher::WATCH_INTERVAL {
                    *view.watch.borrow_mut() = Instant::now();
                    view.watch_game();
                }
            }
        }
    });
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
        state.set_readiness(if self.read.get() { readiness.text.into() } else { "Reading mods…".into() });
        state.set_readiness_tone(if readiness.warning { MessageTone::Warning } else { MessageTone::Info });
        state.set_can_play(self.read.get() && readiness.can_play && !self.closing.get());
        state.set_is_read(self.read.get());
        state.set_has_read_error(inventory.error.is_some());
        state.set_can_open_folder(inventory.mods_folder.is_some());
        state.set_can_open_log(inventory.loader_log.is_some());
        let folder_tip = if inventory.mods_folder.is_some() {
            "Open the Mods folder"
        } else if self.read.get() {
            "Not available: Last Epoch was not found, so there is no Mods folder. Choose the game executable in Settings."
        } else {
            "Available once the mods are read."
        };
        state.set_open_folder_tip(folder_tip.into());
        state.set_open_log_tip(
            if inventory.loader_log.is_some() {
                "Open MelonLoader's log from the last game with mods"
            } else if inventory.mods_folder.is_none() {
                folder_tip
            } else {
                "Not available: MelonLoader writes its log the first time you play with mods."
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
        let text = message.as_ref().map(|(message, _)| message.as_str()).unwrap_or(if draft_count > 0 {
            "Changes since the last game with mods take effect the next time you play with mods."
        } else {
            ""
        });
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
                .map(|item| {
                    let version = item.version.as_deref().unwrap_or("No version");
                    let caption = item.author.as_ref().map_or_else(|| version.to_owned(), |author| format!("{version} · {author}"));
                    ModRow {
                        file: item.installed.file.as_str().into(),
                        name: item.name.as_str().into(),
                        version: version.into(),
                        author: item.author.as_deref().unwrap_or_default().into(),
                        caption: caption.into(),
                        enabled: item.installed.enabled,
                        drafted: item.draft_count() > 0,
                        problem: if item.installed.enabled { item.problem.as_deref().unwrap_or_default() } else { "" }.into(),
                        problem_label: if item.installed.enabled && item.problem.is_some() {
                            if item.loaded_last_time == Some(false) { "Did not load" } else { "Can't load" }
                        } else {
                            ""
                        }
                        .into(),
                    }
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
                    caption: folder.as_deref().map(|folder| project.caption(folder)).unwrap_or_default().into(),
                    busy: building.as_ref().is_some_and(|file| file.eq_ignore_ascii_case(&project.file())),
                })
                .collect(),
        );
        let selected = inventory.mods.get(self.selection.get());
        state.set_selected_name(selected.map(|item| item.name.as_str()).unwrap_or_default().into());
        state.set_selected_version(selected.and_then(|item| item.version.as_deref()).unwrap_or("No version").into());
        state.set_selected_author(selected.and_then(|item| item.author.as_deref()).unwrap_or_default().into());
        state.set_selected_file(selected.map(|item| item.installed.file.as_str()).unwrap_or_default().into());
        state.set_selected_description(selected.and_then(|item| item.description.as_deref()).unwrap_or_default().into());
        state.set_selected_problem(
            selected
                .and_then(|item| {
                    item.problem.as_ref().map(|problem| {
                        if item.loaded_last_time == Some(false) {
                            format!("Did not load in the last game with mods. {problem}")
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
        state.set_rebuild_text(
            if selected.is_some_and(|item| building.as_ref().is_some_and(|file| file.eq_ignore_ascii_case(&item.installed.file))) {
                "Building…"
            } else if build
                .zip(selected)
                .is_some_and(|(build, item)| build.project.update_available(item.project_version.as_deref()))
            {
                "Update"
            } else {
                "Rebuild"
            }
            .into(),
        );
        state.set_install_build_text(
            build
                .zip(selected)
                .and_then(|(build, item)| build.version.as_ref().filter(|version| Some(version.as_str()) != item.version.as_deref()))
                .map(|version| format!("Install {version}"))
                .unwrap_or_else(|| "Install build".into())
                .into(),
        );
        state.set_selected_source(
            build
                .zip(folder.as_deref())
                .map(|(build, folder)| format!("From sources in {}", build.project.source_folder(folder)))
                .unwrap_or_default()
                .into(),
        );
        state.set_selected_source_path(build.map(|build| build.project.project_file.display().to_string()).unwrap_or_default().into());
        state.set_can_rebuild(build.is_some() && building.is_none());
        state.set_can_install_build(
            build
                .zip(selected)
                .is_some_and(|(build, item)| build.can_install(&item.installed.hash, item.project_version.as_deref()))
                && building.is_none(),
        );
        state.set_selected_build_status(
            build
                .zip(selected)
                .map(|(build, item)| {
                    if build.project.update_available(item.project_version.as_deref()) {
                        "Update available."
                    } else if !build.is_built() {
                        "Not built yet."
                    } else if !build.is_installed(&item.installed.hash) {
                        if build.changed_since_build {
                            "Its latest build is not installed, and its sources changed since that build."
                        } else {
                            "Its latest build is not installed."
                        }
                    } else if build.changed_since_build {
                        "Its sources changed since this build."
                    } else {
                        ""
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
                        let next = setting.group.as_deref().or(group.map(|_| "Other settings"));
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
                        let choices = self
                            .setting_rows
                            .row_data(index)
                            .filter(|old| old.category == setting.category && old.key == setting.key && old.choices.iter().eq(choices.iter().cloned()))
                            .map(|old| old.choices)
                            .unwrap_or_else(|| ModelRc::new(VecModel::from(choices)));
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
                            choice_index: setting.choices.iter().position(|choice| choice.value == setting.value).unwrap_or(0) as i32,
                            choices,
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
                                count_tip: format!(
                                    "Written {} times in a row, the last at {:02}:{:02}:{:02}",
                                    run.count,
                                    last / 3600,
                                    last / 60 % 60,
                                    last % 60
                                )
                                .into(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
}

fn update_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
    } else {
        for (index, row) in rows.into_iter().enumerate() {
            if model.row_data(index).as_ref() != Some(&row) {
                model.set_row_data(index, row);
            }
        }
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
        self.enqueue_tracked(change, Vec::new(), false, true);
    }
    fn enqueue_tracked(
        self: &Rc<Self>,
        change: impl FnOnce(&ModLibrary) -> io::Result<ChangeResult> + Send + 'static,
        settings: Vec<SettingKey>,
        clears_build: bool,
        mutation: bool,
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
            Ok(receive) => self.pending.borrow_mut().push_back(Pending {
                revision,
                receive,
                settings,
                clears_build,
            }),
            Err(error) => {
                self.retain_failed_settings(settings);
                if clears_build {
                    self.finish_build();
                }
                self.say(format!("The change could not be kept: {error}"), true);
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
        self.enqueue_tracked(|_| Ok(ChangeResult::default()), Vec::new(), false, false);
    }
    fn poll(self: &Rc<Self>) {
        let mut refresh = false;
        loop {
            let next = {
                let mut pending = self.pending.borrow_mut();
                match pending.front().map(|job| job.receive.try_recv()) {
                    Some(Ok(result)) => Some((pending.pop_front().unwrap(), result)),
                    Some(Err(mpsc::TryRecvError::Disconnected)) => Some((pending.pop_front().unwrap(), Err(io::Error::other("The mod library stopped.")))),
                    _ => None,
                }
            };
            let Some((pending, result)) = next else {
                break;
            };
            let revision = pending.revision;
            if pending.clears_build {
                self.finish_build();
            }
            match result {
                Err(error) => {
                    self.retain_failed_settings(pending.settings);
                    self.say(format!("The change could not be kept: {error}"), true);
                    refresh = true;
                }
                Ok(observation) => {
                    self.adopted.set(self.adopted.get() + observation.inventory.changes.adopted.len());
                    self.deleted.set(self.deleted.get() + observation.inventory.changes.removed.len());
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
                            if self.editing.borrow().contains(&(setting.category.clone(), setting.key.clone()))
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
                    if let Some(problem) = &inventory.settings_problem {
                        self.say(problem, true);
                    }
                    let adopted = self.adopted.replace(0);
                    let deleted = self.deleted.replace(0);
                    if adopted > 0 || deleted > 0 {
                        self.say(format!("{adopted} added from the Mods folder; {deleted} removed after their DLLs were deleted."), false);
                    }
                    *self.inventory.borrow_mut() = inventory;
                    *self.version.borrow_mut() = observation.version;
                    self.read.set(true);
                    self.read_sources(revision);
                }
            }
        }
        self.show();
        if refresh && !self.services.worker.faulted() {
            self.enqueue_tracked(|_| Ok(ChangeResult::default()), Vec::new(), false, false);
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
            .filter(|setting| unsaved.contains(&(setting.category.clone(), setting.key.clone())))
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
        );
    }
    fn close_decision(self: &Rc<Self>) -> crate::settings::CloseDecision {
        use crate::settings::CloseDecision;
        if !self.closing.replace(true) {
            if let Some(cancel) = self.build_cancel.borrow().as_ref() {
                cancel.cancel();
            }
            self.commit_settings();
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
                            view.say("Building mods from their sources needs the .NET SDK, which was not found. Install it, or choose its folder in Settings, to see and build the mod projects here.", true);
                        }
                    }
                    Err(error) => {
                        *view.sources.borrow_mut() = SourceSnapshot::default();
                        view.say(error.to_string(), true);
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
                let typing = inventory.mods.iter().flat_map(|item| &item.settings).any(|setting| {
                    setting.kind != PreferenceKind::Switch && !setting.is_choice() && view.unsaved.borrow().contains(&(setting.category.clone(), setting.key.clone()))
                });
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

fn running_state(launcher: &GameLauncher, log: Option<&std::path::Path>) -> (bool, bool) {
    let running = launcher.find_running();
    let modded = running.as_ref().zip(log).is_some_and(|(running, log)| {
        std::fs::metadata(log)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| jiff::Timestamp::try_from(time).ok())
            .is_some_and(|time| time.as_second() >= running.start_time.as_second() - 10)
    });
    (running.is_some(), modded)
}

/// Follow preparation through completion and five more observations, without committing a box under its editor.
fn follow_preparation(was_running: bool, running: bool, modded: bool, preparing: bool, typing: bool, follow: u8) -> (bool, u8) {
    if was_running && !running {
        return (true, 0);
    }
    if running && modded && !typing && (preparing || follow > 0) {
        (true, if preparing { 5 } else { follow - 1 })
    } else {
        (false, follow)
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
                    view.enqueue(move |library| {
                        library.restore(&item, index)?;
                        Ok(ChangeResult {
                            message: Some(format!("Put {} back.", item.file)),
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
                let inventory = view.inventory.borrow();
                let Some(root) = inventory.mods_folder.as_ref().and_then(|folder| folder.parent()).map(PathBuf::from) else {
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
        self.enqueue(move |library| {
            Ok(ChangeResult {
                removed: library.remove(&file)?,
                message: Some(format!(
                    "{name} is removed. Its DLL leaves the Mods folder the next time you play with mods; its settings stay in MelonPreferences.cfg."
                )),
                ..Default::default()
            })
        });
    }
    fn add(self: &Rc<Self>, path: PathBuf) {
        self.commit_settings();
        self.enqueue(move |library| {
            let item = library.add(&path, false)?;
            Ok(ChangeResult {
                message: Some(format!("Added {}. It loads the next time you play with mods.", item.file)),
                select: Some(item.file),
                ..Default::default()
            })
        });
    }
    fn pick_mod(self: &Rc<Self>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Add a MelonLoader mod")
            .add_filter("Mod DLL", &["dll"])
            .set_parent(&window.window().window_handle());
        let view = self.clone();
        if let Err(error) = slint::spawn_local(async move {
            if let Some(file) = dialog.pick_file().await {
                view.add(file.path().to_owned());
            }
        }) {
            self.say(format!("The mod picker could not be opened: {error}"), true);
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
            move || receive?.recv().map_err(|_| io::Error::other("The mod library stopped."))?,
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
            move || open_path(&path, reveal, create),
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
        self.say(eg_companion::launcher::STARTING_WITH_MODS, false);
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
        let project = if source {
            self.sources.borrow().not_installed.get(index as usize).cloned()
        } else {
            self.inventory
                .borrow()
                .mods
                .get(index as usize)
                .and_then(|item| self.sources.borrow().builds.get(&item.installed.file.to_lowercase()).map(|build| build.project.clone()))
        };
        let Some(project) = project else {
            return;
        };
        let root = self.inventory.borrow().mods_folder.as_ref().and_then(|folder| folder.parent()).map(PathBuf::from);
        if root.is_none() && !install_only {
            self.say(
                format!(
                    "{} is built against the game's MelonLoader, and Last Epoch was not found. Choose the game executable in Settings.",
                    project.name
                ),
                true,
            );
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
        self.say(format!("{} {}…", if install_only { "Installing" } else { "Building" }, project.name), false);
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

fn installed_message(name: &str, version: Option<&str>, unchanged: bool, enabled: bool) -> String {
    let version = version.map(|version| format!(" {version}")).unwrap_or_default();
    if unchanged {
        format!("{name}{version} is installed already: the build is the same.")
    } else if enabled {
        format!("Installed {name}{version}. It loads the next time you play with mods.")
    } else {
        format!("Installed {name}{version}. It is off; turn it on to load it in the next game with mods.")
    }
}

fn keep_each<T>(changes: impl IntoIterator<Item = T>, mut keep: impl FnMut(T) -> io::Result<()>) -> io::Result<()> {
    let mut failure = None;
    for change in changes {
        if let Err(error) = keep(change) {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(windows)]
fn open_path(path: &std::path::Path, reveal: bool, create: bool) -> io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, process::CommandExt};
    if create {
        std::fs::create_dir_all(path)?;
    }
    if reveal {
        std::process::Command::new("explorer.exe").arg("/select,").arg(path).creation_flags(0x0800_0000).spawn()?;
        return Ok(());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let open: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // SAFETY: all UTF-16 strings are terminated and live until the call returns; ShellExecute opens the user's selected file.
    let result = unsafe { windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
    if result as isize <= 32 {
        Err(io::Error::other(format!("Could not open {} (Windows error {}).", path.display(), result as isize)))
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn open_path(_path: &std::path::Path, _reveal: bool, _create: bool) -> io::Result<()> {
    Err(io::Error::other("Opening files is available on Windows."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_follows_completion_then_stops_and_waits_for_text_commit() {
        let (refresh, mut remaining) = follow_preparation(false, true, true, true, false, 0);
        assert!(refresh);
        assert_eq!(remaining, 5);
        // An active box pauses both reading and the follow budget.
        assert_eq!(follow_preparation(true, true, true, false, true, remaining), (false, 5));
        for expected in (0..5).rev() {
            let (refresh, next) = follow_preparation(true, true, true, false, false, remaining);
            assert!(refresh);
            assert_eq!(next, expected);
            remaining = next;
        }
        assert_eq!(follow_preparation(true, true, true, false, false, remaining), (false, 0));
        assert_eq!(follow_preparation(true, false, false, false, true, 3), (true, 0));
        assert_eq!(follow_preparation(false, true, false, true, false, 0), (false, 0));
    }

    #[test]
    fn failed_setting_does_not_discard_other_settings_and_reports_first_failure() {
        let mut attempted = Vec::new();
        let result = keep_each(["first", "second", "third"], |key| {
            attempted.push(key);
            if key == "first" {
                Err(io::Error::other("first failed"))
            } else if key == "second" {
                Err(io::Error::other("second failed"))
            } else {
                Ok(())
            }
        });
        assert_eq!(attempted, ["first", "second", "third"]);
        assert_eq!(result.unwrap_err().to_string(), "first failed");
    }
}
