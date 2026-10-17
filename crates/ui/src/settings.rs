//! The Settings page (`spec/shell-and-settings.md` §10): module switches that save the moment they flip, and six
//! locations that wait for "Save locations". `SettingsPage` is the page's state and rules, testable without a window;
//! `bind` puts it behind the `SettingsState` global.

use crate::{LocationSetting, MainWindow, MessageTone, ModuleRow, SettingsSectionRow, SettingsState};
use eg_core::settings::{AppSettings, Locations};
use eg_core::settings_service::{Change, SaveOutcome, SettingsService, Snapshot};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What Settings shows of a part of the app it can turn on or off, before loading it: a module that is off is never
/// loaded.
#[derive(Clone, Debug)]
pub struct ModuleChoice {
    /// Its settings and its on/off choice are kept under this.
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// How a sentence names it: "the trainer".
    pub name: &'static str,
    /// Whether it is on without a choice (the trainer: on in a source checkout, off in a release).
    pub default_on: bool,
    /// Why it could not start in this run.
    pub problem: Option<String>,
    pub sections: Vec<ModuleSection>,
}

/// Sections supplied by the module. Persistence owns no knowledge of their keys or defaults.
#[derive(Clone, Debug)]
pub enum ModuleSection {
    Toggle {
        key: &'static str,
        title: &'static str,
        description: &'static str,
        note: &'static str,
        default: bool,
        on_text: &'static str,
        off_text: &'static str,
    },
    Status {
        title: &'static str,
        description: &'static str,
        text: &'static str,
        action: &'static str,
        unavailable: &'static str,
    },
}

impl ModuleChoice {
    pub fn validate(&self, settings: &AppSettings) -> Result<(), String> {
        let Some(slice) = settings.module_settings.get(self.id) else {
            return Ok(());
        };
        if slice.is_null() {
            return Ok(());
        }
        let Some(fields) = slice.as_object() else {
            return Err(format!("The settings for {} must be an object.", self.title));
        };
        for section in &self.sections {
            if let ModuleSection::Toggle { key, .. } = section {
                for (_, value) in fields.iter().filter(|(name, _)| name.eq_ignore_ascii_case(key)) {
                    if !value.is_boolean() {
                        return Err(format!("{}: {key} must be true or false. Its settings were left unchanged.", self.title));
                    }
                }
            }
        }
        Ok(())
    }
}

/// What the message is: one kind, so the action bar shows exactly one icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Saved,
    Saving,
    MemoryOnly,
    RestartRequired,
    Error,
}

#[derive(Clone, Debug)]
struct ModuleSwitch {
    choice: ModuleChoice,
    enabled: bool,
    /// Whether this run has the module (its choice when the app started), whatever the switch says now.
    enabled_at_start: bool,
}

/// The six location boxes, in the page's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    GameExecutable,
    UserData,
    ModLibrary,
    ModSources,
    Dotnet,
    MelonLoader,
}

impl Location {
    pub const ALL: [Location; 6] = [
        Location::GameExecutable,
        Location::UserData,
        Location::ModLibrary,
        Location::ModSources,
        Location::Dotnet,
        Location::MelonLoader,
    ];

    fn label(self) -> &'static str {
        match self {
            Location::GameExecutable => "Game executable",
            Location::UserData => "User-data folder",
            Location::ModLibrary => "Mod library",
            Location::ModSources => "Mod sources folder",
            Location::Dotnet => ".NET SDK folder",
            Location::MelonLoader => "MelonLoader folder",
        }
    }

    fn caption(self) -> &'static str {
        match self {
            Location::GameExecutable => "Leave empty to find the game automatically.",
            Location::UserData => {
                "Fault reports, the trainer's labels and captured game definitions, and the mod library unless it has a folder of its own. A new folder takes effect after a restart, which copies everything there."
            }
            Location::ModLibrary => "A copy of every mod version you install, and which mods are on. A new folder takes effect after a restart, which copies the library there.",
            Location::ModSources => "Your own mod projects. A mod whose project is here can be rebuilt and its build installed from the Mods page.",
            Location::Dotnet => "The folder with dotnet.exe, which builds the mod projects. Leave empty to find it automatically.",
            Location::MelonLoader => {
                "Where a game with mods runs MelonLoader, its Mods and its UserData from. Leave empty for the game folder. Another folder needs a copy of the game's MelonLoader folder, version 0.6.6 or later; its version.dll stays in the game folder. Takes effect after a restart."
            }
        }
    }

    fn browse_label(self) -> &'static str {
        match self {
            Location::GameExecutable => "Browse for the game executable",
            Location::UserData => "Browse for the user-data folder",
            Location::ModLibrary => "Browse for the mod library folder",
            Location::ModSources => "Browse for the mod sources folder",
            Location::Dotnet => "Browse for the .NET SDK folder",
            Location::MelonLoader => "Browse for the MelonLoader folder",
        }
    }

    /// The picker's title.
    pub fn picker_title(self) -> &'static str {
        match self {
            Location::GameExecutable => "Select Last Epoch executable",
            Location::UserData => "Select user-data folder",
            Location::ModLibrary => "Select the mod library folder",
            Location::ModSources => "Select the mod sources folder",
            Location::Dotnet => "Select the folder with dotnet.exe",
            Location::MelonLoader => "Select the folder MelonLoader runs from",
        }
    }
}

pub struct SettingsPage {
    service: SettingsService,
    snapshot: Snapshot,
    modules: Vec<ModuleSwitch>,
    boxes: [String; 6],
    placeholders: [String; 6],
    message: String,
    kind: MessageKind,
    can_restart_at_all: bool,
    edits: [u64; 6],
    location_save: Option<(u64, [u64; 6])>,
    location_error: Option<String>,
    external_error: Option<String>,
    section_edits: std::collections::BTreeMap<(usize, usize), (u64, bool)>,
    close_observed: Option<u64>,
}

impl SettingsPage {
    pub fn new(service: SettingsService, modules: Vec<ModuleChoice>, found_game: Option<PathBuf>, found_dotnet: Option<PathBuf>, can_restart: bool) -> Self {
        let snapshot = service.snapshot();
        let store = &snapshot.store;
        let modules = modules
            .into_iter()
            .map(|mut choice| {
                if let Err(error) = choice.validate(store.saved()) {
                    choice.problem = Some(error);
                }
                ModuleSwitch {
                    enabled: store.saved().modules.get(choice.id).copied().unwrap_or(choice.default_on),
                    enabled_at_start: store.startup_modules().get(choice.id).copied().unwrap_or(choice.default_on) && choice.problem.is_none(),
                    choice,
                }
            })
            .collect();
        let text = |path: Option<&Path>| path.map(|path| path.display().to_string()).unwrap_or_default();
        let paths = store.paths();
        let placeholders = [
            text(found_game.as_deref()),
            text(Some(&paths.default_user_data_directory)),
            text(Some(&paths.default_mod_library_directory())),
            paths.mod_sources_directory().map_or_else(|| "None".to_owned(), |path| path.display().to_string()),
            found_dotnet.map_or_else(|| "Not found".to_owned(), |path| path.display().to_string()),
            "The game folder".into(),
        ];
        let message = store.startup_error().unwrap_or_default().to_owned();
        let mut page = Self {
            service,
            snapshot,
            modules,
            boxes: Default::default(),
            placeholders,
            kind: if message.is_empty() { MessageKind::Saved } else { MessageKind::Error },
            message: message.clone(),
            can_restart_at_all: can_restart,
            edits: [0; 6],
            location_save: None,
            location_error: None,
            external_error: (!message.is_empty()).then_some(message.clone()),
            section_edits: Default::default(),
            close_observed: None,
        };
        page.boxes = location_text(page.snapshot.store.saved());
        page
    }

    pub fn module_enabled(&self, index: usize) -> bool {
        self.modules.get(index).is_some_and(|module| module.enabled)
    }
    pub fn module_problem(&self, index: usize) -> Option<&str> {
        self.modules.get(index).and_then(|module| module.choice.problem.as_deref())
    }
    pub fn location(&self, location: Location) -> &str {
        &self.boxes[location as usize]
    }
    pub fn placeholder(&self, location: Location) -> &str {
        &self.placeholders[location as usize]
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn kind(&self) -> MessageKind {
        self.kind
    }
    pub fn can_restart(&self) -> bool {
        !self.snapshot.saving() && self.kind == MessageKind::RestartRequired && self.can_restart_at_all
    }
    pub fn saving(&self) -> bool {
        self.service.is_saving()
    }
    pub fn can_retry(&self) -> bool {
        !self.saving() && self.snapshot.store.has_pending_write()
    }

    pub fn edit(&mut self, location: Location, text: &str) {
        self.boxes[location as usize] = text.to_owned();
        self.edits[location as usize] += 1;
    }

    pub fn has_unsaved_locations(&self) -> bool {
        self.boxes
            .iter()
            .zip(location_text(self.snapshot.store.saved()))
            .any(|(typed, saved)| !eg_core::paths::same_text(typed.trim(), &saved))
    }

    pub fn can_save_locations(&self) -> bool {
        self.has_unsaved_locations() || self.snapshot.store.unreadable()
    }

    pub fn flip_module(&mut self, index: usize, on: bool) {
        let Some(module) = self.modules.get_mut(index) else {
            return;
        };
        if module.enabled == on {
            return;
        }
        module.enabled = on;
        let result = self.service.submit(Change::ModuleEnabled {
            id: module.choice.id.into(),
            enabled: on,
            default: module.choice.default_on,
        });
        self.submitted(result);
    }

    pub fn flip_setting(&mut self, module_index: usize, section_index: usize, on: bool) {
        let Some(module) = self.modules.get(module_index) else {
            return;
        };
        if !module.enabled_at_start || module.choice.problem.is_some() {
            return;
        }
        let Some(ModuleSection::Toggle { key, .. }) = module.choice.sections.get(section_index) else {
            return;
        };
        let result = self.service.module(module.choice.id).set_bool(key, on);
        if let Ok(revision) = result {
            self.section_edits.insert((module_index, section_index), (revision, on));
        }
        self.submitted(result);
    }

    pub fn save_locations(&mut self) {
        self.location_error = None;
        let [game, user_data, library, sources, dotnet, loader] = self.boxes.clone();
        let result = self.service.submit(Change::Locations(Locations {
            game_executable: game,
            user_data_directory: user_data,
            mod_sources_directory: Some(sources),
            mod_library_directory: Some(library),
            dotnet_directory: Some(dotnet),
            melon_loader_directory: Some(loader),
        }));
        if let Ok(revision) = result {
            self.location_save = Some((revision, self.edits));
        }
        self.submitted(result);
    }

    pub fn retry(&mut self) {
        let result = self.service.submit(Change::Retry);
        self.submitted(result);
    }

    /// A newly failed save must be shown before closing, even if it finished between the last frame and the click.
    fn close_decision(&mut self) -> CloseDecision {
        let observed = *self.close_observed.get_or_insert(self.snapshot.completed);
        if !self.service.pause_when_idle() {
            return CloseDecision::Wait;
        }
        self.close_observed = None;
        self.refresh();
        if self.snapshot.completed > observed && matches!(self.snapshot.outcome, SaveOutcome::Failed(_)) {
            self.service.resume();
            CloseDecision::ShowFailure
        } else {
            CloseDecision::Close
        }
    }

    fn submitted(&mut self, result: Result<u64, String>) {
        match result {
            Ok(_) => {
                self.message = "Saving settings…".into();
                self.kind = MessageKind::Saving;
                self.refresh();
            }
            Err(reason) => self.set_error(&reason),
        }
    }

    /// Applies only newer observations. Location acknowledgements never overwrite later typing.
    pub fn refresh(&mut self) {
        let Some(snapshot) = self.service.snapshot_after(self.snapshot.completed) else {
            return;
        };
        self.snapshot = snapshot;
        self.section_edits.retain(|_, (revision, _)| *revision > self.snapshot.completed);
        if let Some((revision, edits)) = self.location_save
            && let Some((completed, result)) = &self.snapshot.locations
            && *completed >= revision
        {
            match result {
                Ok(saved) => {
                    self.location_error = None;
                    self.external_error = None;
                    for (index, value) in location_text(saved).into_iter().enumerate() {
                        if edits[index] == self.edits[index] {
                            self.boxes[index] = value;
                        }
                    }
                }
                Err(error) => self.location_error = Some(format!("Locations were not saved: {error}")),
            }
            if self.snapshot.pending_location != Some(revision) {
                self.location_save = None;
            }
        }
        // A completion from an older request cannot reset a newer optimistic switch or its message.
        if self.snapshot.saving() {
            return;
        }
        for module in &mut self.modules {
            module.enabled = self.snapshot.store.desired().modules.get(module.choice.id).copied().unwrap_or(module.choice.default_on);
        }
        match self.snapshot.outcome.clone() {
            SaveOutcome::Persisted => self.describe_saved(),
            SaveOutcome::MemoryOnly => {
                self.message = "Settings changed for this run only. The unreadable file is unchanged; Save locations replaces it.".into();
                self.kind = MessageKind::MemoryOnly;
            }
            SaveOutcome::Failed(reason) | SaveOutcome::Faulted(reason) => {
                let message = self.location_error.clone().unwrap_or_else(|| format!("Settings were not saved: {reason}"));
                self.set_error(&message);
            }
        }
    }

    pub fn report_error(&mut self, message: &str) {
        self.external_error = Some(message.into());
        self.set_error(message);
    }

    fn set_error(&mut self, message: &str) {
        self.message = message.into();
        self.kind = MessageKind::Error;
    }

    fn describe_saved(&mut self) {
        if let Some(error) = self.location_error.clone().or_else(|| self.external_error.clone()) {
            self.set_error(&error);
            return;
        }
        let store = &self.snapshot.store;
        let saved = store.saved();
        let differs = |active: Option<&Path>, saved: &Option<PathBuf>| match (active, saved) {
            (None, None) => false,
            (Some(active), Some(saved)) => !eg_core::paths::same_text(&active.to_string_lossy(), &saved.to_string_lossy()),
            _ => true,
        };
        let paths = saved.migration_source.is_some()
            || saved.mod_library_migration_source.is_some()
            || differs(store.active_game_executable(), &saved.game_executable)
            || differs(store.active_melon_loader_directory(), &saved.melon_loader_directory);
        let turns = self
            .modules
            .iter()
            .filter_map(|module| {
                let on = saved.modules.get(module.choice.id).copied().unwrap_or(module.choice.default_on);
                let active = store.startup_modules().get(module.choice.id).copied().unwrap_or(module.choice.default_on);
                (on != active).then(|| format!("{} {}", module.choice.name, if on { "on" } else { "off" }))
            })
            .collect::<Vec<_>>()
            .join(" and ");
        self.kind = if paths || !turns.is_empty() { MessageKind::RestartRequired } else { MessageKind::Saved };
        self.message = match (paths, !turns.is_empty()) {
            (true, true) => format!("Saved. Restart Eterra's Gift to use the new locations and turn {turns}."),
            (true, false) => "Saved. Restart Eterra's Gift to use the new locations.".into(),
            (false, true) => format!("Saved. Restart Eterra's Gift to turn {turns}."),
            (false, false) => "Settings saved.".into(),
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseDecision {
    Wait,
    ShowFailure,
    Close,
}

fn location_text(saved: &AppSettings) -> [String; 6] {
    [
        &saved.game_executable,
        &saved.user_data_directory,
        &saved.mod_library_directory,
        &saved.mod_sources_directory,
        &saved.dotnet_directory,
        &saved.melon_loader_directory,
    ]
    .map(|path| path.as_ref().map(|path| path.display().to_string()).unwrap_or_default())
}

/// Puts the page and module-supplied sections behind the window's `SettingsState`.
pub fn bind(window: &MainWindow, page: SettingsPage) {
    bind_with_close(window, page, None);
}

pub(crate) fn bind_with_close(window: &MainWindow, page: SettingsPage, before_close: Option<Rc<dyn Fn() -> CloseDecision>>) {
    let page = Rc::new(RefCell::new(page));
    let state = window.global::<SettingsState>();

    let locations = Rc::new(VecModel::default());
    state.set_locations(ModelRc::from(locations.clone()));
    let view = Rc::new(View {
        page,
        locations,
        window: window.as_weak(),
        modules: Rc::new(VecModel::default()),
        sections: Rc::new(VecModel::default()),
        watch: slint::Timer::default(),
        closing: Cell::new(false),
        before_close,
    });
    state.set_modules(ModelRc::from(view.modules.clone()));
    state.set_sections(ModelRc::from(view.sections.clone()));
    view.show_all();
    view.watch.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(40), {
        let view = Rc::downgrade(&view);
        move || {
            if let Some(view) = view.upgrade() {
                let observed = view.page.borrow().snapshot.completed;
                if view.closing.get() {
                    let decision = view.close_decision();
                    match decision {
                        CloseDecision::Wait => {}
                        CloseDecision::ShowFailure => {
                            view.closing.set(false);
                        }
                        CloseDecision::Close => {
                            view.closing.set(false);
                            if let Some(window) = view.window.upgrade() {
                                let _ = window.hide();
                            }
                            return;
                        }
                    }
                } else {
                    view.page.borrow_mut().refresh();
                }
                if matches!(view.page.borrow().snapshot.outcome, SaveOutcome::Faulted(_)) {
                    let _ = slint::quit_event_loop();
                }
                if view.page.borrow().snapshot.completed != observed {
                    view.show_all();
                }
            }
        }
    });
    window.window().on_close_requested({
        let view = view.clone();
        move || {
            let decision = view.close_decision();
            match decision {
                CloseDecision::Close => slint::CloseRequestResponse::HideWindow,
                CloseDecision::Wait => {
                    view.closing.set(true);
                    slint::CloseRequestResponse::KeepWindowShown
                }
                CloseDecision::ShowFailure => {
                    view.show_all();
                    slint::CloseRequestResponse::KeepWindowShown
                }
            }
        }
    });

    state.on_module_toggled({
        let view = view.clone();
        move |index, on| {
            view.page.borrow_mut().flip_module(index as usize, on);
            view.show();
        }
    });
    state.on_setting_toggled({
        let view = view.clone();
        move |module, section, on| {
            view.page.borrow_mut().flip_setting(module as usize, section as usize, on);
            view.show();
        }
    });
    state.on_retry({
        let view = view.clone();
        move || {
            view.page.borrow_mut().retry();
            view.show_all();
        }
    });
    state.on_location_edited({
        let view = view.clone();
        move |index, text| {
            if let Some(&location) = Location::ALL.get(index as usize) {
                view.page.borrow_mut().edit(location, &text);
                view.show_row(location);
                view.show();
            }
        }
    });
    state.on_save_locations({
        let view = view.clone();
        move || {
            view.page.borrow_mut().save_locations();
            view.show_all();
        }
    });
    state.on_browse({
        let view = view.clone();
        move |index| {
            if let Some(&location) = Location::ALL.get(index as usize) {
                view.browse(location);
            }
        }
    });
    // TODO(launcher): Restart now exits with 42 for the launcher to start the app again; no Rust launcher exists yet,
    // so `can-restart` stays false and this is never offered.
    state.on_restart(|| eprintln!("TODO: Restart now needs the launcher"));
}

struct View {
    before_close: Option<Rc<dyn Fn() -> CloseDecision>>,
    page: Rc<RefCell<SettingsPage>>,
    locations: Rc<VecModel<LocationSetting>>,
    window: Weak<MainWindow>,
    modules: Rc<VecModel<ModuleRow>>,
    sections: Rc<VecModel<SettingsSectionRow>>,
    watch: slint::Timer,
    closing: Cell<bool>,
}

impl View {
    fn close_decision(&self) -> CloseDecision {
        if let Some(before_close) = &self.before_close {
            let decision = before_close();
            if decision != CloseDecision::Close {
                return decision;
            }
        }
        let decision = self.page.borrow_mut().close_decision();
        if decision == CloseDecision::ShowFailure
            && let Some(window) = self.window.upgrade()
        {
            window.global::<crate::Nav>().invoke_navigate("settings".into());
        }
        decision
    }
    fn row(&self, location: Location) -> LocationSetting {
        let page = self.page.borrow();
        LocationSetting {
            label: location.label().into(),
            value: page.location(location).into(),
            placeholder: page.placeholder(location).into(),
            caption: location.caption().into(),
            browse_label: location.browse_label().into(),
        }
    }

    fn show_row(&self, location: Location) {
        let row = self.row(location);
        if self.locations.row_data(location as usize).as_ref() != Some(&row) {
            self.locations.set_row_data(location as usize, row);
        }
    }

    fn show_all(&self) {
        if self.locations.row_count() == 0 {
            for location in Location::ALL {
                self.locations.push(self.row(location));
            }
        } else {
            for location in Location::ALL {
                self.show_row(location);
            }
        }
        self.show();
    }

    fn show(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let page = self.page.borrow();
        let state = window.global::<SettingsState>();
        let modules = page
            .modules
            .iter()
            .map(|module| ModuleRow {
                title: module.choice.title.into(),
                description: module.choice.description.into(),
                problem: module.choice.problem.as_deref().unwrap_or_default().into(),
                enabled: module.enabled,
            })
            .collect();
        update_rows(&self.modules, modules);
        let mut sections = Vec::new();
        for (module_index, module) in page.modules.iter().enumerate().filter(|(_, module)| module.enabled_at_start) {
            for (section_index, section) in module.choice.sections.iter().enumerate() {
                let mut row = SettingsSectionRow {
                    module_index: module_index as i32,
                    section_index: section_index as i32,
                    ..Default::default()
                };
                match section {
                    ModuleSection::Toggle {
                        key,
                        title,
                        description,
                        note,
                        default,
                        on_text,
                        off_text,
                    } => {
                        row.title = (*title).into();
                        row.description = (*description).into();
                        row.note = (*note).into();
                        row.toggle = true;
                        row.on_text = (*on_text).into();
                        row.off_text = (*off_text).into();
                        row.checked = page
                            .snapshot
                            .store
                            .desired()
                            .module_settings
                            .get(module.choice.id)
                            .and_then(|slice| slice.as_object())
                            .and_then(|fields| fields.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)))
                            .and_then(|(_, value)| value.as_bool())
                            .unwrap_or(*default);
                        if let Some((_, on)) = page.section_edits.get(&(module_index, section_index)) {
                            row.checked = *on;
                        }
                    }
                    ModuleSection::Status {
                        title,
                        description,
                        text,
                        action,
                        unavailable,
                    } => {
                        row.title = (*title).into();
                        row.description = (*description).into();
                        row.status = (*text).into();
                        row.action = (*action).into();
                        row.unavailable = (*unavailable).into();
                    }
                }
                sections.push(row);
            }
        }
        update_rows(&self.sections, sections);
        state.set_saving(page.saving());
        state.set_can_retry(page.can_retry());
        state.set_locations_changed(page.can_save_locations());
        state.set_message(page.message().into());
        state.set_message_tone(match page.kind() {
            _ if page.message().is_empty() => MessageTone::None,
            MessageKind::Saved => MessageTone::Confirmed,
            MessageKind::RestartRequired | MessageKind::Saving | MessageKind::MemoryOnly => MessageTone::Info,
            MessageKind::Error => MessageTone::Error,
        });
        state.set_can_restart(page.can_restart());
    }

    /// Opens a picker, parented to the window, and puts the chosen path in its box.
    fn browse(self: &Rc<Self>, location: Location) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let dialog = rfd::AsyncFileDialog::new().set_title(location.picker_title()).set_parent(&window.window().window_handle());
        let view = self.clone();
        let picked = slint::spawn_local(async move {
            let picked = if location == Location::GameExecutable {
                dialog.add_filter("Windows executable", &["exe"]).pick_file().await
            } else {
                dialog.pick_folder().await
            };
            if let Some(picked) = picked {
                view.page.borrow_mut().edit(location, &picked.path().display().to_string());
                view.show_row(location);
                view.show();
            }
        });
        if let Err(error) = picked {
            let picker = if location == Location::GameExecutable { "executable picker" } else { "folder picker" };
            self.page.borrow_mut().report_error(&format!("The {picker} could not be opened: {error}"));
            self.show();
        }
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use eg_core::settings::SettingsStore;
    use std::fs;

    fn trainer(default_on: bool) -> ModuleChoice {
        ModuleChoice {
            id: "trainer",
            title: "Trainer",
            description: "",
            name: "the trainer",
            default_on,
            problem: None,
            sections: Vec::new(),
        }
    }

    fn page(temp: &Path) -> SettingsPage {
        let root = temp.join("app");
        fs::create_dir_all(&root).unwrap();
        let store = SettingsStore::open(&root, &temp.join("settings.json"), &[]);
        SettingsPage::new(SettingsService::inline(store), vec![trainer(true)], None, None, true)
    }

    #[test]
    fn a_switch_saves_as_it_flips_and_waits_for_a_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut page = page(temp.path());
        assert!(page.module_enabled(0));
        page.flip_module(0, false);
        assert_eq!(page.message(), "Saved. Restart Eterra's Gift to turn the trainer off.");
        assert!(page.can_restart());
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(temp.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["modules"]["trainer"], false);
        // Back to the default: the choice leaves the file, and nothing waits any more.
        page.flip_module(0, true);
        assert_eq!(page.message(), "Settings saved.");
        assert_eq!(page.kind(), MessageKind::Saved);
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(temp.path().join("settings.json")).unwrap()).unwrap();
        assert!(saved.get("modules").is_none());
    }

    #[test]
    fn locations_save_on_request_and_show_as_saved() {
        let temp = tempfile::tempdir().unwrap();
        let mut page = page(temp.path());
        assert!(!page.can_save_locations());
        page.edit(Location::UserData, "data");
        assert!(page.can_save_locations());
        page.save_locations();
        assert_eq!(page.kind(), MessageKind::Error);
        assert!(page.message().starts_with("Locations were not saved: Enter the full path of the user-data folder"));
        let moved = temp.path().join("moved");
        page.edit(Location::UserData, &format!(" {}\\ ", moved.display()));
        page.save_locations();
        assert_eq!(page.message(), "Saved. Restart Eterra's Gift to use the new locations.");
        assert_eq!(page.location(Location::UserData), moved.display().to_string());
        assert!(!page.can_save_locations());
        // Upper or lower case, a box that says what is saved is not a change.
        page.edit(Location::UserData, &moved.display().to_string().to_uppercase());
        assert!(!page.can_save_locations());
    }

    #[test]
    fn a_switch_does_not_clear_an_error_it_did_not_cause() {
        let temp = tempfile::tempdir().unwrap();
        let mut page = page(temp.path());
        page.edit(Location::Dotnet, &temp.path().display().to_string());
        page.save_locations();
        let refused = page.message().to_owned();
        page.flip_module(0, false);
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
        page.flip_module(0, false);
        assert_eq!(page.message(), original);
        assert_eq!(page.kind(), MessageKind::Error);
        page.edit(Location::GameExecutable, "");
        page.save_locations();
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
        let service = SettingsService::new(store);
        let mut page = SettingsPage::new(service.clone(), vec![trainer(true)], None, None, true);
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
        page.save_locations();
        assert_eq!(page.kind(), MessageKind::Saving);
        page.edit(Location::UserData, &later.display().to_string());
        // A newer switch is queued too; neither acknowledgement may erase the newer location edit.
        page.flip_module(0, false);
        let ticket = service.snapshot().submitted;
        release.send(()).unwrap();
        service.wait(ticket);
        page.refresh();
        assert_eq!(page.location(Location::UserData), later.display().to_string());
        assert!(page.can_save_locations());
        assert_eq!(service.snapshot().store.saved().user_data_directory, Some(first));
        assert!(!page.module_enabled(0));
    }

    #[test]
    fn retry_acknowledges_and_normalizes_a_failed_location_write() {
        let temp = tempfile::tempdir().unwrap();
        let mut page = page(temp.path());
        let file = temp.path().join("settings.json");
        fs::create_dir(&file).unwrap();
        let moved = temp.path().join("moved");
        page.edit(Location::UserData, &format!(" {}\\ ", moved.display()));
        page.save_locations();
        assert_eq!(page.kind(), MessageKind::Error);
        assert!(page.can_retry());
        fs::remove_dir(&file).unwrap();
        page.retry();
        assert_eq!(page.kind(), MessageKind::RestartRequired);
        assert_eq!(page.location(Location::UserData), moved.display().to_string());
        assert!(!page.can_retry());
        assert!(!page.can_save_locations());
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
        let mut second = crate::trainer_settings::descriptor(true);
        second.id = "second";
        let mut page = SettingsPage::new(service.clone(), vec![crate::trainer_settings::descriptor(true), second], None, None, false);
        assert!(page.module_problem(0).is_some());
        assert!(!page.modules[0].enabled_at_start);
        assert!(page.modules[1].enabled_at_start);
        page.flip_setting(0, 0, false); // Invalid module cannot overwrite its malformed slice.
        page.flip_setting(1, 0, false);
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
        page.flip_module(0, false);
        assert_eq!(page.kind(), MessageKind::MemoryOnly);
        assert!(!page.can_restart());
        assert!(page.message().contains("this run only"));
    }

    #[test]
    fn closing_waits_for_accepted_saves_and_shows_a_failure_before_exiting() {
        for fails in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let file = temp.path().join("settings.json");
            let service = SettingsService::new(SettingsStore::open(&temp.path().join("app"), &file, &[]));
            let mut page = SettingsPage::new(service.clone(), vec![trainer(true)], None, None, false);
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
            page.flip_module(0, false);
            assert_eq!(page.close_decision(), CloseDecision::Wait);
            let revision = service.snapshot().submitted;
            release.send(()).unwrap();
            service.wait(revision);
            // Another callback can observe the failure while a close is pending.
            page.refresh();
            assert_eq!(page.close_decision(), if fails { CloseDecision::ShowFailure } else { CloseDecision::Close });
            assert_eq!(page.close_decision(), CloseDecision::Close);
            if fails {
                assert!(page.can_retry());
            }
        }
    }
}
