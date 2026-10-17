//! The Settings page (`spec/shell-and-settings.md` §10): module switches that save the moment they flip, and six
//! locations that wait for "Save locations". `SettingsPage` is the page's state and rules, testable without a window;
//! `bind` puts it behind the `SettingsState` global.

use crate::{LocationSetting, MainWindow, MessageTone, SettingsState};
use eg_core::settings::{Locations, SettingsStore};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use std::cell::RefCell;
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
}

/// What the message is: one kind, so the action bar shows exactly one icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Saved,
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
    store: Rc<RefCell<SettingsStore>>,
    modules: Vec<ModuleSwitch>,
    boxes: [String; 6],
    /// What each box suggests while it is empty.
    placeholders: [String; 6],
    message: String,
    kind: MessageKind,
    /// The message is a switch save's own failure, which the next switch save replaces.
    switches_failed: bool,
    can_restart_at_all: bool,
}

impl SettingsPage {
    /// `found_game` is the installed game the app found, for the executable box's hint; `found_dotnet` the folder of the
    /// dotnet.exe found for building mods. `can_restart` says whether a launcher runs the app, so Restart now can work.
    pub fn new(store: Rc<RefCell<SettingsStore>>, modules: Vec<ModuleChoice>, found_game: Option<PathBuf>, found_dotnet: Option<PathBuf>, can_restart: bool) -> SettingsPage {
        let (modules, placeholders, message, kind) = {
            let store = store.borrow();
            let enabled = |choice: &ModuleChoice, choices: &std::collections::BTreeMap<String, bool>| choices.get(choice.id).copied().unwrap_or(choice.default_on);
            let modules = modules
                .into_iter()
                .map(|choice| ModuleSwitch {
                    enabled: enabled(&choice, &store.saved().modules),
                    enabled_at_start: enabled(&choice, store.startup_modules()),
                    choice,
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
                "The game folder".to_owned(),
            ];
            let saved = store.saved();
            let kind = if store.startup_error().is_some() {
                MessageKind::Error
            } else if saved.migration_source.is_some() || saved.mod_library_migration_source.is_some() {
                MessageKind::RestartRequired
            } else {
                MessageKind::Saved
            };
            (modules, placeholders, store.startup_error().unwrap_or_default().to_owned(), kind)
        };
        let mut page = SettingsPage {
            store,
            modules,
            boxes: Default::default(),
            placeholders,
            message,
            kind,
            switches_failed: false,
            can_restart_at_all: can_restart,
        };
        page.show_saved_locations();
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

    /// A saved change waits for a restart, and a launcher runs the app, so it can start it again at once.
    pub fn can_restart(&self) -> bool {
        self.kind == MessageKind::RestartRequired && self.can_restart_at_all
    }

    pub fn edit(&mut self, location: Location, text: &str) {
        self.boxes[location as usize] = text.to_owned();
    }

    /// A location box differs from what is saved (trimmed, without case).
    pub fn has_unsaved_locations(&self) -> bool {
        let store = self.store.borrow();
        let saved = store.saved();
        let saved = [
            &saved.game_executable,
            &saved.user_data_directory,
            &saved.mod_library_directory,
            &saved.mod_sources_directory,
            &saved.dotnet_directory,
            &saved.melon_loader_directory,
        ];
        self.boxes
            .iter()
            .zip(saved)
            .any(|(typed, saved)| !eg_core::paths::same_text(typed.trim(), &saved.as_ref().map(|path| path.display().to_string()).unwrap_or_default()))
    }

    /// Saves the locations; the switches save themselves as they change. While the settings file could not be read it
    /// is also the one way to replace that file, so it is offered even with nothing typed.
    pub fn can_save_locations(&self) -> bool {
        self.has_unsaved_locations() || self.store.borrow().unreadable()
    }

    /// A module switch is stored the moment it flips, as in Windows Settings. A choice that matches the module's
    /// default is not stored, so the switch keeps following that default.
    pub fn flip_module(&mut self, index: usize, on: bool) {
        let Some(module) = self.modules.get_mut(index) else {
            return;
        };
        if module.enabled == on {
            return;
        }
        module.enabled = on;
        let (id, default_on) = (module.choice.id, module.choice.default_on);
        let saved = self.store.borrow_mut().save_switches(|settings| {
            if on == default_on {
                settings.modules.remove(id);
            } else {
                settings.modules.insert(id.to_owned(), on);
            }
        });
        match saved {
            // A switch answers for itself only: an error it did not cause, such as the one from startup or a refused
            // location, stays.
            Ok(()) if self.kind != MessageKind::Error || self.switches_failed => self.describe_saved(""),
            Ok(()) => {}
            Err(error) => {
                self.report_error(&format!("Settings were not saved: {error}"));
                self.switches_failed = true;
            }
        }
    }

    pub fn save_locations(&mut self) {
        let [game, user_data, library, sources, dotnet, loader] = self.boxes.clone();
        let saved = self.store.borrow_mut().save_locations(&Locations {
            game_executable: game,
            user_data_directory: user_data,
            mod_sources_directory: Some(sources),
            mod_library_directory: Some(library),
            dotnet_directory: Some(dotnet),
            melon_loader_directory: Some(loader),
        });
        match saved {
            Ok(()) => {
                // The boxes show what was saved, as the store spells it (a full path, no trailing separator).
                self.show_saved_locations();
                self.describe_saved("Locations saved.");
            }
            Err(reason) => self.report_error(&format!("Locations were not saved: {reason}")),
        }
    }

    pub fn report_error(&mut self, message: &str) {
        self.message = message.to_owned();
        self.kind = MessageKind::Error;
        self.switches_failed = false;
    }

    fn show_saved_locations(&mut self) {
        let store = self.store.borrow();
        let saved = store.saved();
        let text = |path: &Option<PathBuf>| path.as_ref().map(|path| path.display().to_string()).unwrap_or_default();
        self.boxes = [
            text(&saved.game_executable),
            text(&saved.user_data_directory),
            text(&saved.mod_library_directory),
            text(&saved.mod_sources_directory),
            text(&saved.dotnet_directory),
            text(&saved.melon_loader_directory),
        ];
    }

    /// What a save leaves waiting on a restart, or `otherwise` when nothing does.
    fn describe_saved(&mut self, otherwise: &str) {
        let paths = {
            let store = self.store.borrow();
            let saved = store.saved();
            let differs = |active: Option<&Path>, saved: &Option<PathBuf>| match (active, saved) {
                (None, None) => false,
                (Some(active), Some(saved)) => !eg_core::paths::same_text(&active.to_string_lossy(), &saved.to_string_lossy()),
                _ => true,
            };
            saved.migration_source.is_some()
                || saved.mod_library_migration_source.is_some()
                || differs(store.active_game_executable(), &saved.game_executable)
                || differs(store.active_melon_loader_directory(), &saved.melon_loader_directory)
        };
        let turns: Vec<String> = self
            .modules
            .iter()
            .filter(|module| module.enabled != module.enabled_at_start)
            .map(|module| format!("{} {}", module.choice.name, if module.enabled { "on" } else { "off" }))
            .collect();
        let turns = turns.join(" and ");
        self.switches_failed = false;
        self.kind = if paths || !turns.is_empty() { MessageKind::RestartRequired } else { MessageKind::Saved };
        self.message = match (paths, !turns.is_empty()) {
            (true, true) => format!("Saved. Restart Eterra's Gift to use the new locations and turn {turns}."),
            (true, false) => "Saved. Restart Eterra's Gift to use the new locations.".to_owned(),
            (false, true) => format!("Saved. Restart Eterra's Gift to turn {turns}."),
            (false, false) => otherwise.to_owned(),
        };
    }
}

/// Puts the page behind the window's `SettingsState`. `trainer_running` shows the trainer's own groups.
pub fn bind(window: &MainWindow, page: SettingsPage, trainer_running: bool) {
    let page = Rc::new(RefCell::new(page));
    let state = window.global::<SettingsState>();
    state.set_trainer_running(trainer_running);
    let locations = Rc::new(VecModel::default());
    state.set_locations(ModelRc::from(locations.clone()));
    let view = Rc::new(View {
        page,
        locations,
        window: window.as_weak(),
    });
    view.show_all();

    state.on_trainer_toggled({
        let view = view.clone();
        move |on| {
            view.page.borrow_mut().flip_module(0, on);
            view.show();
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
    page: Rc<RefCell<SettingsPage>>,
    locations: Rc<VecModel<LocationSetting>>,
    window: Weak<MainWindow>,
}

impl View {
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
        state.set_trainer_enabled(page.module_enabled(0));
        state.set_trainer_problem(page.module_problem(0).unwrap_or_default().into());
        state.set_locations_changed(page.can_save_locations());
        state.set_message(page.message().into());
        state.set_message_tone(match page.kind() {
            _ if page.message().is_empty() => MessageTone::None,
            MessageKind::Saved => MessageTone::Confirmed,
            MessageKind::RestartRequired => MessageTone::Info,
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    fn trainer(default_on: bool) -> ModuleChoice {
        ModuleChoice {
            id: "trainer",
            title: "Trainer",
            description: "",
            name: "the trainer",
            default_on,
            problem: None,
        }
    }

    fn page(temp: &Path) -> SettingsPage {
        let root = temp.join("app");
        fs::create_dir_all(&root).unwrap();
        let store = SettingsStore::open(&root, &temp.join("settings.json"), &[]);
        SettingsPage::new(Rc::new(RefCell::new(store)), vec![trainer(true)], None, None, true)
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
        assert_eq!(page.message(), "");
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
    fn empty_boxes_suggest_their_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let page = page(temp.path());
        assert_eq!(page.placeholder(Location::GameExecutable), "");
        assert_eq!(page.placeholder(Location::UserData), temp.path().join("app").join("data").display().to_string());
        assert_eq!(page.placeholder(Location::ModSources), "None");
        assert_eq!(page.placeholder(Location::Dotnet), "Not found");
        assert_eq!(page.placeholder(Location::MelonLoader), "The game folder");
    }
}
