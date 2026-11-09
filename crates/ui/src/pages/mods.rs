//! Mods in the window: the `eg_companion::mods::feature` feature's impure half. It shows the feature's state in the
//! `ModsState` global, and queues library work, runs the compiler and starts the game when the feature says to.

use crate::shell::{Intent as ShellIntent, Page, ShellHandle};
use crate::{MainWindow, MessageTone, ModLogRow, ModRow, ModSettingRow, ModsState, SelectedModView, SettingChoiceRow, SourceRow};
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::mods::{
    feature::{
        Answer, Effect, Intent, LogView, ModRowView, Mods, Output, SelectedKey, SelectedView, SettingView, SourceRowView, SourcesKey, State, Subscription, library_stopped_words,
        run,
    },
    page::running_state,
    sources::ModSources,
    worker::ModWorker,
};
use eg_core::settings_service::SettingsService;
use phrases::phrases;
use slint::{ComponentHandle, ModelRc, SharedString};
use spiderland::{CheckedSlice, Intents, Store, Version, Wiring, callbacks};
use spiderland_slint::{Background, InnerRows, Rows, WindowHost, Windowed, every};
use std::any::Any;
use task_runtime::CancellationToken;

use std::path::PathBuf;
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

struct Window {
    services: ModsPage,
    background: Background,
    build_cancel: Option<CancellationToken>,
    rows: Rows<ModRow>,
    source_rows: Rows<SourceRow>,
    setting_rows: Rows<ModSettingRow>,
    /// Each setting's choices, and their labels for the view's menu, kept in models of their own so an open choice list isn't
    /// rebuilt under the pointer.
    setting_choices: InnerRows<SettingChoiceRow>,
    setting_labels: InnerRows<SharedString>,
    log_rows: Rows<ModLogRow>,
    facts: Rows<SharedString>,
    /// A test build makes each region's rows anyway, and fails if its key misses an input.
    shown_list: CheckedSlice<Version, Vec<ModRowView>>,
    shown_sources: CheckedSlice<SourcesKey, Vec<SourceRowView>>,
    shown_selected: CheckedSlice<SelectedKey, SelectedView>,
}

/// Builds the page's store, not started, so the window's close can be wired to it first: it goes first in the close, where it
/// finishes the build, keeps the drafts and waits for the worker, and answers with [`Output::Closed`]. [`bind`] takes it started.
pub(crate) fn build(window: &MainWindow, page: ModsPage, background: Background, shell: &ShellHandle) -> Wiring<Mods> {
    let host = Window {
        services: page,
        background,
        build_cancel: None,
        rows: Rows::default(),
        source_rows: Rows::default(),
        setting_rows: Rows::default(),
        setting_choices: InnerRows::default(),
        setting_labels: InnerRows::default(),
        log_rows: Rows::default(),
        facts: Rows::default(),
        shown_list: CheckedSlice::default(),
        shown_sources: CheckedSlice::default(),
        shown_selected: CheckedSlice::default(),
    };
    let view = window.global::<ModsState>();
    view.set_mods(host.rows.model());
    view.set_sources(host.source_rows.model());
    view.set_settings(host.setting_rows.model());
    view.set_log_lines(host.log_rows.model());
    view.set_readiness_facts(host.facts.model());
    let wiring = Store::<Mods>::build(State::new(jiff::Timestamp::now(), shell.current() == Page::Mods), Windowed::new(window, host));
    crate::trail::follow(&wiring, "mods");
    shell.forward_shown(Page::Mods, &wiring.intents(), |shown| Intent::PageChanged {
        shown,
        now: jiff::Timestamp::now(),
    });
    wiring.forward(&shell.intents(), |output| (*output == Output::ShowPage).then_some(ShellIntent::Navigate(Page::Mods)));
    wiring
}

/// Connects the view's callbacks to the started store, which they keep alive for as long as the window lasts.
pub(crate) fn bind(window: &MainWindow, store: Store<Mods>) -> Store<Mods> {
    let view = window.global::<ModsState>();
    callbacks!(view, store, {
        on_refresh: || Intent::Refresh,
        on_commit_settings: || Intent::CommitSettings,
        on_undo_remove: || Intent::UndoRemove,
        on_revert_drafts: || Intent::RevertDrafts,
        on_add_mod: || Intent::PickMod,
        on_open_folder: || Intent::OpenFolder,
        on_open_log: || Intent::OpenLog,
        on_play: || Intent::Play,
        on_select_mod: |index| Intent::Select(index),
        on_toggle_mod: |index| Intent::Toggle(index),
        on_remove_mod: |index| Intent::Remove(index),
        on_reveal_mod: |index| Intent::Reveal(index),
        on_rebuild: |index| Intent::Rebuild(index),
        on_install_build: |index| Intent::InstallBuild(index),
        on_build_source: |index| Intent::BuildSource(index),
        on_setting_edited: |index, value| Intent::SettingEdited(index, value.into()),
        on_setting_toggled: |index, on| Intent::SettingToggled(index, on),
        on_setting_choice: |index, value| Intent::SettingChoice(index, value.into()),
    });
    store
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

/// A setting's row, with its choices apart for the models the view keeps.
fn setting_row(row: SettingView) -> (ModSettingRow, Vec<SettingChoiceRow>) {
    let choices = row
        .choices
        .into_iter()
        .map(|(value, label)| SettingChoiceRow {
            value: value.into(),
            label: label.into(),
        })
        .collect();
    let row = ModSettingRow {
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
        choices: ModelRc::default(),
        choice_labels: ModelRc::default(),
    };
    (row, choices)
}

fn show_selected(view: &ModsState<'_>, selected: &SelectedView) {
    view.set_selected(SelectedModView {
        name: selected.name.as_str().into(),
        detail: selected.detail.as_str().into(),
        file: selected.file.as_str().into(),
        source: selected.source.as_str().into(),
        source_path: selected.source_path.as_str().into(),
        build_status: selected.build_status.as_str().into(),
        description: selected.description.as_str().into(),
        problem: selected.problem.as_str().into(),
        state: selected.state.into(),
        enabled: selected.enabled,
        has_sources: selected.has_sources,
        can_install_build: selected.can_install_build,
        rebuild_text: selected.rebuild_text.into(),
        rebuild_tip: selected.rebuild_tip.into(),
        install_build_text: selected.install_build_text.as_str().into(),
        previous_build: selected.previous_build,
    });
}

impl WindowHost<Mods> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
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

        let (rows, source_rows) = (&self.rows, &self.source_rows);
        self.shown_list
            .update("mods", state.list_version(), || state.mod_rows(), |mods| rows.sync(mods.into_iter().map(mod_row)));
        self.shown_sources.update(
            "sources",
            state.sources_key(),
            || state.source_rows(),
            |sources| source_rows.sync(sources.into_iter().map(source_row)),
        );
        let (setting_rows, choices, labels, log_rows) = (&self.setting_rows, &mut self.setting_choices, &mut self.setting_labels, &self.log_rows);
        self.shown_selected.update(
            "selected",
            state.selected_key(),
            || state.selected_view(),
            |selected| {
                show_selected(&view, &selected);
                let rows: Vec<_> = selected.settings.into_iter().map(setting_row).collect();
                let label_lists: Vec<Vec<SharedString>> = rows.iter().map(|(_, choices)| choices.iter().map(|choice| choice.label.clone()).collect()).collect();
                let rows = choices.fill(rows, |row, choices| row.choices = choices);
                setting_rows.sync(labels.fill(rows.into_iter().zip(label_lists), |row, labels| row.choice_labels = labels));
                log_rows.sync(selected.log.into_iter().map(log_row));
            },
        );
    }

    fn subscribe(&mut self, Subscription::Tick: &Subscription, intents: &Intents<Intent>) -> Box<dyn Any> {
        Box::new(every(GameLauncher::WATCH_INTERVAL, intents, || Intent::Watch(jiff::Timestamp::now())))
    }

    fn perform(&mut self, window: Option<&MainWindow>, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::Enqueue { revision, change, undo } => {
                let launcher = self.services.launcher.clone();
                let settings = self.services.settings.clone();
                match self.services.worker.enqueue(move |library| run(change, undo, library, &launcher, &settings)) {
                    Ok(receive) => self.background.wait(intents, receive, move |result| Intent::Answered {
                        revision,
                        answer: match result {
                            Ok(result) => Answer::of(Ok(result)),
                            Err(_) => Answer::library_stopped(),
                        },
                    }),
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
            Effect::WatchGame { asked, log } => {
                let launcher = self.services.launcher.clone();
                self.background.run(
                    intents,
                    move || {
                        let (running, modded) = running_state(&launcher, log.as_deref());
                        (running, modded, launcher.is_starting())
                    },
                    move |result| Intent::Watched { asked, result },
                );
            }
            Effect::Locate { item, folder } => match self.services.worker.enqueue(move |library| Ok(library.locate(&item, folder.as_deref()))) {
                Ok(receive) => self.background.wait(intents, receive, |result| {
                    Intent::Located(match result {
                        Ok(Ok(path)) => Ok(path),
                        Ok(Err(error)) => Err(error.to_string()),
                        Err(_) => Err(library_stopped_words().to_owned()),
                    })
                }),
                Err(error) => intents.send(Intent::Located(Err(error.to_string()))),
            },
            Effect::Open { path, reveal, create } => self.open(path, reveal, create, intents),
            Effect::PickMod => self.pick_mod(window, intents),
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
        }
    }
}

impl Window {
    fn open(&self, path: PathBuf, reveal: bool, create: bool, intents: &Intents<Intent>) {
        self.background.run(
            intents,
            move || eg_companion::shell::open_path(&path, reveal, create).map_err(|error| error.to_string()),
            Intent::Opened,
        );
    }

    /// Opens a picker, parented to the window; with no window there is nothing to pick for.
    fn pick_mod(&self, window: Option<&MainWindow>, intents: &Intents<Intent>) {
        let Some(window) = window else {
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
