//! Settings in the window: the `eg_core::settings_page` feature's impure half. It shows the feature's state in the
//! `SettingsState` global, and saves, picks files, tells linked modules and closes the window when the feature says to.
use eg_i18n::phrases;

pub mod trainer;

pub use eg_core::settings_page::{LiveStatus, ModuleChoice, ModuleLink, ModuleSection};

use crate::{LocationSetting, MainWindow, MessageTone, ModuleRow, Nav, SettingsSectionRow, SettingsState};
use eg_core::settings_page::{Effect, Intent, Location, LocationView, MessageKind, ModuleRowView, SectionBody, SectionView, SettingsPage, State, submit, try_pause};
use eg_core::settings_service::SettingsService;
use eg_flow::{Host, Intents, Slice, Store, Subscriptions, Version};
use eg_flow_slint::{Rows, on_wake};
use slint::{ComponentHandle, Timer, Weak};
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
    /// What goes first in the window's close, and tells this page when it's done; without it, this page's turn is at once.
    close_first: Option<Rc<dyn Fn()>>,
    links: BTreeMap<usize, Rc<dyn ModuleLink>>,
    restart: RestartRequest,
    window: Weak<MainWindow>,
    modules: Rows<ModuleRow>,
    sections: Rows<SettingsSectionRow>,
    locations: Rows<LocationSetting>,
    shown_rows: Slice<(Version, Version, Version, Version)>,
    shown_locations: Slice<Version>,
    saves: Subscriptions<()>,
}

/// Puts the page and module-supplied sections behind the window's `SettingsState`. Returns where to tell the page how what
/// goes first in the window's close came out.
pub(crate) fn bind(window: &MainWindow, settings: Settings, close_first: Option<Rc<dyn Fn()>>) -> Intents<Intent> {
    let Settings { service, state, links, restart } = settings;
    let start_close = close_first.clone();
    let host = Window {
        service: service.clone(),
        close_first,
        links,
        restart,
        window: window.as_weak(),
        modules: Rows::default(),
        sections: Rows::default(),
        locations: Rows::default(),
        shown_rows: Slice::default(),
        shown_locations: Slice::default(),
        saves: Subscriptions::default(),
    };
    let view = window.global::<SettingsState>();
    view.set_modules(host.modules.model());
    view.set_sections(host.sections.model());
    view.set_locations(host.locations.model());
    let store = Store::<SettingsPage>::new(state, host);
    // A save that finished before the listener was there, and what linked modules say to begin with.
    store.send(Intent::Observed(Box::new(service.snapshot())));
    store.send(Intent::LinkChanged);

    // A close always waits here and goes ahead from `Effect::Close`, so it can wait for saves in flight.
    window.window().on_close_requested({
        let store = store.clone();
        move || {
            match &start_close {
                Some(first) => first(),
                None => store.send(Intent::CloseRequested),
            }
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
    let send = |make: fn() -> Intent| {
        let store = store.clone();
        move || store.send(make())
    };
    view.on_save_locations(send(|| Intent::SaveLocations));
    view.on_retry(send(|| Intent::Retry));
    view.on_restart(send(|| Intent::Restart));
    // A linked module's state moved on (the trainer's binding says so on each new backend snapshot).
    view.on_backend_updated(send(|| Intent::LinkChanged));
    view.on_module_toggled({
        let store = store.clone();
        move |index, on| store.send(Intent::ToggleModule(index as usize, on))
    });
    view.on_setting_toggled({
        let store = store.clone();
        move |module, section, on| store.send(Intent::ToggleSetting(module as usize, section as usize, on))
    });
    view.on_section_action({
        let store = store.clone();
        move |module, section| store.send(Intent::Act(module as usize, section as usize))
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
    store.intents()
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

impl Host<SettingsPage> for Window {
    fn render(&mut self, state: &State, intents: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
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
        // Each finished save wakes the page, whoever asked for it (the trainer saves values of its own). The listener
        // stays with the service; it only fires a waker, which does nothing once this page is gone.
        let service = &self.service;
        self.saves.sync([()], |()| {
            let reading = service.clone();
            let (waker, listening) = on_wake(intents, move || Intent::Observed(Box::new(reading.snapshot())));
            service.on_completed(move || waker.wake());
            listening
        });
    }

    fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
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
            Effect::Browse(location) => self.browse(location, intents),
            Effect::StartClose => match &self.close_first {
                Some(first) => first(),
                None => intents.send(Intent::CloseRequested),
            },
            Effect::TryPause => intents.send(try_pause(&self.service)),
            Effect::Resume => self.service.resume(),
            Effect::ShowPage => {
                if let Some(window) = self.window.upgrade() {
                    window.global::<Nav>().invoke_navigate("settings".into());
                }
            }
            Effect::Close => {
                // Not from inside the window's own close callback, which is where this can be running.
                let window = self.window.clone();
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
    /// Opens a picker, parented to the window, and puts the chosen path in its box.
    fn browse(&self, location: Location, intents: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
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
