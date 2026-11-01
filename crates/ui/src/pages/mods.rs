//! Mods in the window: the `eg_companion::mods::feature` feature's impure half. It shows the feature's state in the
//! `ModsState` global, and queues library work, runs the compiler and starts the game when the feature says to.

use crate::shell::{Page, ShellHandle};
use crate::{MainWindow, MessageTone, ModLogRow, ModRow, ModSettingRow, ModsState, SettingChoiceRow, SourceRow};
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::mods::{
    feature::{Answer, Effect, Intent, LogView, ModRowView, Mods, SelectedKey, SelectedView, SettingView, SourceRowView, SourcesKey, State, library_stopped_words, run},
    page::running_state,
    sources::ModSources,
    worker::ModWorker,
};
use eg_core::settings_service::SettingsService;
use flow::{Host, Intents, Slice, Store, Subscriptions, Version};
use flow_slint::{Background, Rows, every};
use phrases::phrases;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use task_runtime::CancellationToken;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

phrases! {
    PickerTitle: "Add a mod"
    PickerFilter: "Mod DLL"
    PickerFailed: "The file picker could not be opened: {error}"
}

/// The services the page runs on, from whoever composes the app.
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

/// The bound page, for the window's close: it goes first, and says when it is done.
#[derive(Clone)]
pub struct ModsHandle {
    store: Store<Mods>,
    closed: OnClosed,
}

impl ModsHandle {
    /// Begins closing the page: finishes the build, keeps the drafts, and waits for the worker.
    pub fn close(&self) {
        self.store.send(Intent::CloseRequested);
    }

    /// `closed` hears when closing has finished: true when the window may go on, false when the page stays to show a failure.
    pub fn on_closed(&self, closed: impl Fn(bool) + 'static) {
        *self.closed.borrow_mut() = Some(Box::new(closed));
    }
}

/// Who hears when closing has finished.
type OnClosed = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

struct Window {
    services: ModsPage,
    background: Background,
    window: Weak<MainWindow>,
    closed: OnClosed,
    build_cancel: Option<CancellationToken>,
    rows: Rows<ModRow>,
    source_rows: Rows<SourceRow>,
    setting_rows: Rows<ModSettingRow>,
    log_rows: Rows<ModLogRow>,
    facts: Rows<SharedString>,
    shown_list: Slice<Version>,
    shown_sources: Slice<SourcesKey>,
    shown_selected: Slice<SelectedKey>,
    watch: Subscriptions<()>,
}

pub(crate) fn bind(window: &MainWindow, page: ModsPage, background: Background, shell: &ShellHandle) -> ModsHandle {
    let closed: OnClosed = Rc::default();
    let host = Window {
        services: page,
        background,
        window: window.as_weak(),
        closed: closed.clone(),
        build_cancel: None,
        rows: Rows::default(),
        source_rows: Rows::default(),
        setting_rows: Rows::default(),
        log_rows: Rows::default(),
        facts: Rows::default(),
        shown_list: Slice::default(),
        shown_sources: Slice::default(),
        shown_selected: Slice::default(),
        watch: Subscriptions::default(),
    };
    let view = window.global::<ModsState>();
    view.set_mods(host.rows.model());
    view.set_sources(host.source_rows.model());
    view.set_settings(host.setting_rows.model());
    view.set_log_lines(host.log_rows.model());
    view.set_readiness_facts(host.facts.model());
    let store = Store::<Mods>::new(State::new(jiff::Timestamp::now()), host);
    let send = |make: fn() -> Intent| {
        let store = store.clone();
        move || store.send(make())
    };
    view.on_stop_work(send(|| Intent::StopWork));
    view.on_refresh(send(|| Intent::Refresh));
    view.on_commit_settings(send(|| Intent::CommitSettings));
    view.on_undo_remove(send(|| Intent::UndoRemove));
    view.on_revert_drafts(send(|| Intent::RevertDrafts));
    view.on_add_mod(send(|| Intent::PickMod));
    view.on_open_folder(send(|| Intent::OpenFolder));
    view.on_open_log(send(|| Intent::OpenLog));
    view.on_play(send(|| Intent::Play));
    let at = |make: fn(i32) -> Intent| {
        let store = store.clone();
        move |index| store.send(make(index))
    };
    view.on_select_mod(at(Intent::Select));
    view.on_toggle_mod(at(Intent::Toggle));
    view.on_remove_mod(at(Intent::Remove));
    view.on_reveal_mod(at(Intent::Reveal));
    view.on_rebuild(at(Intent::Rebuild));
    view.on_install_build(at(Intent::InstallBuild));
    view.on_build_source(at(Intent::BuildSource));
    view.on_setting_edited({
        let store = store.clone();
        move |index, value| store.send(Intent::SettingEdited(index, value.into()))
    });
    view.on_setting_toggled({
        let store = store.clone();
        move |index, on| store.send(Intent::SettingToggled(index, on))
    });
    view.on_setting_choice({
        let store = store.clone();
        move |index, value| store.send(Intent::SettingChoice(index, value.into()))
    });
    if shell.current() == Page::Mods {
        store.send(Intent::PageChanged {
            shown: true,
            now: jiff::Timestamp::now(),
        });
    }
    shell.on_page_entered({
        let store = store.clone();
        move |page| {
            store.send(Intent::PageChanged {
                shown: page == Page::Mods,
                now: jiff::Timestamp::now(),
            });
        }
    });
    ModsHandle { store, closed }
}

fn mod_row(row: ModRowView) -> ModRow {
    ModRow {
        name: row.name.into(),
        version: row.version.into(),
        author: row.author.into(),
        enabled: row.enabled,
        drafted: row.drafted,
        problem: row.problem.into(),
        problem_label: row.problem_label.into(),
    }
}

fn source_row(row: SourceRowView) -> SourceRow {
    SourceRow {
        name: row.name.into(),
        project: row.project.into(),
        version: row.version.into(),
        folder: row.folder.into(),
        busy: row.busy,
    }
}

fn log_row(row: LogView) -> ModLogRow {
    ModLogRow {
        time: row.time.into(),
        text: row.text.into(),
        error: row.error,
        count: row.count as i32,
        count_tip: row.count_tip.into(),
    }
}

/// `old` is the row now in this place: the same choices keep their models, so an open choice list isn't rebuilt under the
/// pointer.
fn setting_row(row: SettingView, old: Option<ModSettingRow>) -> ModSettingRow {
    let choices: Vec<SettingChoiceRow> = row
        .choices
        .into_iter()
        .map(|(value, label)| SettingChoiceRow {
            value: value.into(),
            label: label.into(),
        })
        .collect();
    let (choices, choice_labels) = old
        .filter(|old| old.category == row.category.as_str() && old.key == row.key.as_str() && old.choices.iter().eq(choices.iter().cloned()))
        .map(|old| (old.choices, old.choice_labels))
        .unwrap_or_else(|| {
            let labels = choices.iter().map(|choice| choice.label.clone()).collect::<Vec<_>>();
            (ModelRc::new(VecModel::from(choices)), ModelRc::new(VecModel::from(labels)))
        });
    ModSettingRow {
        category: row.category.into(),
        key: row.key.into(),
        name: row.name.into(),
        description: row.description.into(),
        group: row.group.into(),
        kind: row.kind.into(),
        value: row.value.into(),
        is_on: row.is_on,
        unit: row.unit.into(),
        drafted: row.drafted,
        was: row.was.into(),
        error: row.error.into(),
        choice_label: row.choice_label.into(),
        choice_index: row.choice_index,
        choices,
        choice_labels,
    }
}

fn show_selected(view: &ModsState<'_>, selected: &SelectedView) {
    view.set_selected_name(selected.name.as_str().into());
    view.set_selected_detail(selected.detail.as_str().into());
    view.set_selected_file(selected.file.as_str().into());
    view.set_selected_description(selected.description.as_str().into());
    view.set_selected_problem(selected.problem.as_str().into());
    view.set_selected_state(selected.state.into());
    view.set_selected_enabled(selected.enabled);
    view.set_previous_build(selected.previous_build);
    view.set_rebuild_text(selected.rebuild_text.into());
    view.set_rebuild_tip(selected.rebuild_tip.into());
    view.set_install_build_text(selected.install_build_text.as_str().into());
    view.set_selected_source(selected.source.as_str().into());
    view.set_selected_source_path(selected.source_path.as_str().into());
    view.set_selected_has_sources(selected.has_sources);
    view.set_can_install_build(selected.can_install_build);
    view.set_selected_build_status(selected.build_status.as_str().into());
}

impl Host<Mods> for Window {
    fn render(&mut self, state: &State, intents: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let view = window.global::<ModsState>();
        let status = state.status();
        view.set_readiness(status.readiness.into());
        self.facts.sync(status.facts.into_iter().map(SharedString::from));
        view.set_readiness_tone(if status.warning { MessageTone::Warning } else { MessageTone::Info });
        view.set_can_play(status.can_play);
        view.set_is_read(status.is_read);
        view.set_can_open_folder(status.can_open_folder);
        view.set_can_open_log(status.can_open_log);
        view.set_open_folder_tip(status.open_folder_tip.into());
        view.set_open_log_tip(status.open_log_tip.into());
        view.set_installed_count(status.installed as i32);
        view.set_enabled_count(status.enabled as i32);
        view.set_selected_index(status.selected_index);
        view.set_busy(status.busy);
        view.set_building(status.building);
        view.set_draft_count(status.draft_count as i32);
        view.set_can_undo(status.can_undo);
        view.set_change_is_draft(status.change_is_draft);
        view.set_change_message(status.change_message.into());
        view.set_change_tone(if status.change_is_error { MessageTone::Error } else { MessageTone::Info });

        let (rows, source_rows, setting_rows, log_rows) = (&self.rows, &self.source_rows, &self.setting_rows, &self.log_rows);
        self.shown_list.update(state.list_version(), || rows.sync(state.mod_rows().into_iter().map(mod_row)));
        self.shown_sources
            .update(state.sources_key(), || source_rows.sync(state.source_rows().into_iter().map(source_row)));
        self.shown_selected.update(state.selected_key(), || {
            let selected = state.selected_view();
            show_selected(&view, &selected);
            let current = setting_rows.model();
            setting_rows.sync(
                selected
                    .settings
                    .into_iter()
                    .enumerate()
                    .map(|(index, setting)| setting_row(setting, current.row_data(index)))
                    .collect::<Vec<_>>(),
            );
            log_rows.sync(selected.log.into_iter().map(log_row));
        });
        self.watch.sync(state.watching().then_some(()), |()| {
            every(GameLauncher::WATCH_INTERVAL, intents, || Intent::Watch(jiff::Timestamp::now()))
        });
    }

    fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::Enqueue { revision, change, undo } => {
                let launcher = self.services.launcher.clone();
                let settings = self.services.settings.clone();
                match self.services.worker.enqueue(move |library| run(change, undo, library, &launcher, &settings)) {
                    Ok(receive) => self.background.run(
                        intents,
                        move || receive.recv(),
                        move |result| Intent::Answered {
                            revision,
                            answer: match result {
                                Ok(Ok(result)) => Answer::of(Ok(result)),
                                Ok(Err(_)) | Err(_) => Answer::library_stopped(),
                            },
                        },
                    ),
                    Err(error) => intents.send(Intent::Answered {
                        revision,
                        answer: Answer::stopped(&error),
                    }),
                }
            }
            Effect::ReadSources { revision, folder, sdk, files } => {
                let sources = self.services.sources.clone();
                let cancel = self.services.worker.cancellation();
                self.background.run(
                    intents,
                    move || {
                        let result = sources.read(folder.as_deref(), sdk.as_deref(), &files, &cancel);
                        (result, sources.sdk_missing())
                    },
                    move |result| Intent::SourcesRead { revision, result },
                );
            }
            Effect::WatchGame { generation, log } => {
                let launcher = self.services.launcher.clone();
                self.background.run(
                    intents,
                    move || {
                        let (running, modded) = running_state(&launcher, log.as_deref());
                        (running, modded, launcher.is_starting())
                    },
                    move |result| Intent::Watched { generation, result },
                );
            }
            Effect::Locate { item, folder } => match self.services.worker.enqueue(move |library| Ok(library.locate(&item, folder.as_deref()))) {
                Ok(receive) => self.background.run(
                    intents,
                    move || receive.recv(),
                    |result| {
                        Intent::Located(match result {
                            Ok(Ok(Ok(path))) => Ok(path),
                            Ok(Ok(Err(error))) => Err(error.to_string()),
                            Ok(Err(_)) | Err(_) => Err(library_stopped_words().to_owned()),
                        })
                    },
                ),
                Err(error) => intents.send(Intent::Located(Err(error.to_string()))),
            },
            Effect::Open { path, reveal, create } => self.open(path, reveal, create, intents),
            Effect::PickMod => self.pick_mod(intents),
            Effect::Launch(mode) => {
                let launcher = self.services.launcher.clone();
                self.background.run(intents, move || launcher.play(mode), Intent::Launched);
            }
            Effect::Build { project, root, sdk, install_only } => {
                let cancel = CancellationToken::default();
                self.build_cancel = Some(cancel.clone());
                let sources = self.services.sources.clone();
                self.background.run(
                    intents,
                    move || {
                        if install_only {
                            return Ok(project.target_path);
                        }
                        let root = root.expect("the feature asks for a build only with the game's root");
                        sources.build(&project, &root, sdk.as_deref(), &cancel).map_err(|error| error.to_string())
                    },
                    Intent::Built,
                );
            }
            Effect::CancelBuild => {
                if let Some(cancel) = &self.build_cancel {
                    cancel.cancel();
                }
            }
            Effect::ShowPage => {
                if let Some(window) = self.window.upgrade() {
                    window.global::<crate::Nav>().invoke_navigate("mods".into());
                }
            }
            Effect::Closed => self.tell_closed(true),
            Effect::CloseAbandoned => self.tell_closed(false),
        }
    }
}

impl Window {
    fn tell_closed(&self, closed: bool) {
        if let Some(tell) = &*self.closed.borrow() {
            tell(closed);
        }
    }

    fn open(&self, path: PathBuf, reveal: bool, create: bool, intents: &Intents<Intent>) {
        self.background.run(
            intents,
            move || eg_companion::shell::open_path(&path, reveal, create).map_err(|error| error.to_string()),
            Intent::Opened,
        );
    }

    fn pick_mod(&self, intents: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(PickerTitle.get())
            .add_filter(PickerFilter.get(), &["dll"])
            .set_parent(&window.window().window_handle());
        let picking = intents.clone();
        if let Err(error) = slint::spawn_local(async move {
            if let Some(file) = dialog.pick_file().await {
                picking.send(Intent::Add(file.path().to_owned()));
            }
        }) {
            intents.send(Intent::Failed(PickerFailed.with(&[("error", &error)])));
        }
    }
}
