//! Settings in the window: the `eg_core::settings_page` feature's impure half. It shows the feature's state in the
//! `SettingsState` global, and saves, picks files, tells linked modules and closes the window when the feature says to.
use phrases::phrases;

pub mod trainer;

pub use eg_core::settings_page::{LiveStatus, ModuleChoice, ModuleLink, ModuleSection};

use crate::shell::{Intent as ShellIntent, Page, ShellHandle};
use crate::{LocationSetting, MainWindow, MessageTone, ModuleRow, SettingsSectionRow, SettingsState};
use eg_core::settings_page::{
    Effect, Intent, Location, LocationView, MessageKind, ModuleRowView, Output, SectionBody, SectionView, SettingsPage, State, Subscription, submit, try_pause,
};
use eg_core::settings_service::SettingsService;
use slint::{ComponentHandle, Timer};
use spiderland::{Intents, Slice, Store, Version, callbacks};
use spiderland_slint::{Rows, WindowHost, Windowed, on_wake};
use std::any::Any;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

phrases! {
    WindowsExecutable: "Windows executable"
    ExecutablePickerFailed: "The executable picker could not be opened: {error}"
    FolderPickerFailed: "The folder picker could not be opened: {error}"
}

/// Settings' Restart now, as the app's composition reads it once the window has closed: the app then starts itself again.
#[derive(Clone, Debug, Default)]
pub struct RestartRequest(Rc<Cell<bool>>);

impl RestartRequest {
    pub fn requested(&self) -> bool {
        self.0.get()
    }
}

/// What the page is built from, by whoever composes the app.
pub struct Settings {
    service: SettingsService,
    state: State,
    links: BTreeMap<usize, Rc<dyn ModuleLink>>,
    restart: RestartRequest,
}

impl Settings {
    pub fn new(service: SettingsService, modules: Vec<ModuleChoice>, found_game: Option<PathBuf>, found_dotnet: Option<PathBuf>) -> Self {
        Settings {
            state: State::new(service.snapshot(), modules, found_game, found_dotnet),
            service,
            links: BTreeMap::new(),
            restart: RestartRequest::default(),
        }
    }

    /// Whether Restart now closed the window, for the app to exit asking for a restart.
    pub fn restart_request(&self) -> RestartRequest {
        self.restart.clone()
    }

    /// Links the running module `id` to its sections. The module already started with the saved choices, so linking
    /// sends nothing; only flips do.
    pub fn link(mut self, id: &str, link: Rc<dyn ModuleLink>) -> Self {
        if let Some(index) = self.state.started_module(id) {
            self.links.insert(index, link);
        }
        self
    }
}

struct Window {
    service: SettingsService,
    links: BTreeMap<usize, Rc<dyn ModuleLink>>,
    restart: RestartRequest,
    modules: Rows<ModuleRow>,
    sections: Rows<SettingsSectionRow>,
    locations: Rows<LocationSetting>,
    shown_rows: Slice<(Version, Version, Version, Version)>,
    shown_locations: Slice<Version>,
}

/// Puts the page and module-supplied sections behind the window's `SettingsState`, and takes the window's close: the
/// `close_first` pages that go first hear of it through [`Output::Lifecycle`], and answer with [`Intent::CloseAnswered`].
/// Returns the store, for them to be wired to.
pub(crate) fn bind(window: &MainWindow, settings: Settings, close_first: usize, shell: &ShellHandle) -> Store<SettingsPage> {
    let Settings {
        service,
        mut state,
        links,
        restart,
    } = settings;
    state.set_close_first(close_first);
    let host = Window {
        service: service.clone(),
        links,
        restart,
        modules: Rows::default(),
        sections: Rows::default(),
        locations: Rows::default(),
        shown_rows: Slice::default(),
        shown_locations: Slice::default(),
    };
    let view = window.global::<SettingsState>();
    view.set_version(env!("CARGO_PKG_VERSION").into());
    view.set_modules(host.modules.model());
    view.set_sections(host.sections.model());
    view.set_locations(host.locations.model());
    let store = Store::<SettingsPage>::new(state, Windowed::new(window, host));
    // A save that finished before the listener was there, and what linked modules say to begin with.
    store.send(Intent::Observed(Box::new(service.snapshot())));
    store.send(Intent::LinkChanged);

    // A close always waits here and goes ahead from `Effect::Close`, so it can wait for saves in flight.
    window.window().on_close_requested({
        let store = store.clone();
        move || {
            store.send(Intent::CloseRequested);
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
    store.forward(&shell.intents(), |output| (*output == Output::ShowPage).then_some(ShellIntent::Navigate(Page::Settings)));
    callbacks!(view, store, {
        on_save_locations: || Intent::SaveLocations,
        on_retry: || Intent::Retry,
        on_restart: || Intent::Restart,
        on_module_toggled: |index, on| Intent::ToggleModule(index as usize, on),
        on_setting_toggled: |module, section, on| Intent::ToggleSetting(module as usize, section as usize, on),
        on_section_action: |module, section| Intent::Act(module as usize, section as usize),
    });
    view.on_location_edited({
        let store = store.clone();
        move |index, text| {
            if let Some(&location) = Location::ALL.get(index as usize) {
                store.send(Intent::Edit(location, text.into()));
            }
        }
    });
    view.on_browse({
        let store = store.clone();
        move |index| {
            if let Some(&location) = Location::ALL.get(index as usize) {
                store.send(Intent::Browse(location));
            }
        }
    });
    store
}

fn module_row(row: ModuleRowView) -> ModuleRow {
    ModuleRow {
        title: row.title.into(),
        description: row.description.into(),
        problem: row.problem.into(),
        enabled: row.enabled,
    }
}

fn section_row(view: SectionView) -> SettingsSectionRow {
    let mut row = SettingsSectionRow {
        module_index: view.module as i32,
        section_index: view.section as i32,
        title: view.title.into(),
        description: view.description.into(),
        ..Default::default()
    };
    match view.body {
        SectionBody::Toggle {
            label,
            note,
            on_text,
            off_text,
            checked,
        } => {
            row.toggle = true;
            row.label = label.into();
            row.note = note.into();
            row.on_text = on_text.into();
            row.off_text = off_text.into();
            row.checked = checked;
        }
        SectionBody::Status {
            status,
            action,
            action_enabled,
            action_tip,
            unavailable,
        } => {
            row.status = status.into();
            row.action = action.into();
            row.action_enabled = action_enabled;
            row.action_tip = action_tip.into();
            row.unavailable = unavailable.into();
        }
    }
    row
}

fn location_row(view: LocationView) -> LocationSetting {
    LocationSetting {
        label: view.label.into(),
        name: view.name.into(),
        value: view.value.into(),
        placeholder: view.placeholder.into(),
        caption: view.caption.into(),
        browse_label: view.browse_label.into(),
    }
}

impl WindowHost<SettingsPage> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
        let (modules, sections, locations) = (&self.modules, &self.sections, &self.locations);
        self.shown_rows.update(state.rows_version(), || {
            modules.sync(state.rows().into_iter().map(module_row));
            sections.sync(state.sections().into_iter().map(section_row));
        });
        self.shown_locations
            .update(state.boxes_version(), || locations.sync(state.locations().into_iter().map(location_row)));
        let view = window.global::<SettingsState>();
        view.set_saving(state.saving());
        view.set_saving_locations(state.saving_locations());
        view.set_can_retry(state.can_retry());
        view.set_locations_changed(state.can_save_locations());
        view.set_message(state.message().into());
        view.set_message_tone(match state.kind() {
            _ if state.message().is_empty() => MessageTone::None,
            // Saving is a note, not a confirmation: the check is for what the game has read back.
            MessageKind::Saved => MessageTone::Note,
            MessageKind::RestartRequired | MessageKind::MemoryOnly => MessageTone::Info,
            MessageKind::Error => MessageTone::Error,
        });
        view.set_can_restart(state.can_restart());
    }

    fn subscribe(&mut self, Subscription::Saves: &Subscription, intents: &Intents<Intent>) -> Box<dyn Any> {
        // The listener stays with the service; it only fires a waker, which does nothing once this page is gone.
        let reading = self.service.clone();
        let (waker, listening) = on_wake(intents, move || Intent::Observed(Box::new(reading.snapshot())));
        self.service.on_completed(move || waker.wake());
        Box::new(listening)
    }

    fn perform(&mut self, window: Option<&MainWindow>, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::Switched { module, key, on } => {
                if let Some(link) = self.links.get(&module) {
                    link.switched(key, on);
                }
            }
            Effect::Submit(save, submission) => {
                for intent in submit(&self.service, save, submission) {
                    intents.send(intent);
                }
            }
            Effect::ReadStatus { module, section } => {
                if let Some(link) = self.links.get(&module) {
                    intents.send(Intent::Status(module, section, link.status(section)));
                }
            }
            Effect::Act { module, section } => {
                if let Some(link) = self.links.get(&module) {
                    link.act(section);
                }
            }
            Effect::Browse(location) => self.browse(window, location, intents),
            Effect::TryPause => intents.send(try_pause(&self.service)),
            Effect::Resume => self.service.resume(),
            Effect::Close => {
                // Not from inside the window's own close callback, which is where this can be running.
                let Some(window) = window.map(ComponentHandle::as_weak) else {
                    return;
                };
                Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = window.upgrade() {
                        let _ = window.hide();
                    }
                });
            }
            Effect::Quit => {
                let _ = slint::quit_event_loop();
            }
            Effect::RequestRestart(on) => self.restart.0.set(on),
        }
    }
}

impl Window {
    /// Opens a picker, parented to the window, and puts the chosen path in its box. With no window there is nothing to pick for.
    fn browse(&self, window: Option<&MainWindow>, location: Location, intents: &Intents<Intent>) {
        let Some(window) = window else {
            return;
        };
        let dialog = rfd::AsyncFileDialog::new().set_title(location.picker_title()).set_parent(&window.window().window_handle());
        let picking = intents.clone();
        let picked = slint::spawn_local(async move {
            let picked = if location == Location::GameExecutable {
                dialog.add_filter(WindowsExecutable.get(), &["exe"]).pick_file().await
            } else {
                dialog.pick_folder().await
            };
            if let Some(picked) = picked {
                picking.send(Intent::Edit(location, picked.path().display().to_string()));
            }
        });
        if let Err(error) = picked {
            let failed = if location == Location::GameExecutable {
                ExecutablePickerFailed
            } else {
                FolderPickerFailed
            };
            intents.send(Intent::ReportError(failed.with(&[("error", &error)])));
        }
    }
}
