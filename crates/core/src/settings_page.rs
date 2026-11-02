//! The Settings page as a feature: module switches that save the moment they flip, and six locations that wait for "Save
//! locations".
//!
//! Saving is an effect, and each finished save comes back as a snapshot (`Intent::Observed`). Closing the window is a small
//! state machine here too: a close waits for accepted saves, and a save that failed in the meantime is shown instead of
//! being lost with the window. It names no module: each one describes itself as a [`ModuleChoice`].

use crate::settings::{AppSettings, Locations};
use crate::settings_service::{Change, SaveOutcome, SettingsService, Snapshot};
use spiderland::{Effects, Feature, Tracked, Version};
use spiderland_phrases::{Phrase, phrases};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

phrases! {
    NotAnObject: "The settings for {module} must be an object."
    NotTrueOrFalse: "{key} must be true or false in the settings for {module}, which were left unchanged."
    NoModSources: "None"
    NotFound: "Not found"
    GameFolder: "The game folder"
    LocationsNotSaved: "Locations were not saved: {error}"
    MemoryOnly: "Changes last for this run only. Save locations replaces the unreadable file."
    SettingsNotSaved: "Settings were not saved: {reason}"
    TurnOn: "{module} on"
    TurnOff: "{module} off"
    And: "{list} and {next}"
    SavedRestartBoth: "Saved. Restart Eterra's Gift to use the new locations and turn {turns}."
    SavedRestartLocations: "Saved. Restart Eterra's Gift to use the new locations."
    SavedRestartTurn: "Saved. Restart Eterra's Gift to turn {turns}."
    LocationsSaved: "Locations saved."
}

/// What Settings shows of a part of the app it can turn on or off, before loading it: a module that is off is never
/// loaded.
#[derive(Clone, Debug)]
pub struct ModuleChoice {
    /// Its settings and its on/off choice are kept under this.
    pub id: &'static str,
    pub title: Phrase,
    pub description: Phrase,
    /// How a sentence names it: "the trainer".
    pub name: Phrase,
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
        title: Phrase,
        /// The accessible name, when the title is not it.
        label: Phrase,
        description: Phrase,
        /// Shown when it is not empty.
        note: Option<Phrase>,
        default: bool,
        on_text: Phrase,
        off_text: Phrase,
    },
    Status {
        title: Phrase,
        description: Phrase,
        /// What it says without a running module behind it.
        text: Phrase,
        action: Phrase,
        /// The action's tip, shown while it can run.
        tip: Phrase,
        /// Why the action cannot run, beside it while it cannot.
        unavailable: Phrase,
    },
}

/// What a status section shows now, and whether its action can run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveStatus {
    pub text: String,
    /// The action is available at all: its reason hides.
    pub can_act: bool,
    /// The action is running already: the button waits, but the reason stays hidden.
    pub busy: bool,
}

/// A running module behind its sections: a switch takes effect the moment it flips, whatever its save does, and a status
/// section follows the module.
pub trait ModuleLink {
    /// The user flipped the switch saved under `key`.
    fn switched(&self, key: &str, on: bool);
    /// The status section at `section` as it stands; None keeps its static text.
    fn status(&self, section: usize) -> Option<LiveStatus>;
    /// The status section's action.
    fn act(&self, section: usize);
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
            return Err(NotAnObject.with(&[("module", &self.title)]));
        };
        for section in &self.sections {
            if let ModuleSection::Toggle { key, .. } = section {
                for (_, value) in fields.iter().filter(|(name, _)| name.eq_ignore_ascii_case(key)) {
                    if !value.is_boolean() {
                        return Err(NotTrueOrFalse.with(&[("key", key), ("module", &self.name)]));
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

    /// What the settings file holds for it.
    fn saved(self, settings: &AppSettings) -> &Option<PathBuf> {
        match self {
            Location::GameExecutable => &settings.game_executable,
            Location::UserData => &settings.user_data_directory,
            Location::ModLibrary => &settings.mod_library_directory,
            Location::ModSources => &settings.mod_sources_directory,
            Location::Dotnet => &settings.dotnet_directory,
            Location::MelonLoader => &settings.melon_loader_directory,
        }
    }

    /// What the page says about it: its label, the box's accessible name, its caption, its Browse button's name and the
    /// picker's title.
    const fn words(self) -> [Phrase; 5] {
        match self {
            Location::GameExecutable => [
                Phrase::new("Game executable"),
                Phrase::new("Game executable"),
                Phrase::new("Leave empty to find the game automatically."),
                Phrase::new("Browse for the game executable"),
                Phrase::new("Select the Last Epoch executable"),
            ],
            Location::UserData => [
                Phrase::new("User-data folder"),
                Phrase::new("User-data folder"),
                Phrase::new("Fault reports, trainer labels, game definitions and, unless it has its own folder, the mod library. A new folder takes effect after a restart."),
                Phrase::new("Browse for the user-data folder"),
                Phrase::new("Select user-data folder"),
            ],
            Location::ModLibrary => [
                Phrase::new("Mod library"),
                Phrase::new("Mod library folder"),
                Phrase::new("Every mod version you install, and which mods are on. A new folder takes effect after a restart."),
                Phrase::new("Browse for the mod library folder"),
                Phrase::new("Select the mod library folder"),
            ],
            Location::ModSources => [
                Phrase::new("Mod sources folder"),
                Phrase::new("Mod sources folder"),
                Phrase::new("Your own mod projects, for the Mods page to build and install."),
                Phrase::new("Browse for the mod sources folder"),
                Phrase::new("Select the mod sources folder"),
            ],
            Location::Dotnet => [
                Phrase::new(".NET SDK folder"),
                Phrase::new(".NET SDK folder"),
                Phrase::new("The folder with dotnet.exe, which builds mod projects. Leave empty to find it automatically."),
                Phrase::new("Browse for the .NET SDK folder"),
                Phrase::new("Select the folder with dotnet.exe"),
            ],
            Location::MelonLoader => [
                Phrase::new("MelonLoader folder"),
                Phrase::new("MelonLoader folder"),
                Phrase::new(
                    "Where games with mods run MelonLoader from. Leave empty for the game folder. Another folder needs a copy of MelonLoader 0.6.6 or later. Takes effect after a restart.",
                ),
                Phrase::new("Browse for the MelonLoader folder"),
                Phrase::new("Select the folder MelonLoader runs from"),
            ],
        }
    }

    /// The picker's title.
    pub fn picker_title(self) -> &'static str {
        self.words()[4].get()
    }
}

/// The settings page as a feature: see the module's documentation.
pub struct SettingsPage;

/// Which submission a result belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submission {
    Module,
    /// A module's switch (module index, section index), and the value it was set to.
    Setting(usize, usize, bool),
    Locations,
    Retry,
}

pub enum Intent {
    ToggleModule(usize, bool),
    ToggleSetting(usize, usize, bool),
    Edit(Location, String),
    Browse(Location),
    SaveLocations,
    Retry,
    /// The service took a change, or refused it.
    Submitted(Submission, Result<u64, String>),
    /// What the service holds now: a save finished, or a submission changed what is in flight.
    Observed(Box<Snapshot>),
    /// A linked module's state moved on.
    LinkChanged,
    /// A status section's action (module index, section index).
    Act(usize, usize),
    /// What a linked module's status section says now.
    Status(usize, usize, Option<LiveStatus>),
    /// Something went wrong outside the saves, such as a file picker.
    ReportError(String),
    Restart,
    /// It is this page's turn to close: whatever comes first in the window's close has finished.
    CloseRequested,
    /// What comes first in the window's close has a failure to show, so the window stays.
    CloseAbandoned,
    /// The answer to [`Effect::TryPause`]: saves are paused for closing, or are still running.
    Paused {
        paused: bool,
        snapshot: Box<Snapshot>,
    },
}

#[derive(Clone, Debug)]
pub enum Save {
    ModuleEnabled { id: &'static str, enabled: bool, default: bool },
    Setting { module: &'static str, key: &'static str, on: bool },
    Locations(Locations),
    Retry,
}

#[derive(Clone, Debug)]
pub enum Effect {
    /// Tell a running module a switch flipped, which takes effect whatever the save does.
    Switched {
        module: usize,
        key: &'static str,
        on: bool,
    },
    Submit(Save, Submission),
    /// Read a linked module's status section again.
    ReadStatus {
        module: usize,
        section: usize,
    },
    Act {
        module: usize,
        section: usize,
    },
    /// Open a picker for a location, whose choice comes back as `Intent::Edit`.
    Browse(Location),
    /// Begin the window's close, as if its close button were pressed.
    StartClose,
    /// Pause saves if none is running, and say which.
    TryPause,
    /// Accept saves again: the window stays to show one that failed.
    Resume,
    /// Bring the Settings page into view, to show a failed save.
    ShowPage,
    /// Close the window.
    Close,
    /// End the event loop: the settings writer has faulted.
    Quit,
    /// Ask the app to start itself again once it has closed, or stop asking.
    RequestRestart(bool),
}

/// Performs a [`Save`], returning what the feature should hear. No file is touched here in production: the service
/// queues it.
pub fn submit(service: &SettingsService, save: Save, submission: Submission) -> [Intent; 2] {
    let result = match save {
        Save::ModuleEnabled { id, enabled, default } => service.submit(Change::ModuleEnabled { id: id.into(), enabled, default }),
        Save::Setting { module, key, on } => service.module(module).set_bool(key, on),
        Save::Locations(locations) => service.submit(Change::Locations(locations)),
        Save::Retry => service.submit(Change::Retry),
    };
    [Intent::Submitted(submission, result), Intent::Observed(Box::new(service.snapshot()))]
}

/// Performs [`Effect::TryPause`].
pub fn try_pause(service: &SettingsService) -> Intent {
    let paused = service.pause_when_idle();
    Intent::Paused {
        paused,
        snapshot: Box::new(service.snapshot()),
    }
}

pub struct State {
    modules: Tracked<Vec<ModuleSwitch>>,
    snapshot: Tracked<Snapshot>,
    boxes: Tracked<[String; 6]>,
    placeholders: [String; 6],
    message: String,
    kind: MessageKind,
    edits: [u64; 6],
    /// The boxes' edit counts as Save locations took them, until the service answers with a revision.
    submitting_locations: Option<[u64; 6]>,
    location_save: Option<(u64, [u64; 6])>,
    location_error: Option<String>,
    external_error: Option<String>,
    section_edits: Tracked<BTreeMap<(usize, usize), (u64, bool)>>,
    live: Tracked<BTreeMap<(usize, usize), LiveStatus>>,
    close_observed: Option<u64>,
    /// The last save asked for was the locations', so its success says "Locations saved."; a switch's says nothing.
    locations_last: bool,
    /// The window waits to close for a save in flight.
    closing: bool,
    /// Restart now was chosen and waits for the save in flight.
    restarting: bool,
    /// Restart now led to the close under way, so a close that fails takes the request back.
    restart_requested: bool,
}

impl State {
    pub fn new(snapshot: Snapshot, modules: Vec<ModuleChoice>, found_game: Option<PathBuf>, found_dotnet: Option<PathBuf>) -> Self {
        let store = &snapshot.store;
        let modules = modules
            .into_iter()
            .map(|mut choice| {
                if let Err(error) = choice.validate(store.saved()) {
                    // A module that was to start says so as a start that failed (the composition's words); one that is off
                    // says the reason as it is.
                    choice.problem.get_or_insert(error);
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
            paths.mod_sources_directory().map_or_else(|| NoModSources.into(), |path| path.display().to_string()),
            found_dotnet.map_or_else(|| NotFound.into(), |path| path.display().to_string()),
            GameFolder.into(),
        ];
        let message = store.startup_error().unwrap_or_default().to_owned();
        // A folder move saved in an earlier run waits for a restart: Restart now is offered, with nothing to say.
        let moving = store.saved().migration_source.is_some() || store.saved().mod_library_migration_source.is_some();
        let boxes = location_text(store.saved());
        Self {
            modules: Tracked::new(modules),
            boxes: Tracked::new(boxes),
            placeholders,
            kind: if !message.is_empty() {
                MessageKind::Error
            } else if moving {
                MessageKind::RestartRequired
            } else {
                MessageKind::Saved
            },
            external_error: (!message.is_empty()).then_some(message.clone()),
            message,
            snapshot: Tracked::new(snapshot),
            edits: [0; 6],
            submitting_locations: None,
            location_save: None,
            location_error: None,
            section_edits: Tracked::default(),
            live: Tracked::default(),
            close_observed: None,
            locations_last: false,
            closing: false,
            restarting: false,
            restart_requested: false,
        }
    }

    /// The module `id`'s index, if this run has it: only a module that started can be linked.
    pub fn started_module(&self, id: &str) -> Option<usize> {
        self.modules.iter().position(|module| module.choice.id == id && module.enabled_at_start)
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
    /// Stays while a save runs; Restart now waits for the save.
    pub fn can_restart(&self) -> bool {
        self.kind == MessageKind::RestartRequired
    }
    pub fn saving(&self) -> bool {
        self.snapshot.saving()
    }
    /// A Save locations is on its way: only it greys the button (a switch's save doesn't).
    pub fn saving_locations(&self) -> bool {
        self.location_save.is_some() || self.submitting_locations.is_some()
    }
    pub fn can_retry(&self) -> bool {
        !self.saving() && self.snapshot.store.has_pending_write()
    }
    pub fn has_unsaved_locations(&self) -> bool {
        self.boxes
            .iter()
            .zip(location_text(self.snapshot.store.saved()))
            .any(|(typed, saved)| !file_ops::case::same(typed.trim(), &saved))
    }
    pub fn can_save_locations(&self) -> bool {
        self.has_unsaved_locations() || self.snapshot.store.unreadable()
    }

    /// What the location rows read.
    pub fn boxes_version(&self) -> Version {
        self.boxes.version()
    }

    /// What the module and section rows read.
    pub fn rows_version(&self) -> (Version, Version, Version, Version) {
        (self.modules.version(), self.snapshot.version(), self.section_edits.version(), self.live.version())
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
            (Some(active), Some(saved)) => !file_ops::case::same(&active.to_string_lossy(), &saved.to_string_lossy()),
            _ => true,
        };
        let moved = saved.migration_source.is_some()
            || saved.mod_library_migration_source.is_some()
            || differs(store.active_game_executable(), &saved.game_executable)
            || differs(store.active_melon_loader_directory(), &saved.melon_loader_directory);
        let turns = self
            .modules
            .iter()
            .filter_map(|module| {
                let on = saved.modules.get(module.choice.id).copied().unwrap_or(module.choice.default_on);
                let active = store.startup_modules().get(module.choice.id).copied().unwrap_or(module.choice.default_on);
                (on != active).then(|| if on { TurnOn } else { TurnOff }.with(&[("module", &module.choice.name)]))
            })
            .reduce(|list, next| And.with(&[("list", &list), ("next", &next)]))
            .unwrap_or_default();
        let kind = if moved || !turns.is_empty() { MessageKind::RestartRequired } else { MessageKind::Saved };
        let message = match (moved, !turns.is_empty()) {
            (true, true) => SavedRestartBoth.with(&[("turns", &turns)]),
            (true, false) => SavedRestartLocations.get().into(),
            (false, true) => SavedRestartTurn.with(&[("turns", &turns)]),
            // A location save says so; a switch save, which shows itself, says nothing.
            (false, false) if self.locations_last => LocationsSaved.get().into(),
            (false, false) => String::new(),
        };
        self.kind = kind;
        self.message = message;
    }

    fn set_error(&mut self, message: &str) {
        self.message = message.into();
        self.kind = MessageKind::Error;
    }

    /// Applies only newer observations. Location acknowledgements never overwrite later typing.
    fn observe(&mut self, snapshot: Snapshot) {
        if snapshot.completed <= self.snapshot.completed {
            // Nothing finished; only what is in flight may have changed.
            if snapshot.submitted != self.snapshot.submitted {
                self.snapshot.submitted = snapshot.submitted;
            }
            return;
        }
        *self.snapshot = snapshot;
        let completed = self.snapshot.completed;
        self.section_edits.retain(|_, (revision, _)| *revision > completed);
        if let Some((revision, edits)) = self.location_save
            && let Some((completed, result)) = &self.snapshot.locations
            && *completed >= revision
        {
            match result {
                Ok(saved) => {
                    let saved = location_text(saved);
                    self.location_error = None;
                    self.external_error = None;
                    for (index, value) in saved.into_iter().enumerate() {
                        if edits[index] == self.edits[index] && self.boxes[index] != value {
                            self.boxes[index] = value;
                        }
                    }
                }
                Err(error) => self.location_error = Some(LocationsNotSaved.with(&[("error", &error)])),
            }
            if self.snapshot.pending_location != Some(revision) {
                self.location_save = None;
            }
        }
        // A completion from an older request cannot reset a newer optimistic switch or its message.
        if self.snapshot.saving() {
            return;
        }
        let desired: Vec<bool> = self
            .modules
            .iter()
            .map(|module| self.snapshot.store.desired().modules.get(module.choice.id).copied().unwrap_or(module.choice.default_on))
            .collect();
        for (module, enabled) in self.modules.iter_mut().zip(desired) {
            module.enabled = enabled;
        }
        match self.snapshot.outcome.clone() {
            SaveOutcome::Persisted => self.describe_saved(),
            // An error the save did not cause, such as the startup one, stays, as `describe_saved` keeps it.
            SaveOutcome::MemoryOnly => match self.location_error.clone().or_else(|| self.external_error.clone()) {
                Some(error) => self.set_error(&error),
                None => {
                    self.message = MemoryOnly.get().into();
                    self.kind = MessageKind::MemoryOnly;
                }
            },
            SaveOutcome::Failed(reason) | SaveOutcome::Faulted(reason) => {
                let message = self.location_error.clone().unwrap_or_else(|| SettingsNotSaved.with(&[("reason", &reason)]));
                self.set_error(&message);
            }
        }
    }

    pub fn rows(&self) -> Vec<ModuleRowView> {
        self.modules
            .iter()
            .map(|module| ModuleRowView {
                title: module.choice.title.get(),
                description: module.choice.description.get(),
                problem: module.choice.problem.clone().unwrap_or_default(),
                enabled: module.enabled,
            })
            .collect()
    }

    pub fn sections(&self) -> Vec<SectionView> {
        let mut sections = Vec::new();
        for (module_index, module) in self.modules.iter().enumerate().filter(|(_, module)| module.enabled_at_start) {
            for (section_index, section) in module.choice.sections.iter().enumerate() {
                sections.push(match section {
                    ModuleSection::Toggle {
                        key,
                        title,
                        label,
                        description,
                        note,
                        default,
                        on_text,
                        off_text,
                    } => SectionView {
                        module: module_index,
                        section: section_index,
                        title: title.get(),
                        description: description.get(),
                        body: SectionBody::Toggle {
                            label: label.get(),
                            note: note.map(Phrase::get).unwrap_or_default(),
                            on_text: on_text.get(),
                            off_text: off_text.get(),
                            // As the module reads it at start, so the switch shows what the app does. An edit on its way
                            // shows over it.
                            checked: self.section_edits.get(&(module_index, section_index)).map_or_else(
                                || crate::settings::module_flag(self.snapshot.store.desired().module_settings.get(module.choice.id), key, *default),
                                |(_, on)| *on,
                            ),
                        },
                    },
                    ModuleSection::Status {
                        title,
                        description,
                        text,
                        action,
                        tip,
                        unavailable,
                    } => {
                        let live = self.live.get(&(module_index, section_index));
                        let available = live.is_some_and(|live| live.can_act);
                        let enabled = available && !live.is_some_and(|live| live.busy);
                        SectionView {
                            module: module_index,
                            section: section_index,
                            title: title.get(),
                            description: description.get(),
                            body: SectionBody::Status {
                                status: live.map_or_else(|| text.get().to_owned(), |live| live.text.clone()),
                                action: action.get(),
                                action_enabled: enabled,
                                // The tip while the action can run; the reason beside it only while it is unavailable, not
                                // while it runs.
                                action_tip: if enabled { tip.get() } else { "" },
                                unavailable: if available { "" } else { unavailable.get() },
                            },
                        }
                    }
                });
            }
        }
        sections
    }

    pub fn locations(&self) -> Vec<LocationView> {
        Location::ALL
            .map(|location| {
                let [label, name, caption, browse, _] = location.words();
                LocationView {
                    label: label.get(),
                    name: name.get(),
                    value: self.location(location).to_owned(),
                    placeholder: self.placeholder(location).to_owned(),
                    caption: caption.get(),
                    browse_label: browse.get(),
                }
            })
            .to_vec()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRowView {
    pub title: &'static str,
    pub description: &'static str,
    pub problem: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionView {
    pub module: usize,
    pub section: usize,
    pub title: &'static str,
    pub description: &'static str,
    pub body: SectionBody,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionBody {
    Toggle {
        label: &'static str,
        note: &'static str,
        on_text: &'static str,
        off_text: &'static str,
        checked: bool,
    },
    Status {
        status: String,
        action: &'static str,
        action_enabled: bool,
        action_tip: &'static str,
        unavailable: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocationView {
    pub label: &'static str,
    pub name: &'static str,
    pub value: String,
    pub placeholder: String,
    pub caption: &'static str,
    pub browse_label: &'static str,
}

fn location_text(saved: &AppSettings) -> [String; 6] {
    Location::ALL.map(|location| location.saved(saved).as_ref().map(|path| path.display().to_string()).unwrap_or_default())
}

impl Feature for SettingsPage {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = spiderland::Never;

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        match intent {
            Intent::ToggleModule(index, on) => {
                let Some(module) = state.modules.get(index) else {
                    return;
                };
                if module.enabled == on {
                    return;
                }
                let save = Save::ModuleEnabled {
                    id: module.choice.id,
                    enabled: on,
                    default: module.choice.default_on,
                };
                state.modules[index].enabled = on;
                state.locations_last = false;
                effects.push(Effect::Submit(save, Submission::Module));
            }
            Intent::ToggleSetting(module_index, section_index, on) => {
                let Some(module) = state.modules.get(module_index) else {
                    return;
                };
                if !module.enabled_at_start || module.choice.problem.is_some() {
                    return;
                }
                let Some(ModuleSection::Toggle { key, .. }) = module.choice.sections.get(section_index) else {
                    return;
                };
                let (id, key) = (module.choice.id, *key);
                state.locations_last = false;
                effects.push(Effect::Switched { module: module_index, key, on });
                effects.push(Effect::Submit(Save::Setting { module: id, key, on }, Submission::Setting(module_index, section_index, on)));
            }
            Intent::Edit(location, text) => {
                state.boxes[location as usize] = text;
                state.edits[location as usize] += 1;
            }
            Intent::Browse(location) => effects.push(Effect::Browse(location)),
            Intent::SaveLocations => {
                state.location_error = None;
                state.submitting_locations = Some(state.edits);
                state.locations_last = true;
                let typed = |location: Location| state.boxes[location as usize].clone();
                let locations = Locations {
                    game_executable: typed(Location::GameExecutable),
                    user_data_directory: typed(Location::UserData),
                    mod_sources_directory: Some(typed(Location::ModSources)),
                    mod_library_directory: Some(typed(Location::ModLibrary)),
                    dotnet_directory: Some(typed(Location::Dotnet)),
                    melon_loader_directory: Some(typed(Location::MelonLoader)),
                };
                effects.push(Effect::Submit(Save::Locations(locations), Submission::Locations));
            }
            Intent::Retry => effects.push(Effect::Submit(Save::Retry, Submission::Retry)),
            Intent::Submitted(submission, result) => {
                let edits = state.submitting_locations.take_if(|_| submission == Submission::Locations);
                match result {
                    Ok(revision) => match (submission, edits) {
                        (Submission::Setting(module, section, on), _) => {
                            state.section_edits.insert((module, section), (revision, on));
                        }
                        (Submission::Locations, Some(edits)) => state.location_save = Some((revision, edits)),
                        _ => {}
                    },
                    Err(reason) => state.set_error(&reason),
                }
            }
            Intent::Observed(snapshot) => {
                state.observe(*snapshot);
                if state.closing {
                    effects.push(Effect::TryPause);
                } else {
                    restart_when_saved(state, effects);
                }
                if matches!(state.snapshot.outcome, SaveOutcome::Faulted(_)) {
                    effects.push(Effect::Quit);
                }
            }
            Intent::LinkChanged => {
                for (module_index, module) in state.modules.iter().enumerate().filter(|(_, module)| module.enabled_at_start) {
                    for (section_index, section) in module.choice.sections.iter().enumerate() {
                        if matches!(section, ModuleSection::Status { .. }) {
                            effects.push(Effect::ReadStatus {
                                module: module_index,
                                section: section_index,
                            });
                        }
                    }
                }
            }
            Intent::Act(module, section) => {
                effects.push(Effect::Act { module, section });
                effects.push(Effect::ReadStatus { module, section });
            }
            Intent::Status(module, section, status) => match status {
                Some(status) if state.live.get(&(module, section)) != Some(&status) => {
                    state.live.insert((module, section), status);
                }
                None if state.live.contains_key(&(module, section)) => {
                    state.live.remove(&(module, section));
                }
                _ => {}
            },
            Intent::ReportError(message) => {
                state.external_error = Some(message.clone());
                state.set_error(&message);
            }
            Intent::Restart => {
                state.restarting = true;
                restart_when_saved(state, effects);
            }
            Intent::CloseRequested => begin_close(state, effects),
            Intent::CloseAbandoned => {
                if std::mem::take(&mut state.restart_requested) {
                    effects.push(Effect::RequestRestart(false));
                }
            }
            Intent::Paused { paused, snapshot } => {
                // Still saving: the save's completion comes back here as `Observed`, which asks again.
                if !paused {
                    state.observe(*snapshot);
                    return;
                }
                // A newly failed save must be shown before closing, even if it finished between the last frame and the
                // click.
                let observed = state.close_observed.take().unwrap_or(snapshot.completed);
                state.observe(*snapshot);
                state.closing = false;
                if state.snapshot.completed > observed && matches!(state.snapshot.outcome, SaveOutcome::Failed(_)) {
                    effects.push(Effect::Resume);
                    effects.push(Effect::ShowPage);
                    // The failure is shown instead; nothing restarts.
                    if std::mem::take(&mut state.restart_requested) {
                        effects.push(Effect::RequestRestart(false));
                    }
                } else {
                    effects.push(Effect::Close);
                }
            }
        }
    }
}

/// Restart now closes the window as any close does, once no save is in flight; the app then starts itself again.
fn restart_when_saved(state: &mut State, effects: &mut Effects<Effect>) {
    if !state.restarting || state.saving() {
        return;
    }
    state.restarting = false;
    state.restart_requested = true;
    effects.push(Effect::RequestRestart(true));
    effects.push(Effect::StartClose);
}

fn begin_close(state: &mut State, effects: &mut Effects<Effect>) {
    state.closing = true;
    let completed = state.snapshot.completed;
    state.close_observed.get_or_insert(completed);
    effects.push(Effect::TryPause);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::settings::SettingsStore;
    use spiderland::step;
    use std::cell::RefCell;
    use std::collections::VecDeque;
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

    /// The feature against a real settings service, performing effects as the window's host does, in the same order.
    struct Page {
        state: State,
        service: SettingsService,
        /// What a linked module was told.
        switched: RefCell<Vec<(String, bool)>>,
        closed: bool,
        shown: bool,
        restart: bool,
    }

    impl Page {
        fn new(service: SettingsService, modules: Vec<ModuleChoice>) -> Self {
            Page {
                state: State::new(service.snapshot(), modules, None, None),
                service,
                switched: RefCell::default(),
                closed: false,
                shown: false,
                restart: false,
            }
        }

        fn send(&mut self, intent: Intent) {
            let mut queue = VecDeque::from([intent]);
            while let Some(intent) = queue.pop_front() {
                for effect in step::<SettingsPage>(&mut self.state, intent).effects {
                    match effect {
                        Effect::Switched { key, on, .. } => self.switched.borrow_mut().push((key.into(), on)),
                        Effect::Submit(save, submission) => queue.extend(submit(&self.service, save, submission)),
                        Effect::StartClose => queue.push_back(Intent::CloseRequested),
                        Effect::TryPause => queue.push_back(try_pause(&self.service)),
                        Effect::Resume => self.service.resume(),
                        Effect::ShowPage => self.shown = true,
                        Effect::Close => self.closed = true,
                        Effect::RequestRestart(on) => self.restart = on,
                        Effect::ReadStatus { .. } | Effect::Act { .. } | Effect::Browse(_) | Effect::Quit => {}
                    }
                }
            }
        }

        fn refresh(&mut self) {
            let snapshot = self.service.snapshot();
            self.send(Intent::Observed(Box::new(snapshot)));
        }

        fn edit(&mut self, location: Location, text: &str) {
            self.send(Intent::Edit(location, text.to_owned()));
        }
    }

    impl std::ops::Deref for Page {
        type Target = State;
        fn deref(&self) -> &State {
            &self.state
        }
    }

    fn page(temp: &Path) -> Page {
        let root = temp.join("app");
        fs::create_dir_all(&root).unwrap();
        let store = SettingsStore::open(&root, &temp.join("settings.json"), &[]);
        Page::new(SettingsService::inline(store), vec![trainer(true)])
    }

    #[test]
    fn switches_reach_the_running_module_as_they_flip_whatever_the_save_does() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("settings.json");
        let store = SettingsStore::open(&temp.path().join("app"), &file, &[]);
        let mut page = Page::new(SettingsService::inline(store), vec![with_sections("trainer", true)]);
        page.send(Intent::ToggleSetting(0, 1, false));
        assert_eq!(*page.switched.borrow(), [("autoConnect".to_owned(), false)]);
        // A save that fails still leaves the session with the choice: the session takes it before the save.
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        page.send(Intent::ToggleSetting(0, 2, true));
        assert_eq!(page.kind(), MessageKind::Error);
        assert_eq!(page.switched.borrow().last(), Some(&("iKnowWhatImDoing".to_owned(), true)));
        // A status section is not a switch.
        page.send(Intent::ToggleSetting(0, 0, true));
        assert_eq!(page.switched.borrow().len(), 2);
    }

    #[test]
    fn a_module_that_did_not_start_hears_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
        let mut page = Page::new(SettingsService::inline(store), vec![with_sections("trainer", false)]);
        page.send(Intent::ToggleSetting(0, 1, false));
        assert!(page.switched.borrow().is_empty());
    }

    #[test]
    fn a_status_section_follows_its_module() {
        let temp = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(&temp.path().join("app"), &temp.path().join("settings.json"), &[]);
        let mut page = Page::new(SettingsService::inline(store), vec![with_sections("trainer", true)]);
        let status = |page: &Page| match &page.sections()[0].body {
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
        let version = page.rows_version();
        page.send(Intent::Status(0, 0, Some(live(true, true))));
        assert_eq!(page.rows_version(), version, "the same status leaves the rows alone");
    }

    #[test]
    fn a_location_save_says_so_and_a_switch_save_says_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let mut page = page(temp.path());
        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        page.edit(Location::ModSources, &sources.display().to_string());
        page.send(Intent::SaveLocations);
        assert_eq!((page.kind(), page.message()), (MessageKind::Saved, "Locations saved."));
        page.send(Intent::ToggleModule(0, false));
        page.send(Intent::ToggleModule(0, true));
        assert_eq!((page.kind(), page.message()), (MessageKind::Saved, ""));
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
        assert!(page.can_restart());
        let (release, waiting) = std::sync::mpsc::channel::<()>();
        service
            .module("fixture")
            .update(move |_| {
                waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                Ok(())
            })
            .unwrap();
        page.refresh();
        assert!(page.saving() && page.can_restart());
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
        assert!(page.module_enabled(0));
        page.send(Intent::ToggleModule(0, false));
        assert_eq!(page.message(), "Saved. Restart Eterra's Gift to turn the trainer off.");
        assert!(page.can_restart());
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
        assert!(!page.can_save_locations());
        page.edit(Location::UserData, "data");
        assert!(page.can_save_locations());
        page.send(Intent::SaveLocations);
        assert_eq!(page.kind(), MessageKind::Error);
        assert!(page.message().starts_with("Locations were not saved: Enter the full path of the user-data folder"));
        let moved = temp.path().join("moved");
        page.edit(Location::UserData, &format!(" {}\\ ", moved.display()));
        page.send(Intent::SaveLocations);
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
        assert_eq!((page.kind(), page.message()), (MessageKind::Saved, ""));
        assert!(page.saving());
        page.edit(Location::UserData, &later.display().to_string());
        // A newer switch is queued too; neither acknowledgement may erase the newer location edit.
        page.send(Intent::ToggleModule(0, false));
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
        page.send(Intent::SaveLocations);
        assert_eq!(page.kind(), MessageKind::Error);
        assert!(page.can_retry());
        fs::remove_dir(&file).unwrap();
        page.send(Intent::Retry);
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
        let mut page = Page::new(service.clone(), vec![with_sections("trainer", true), with_sections("second", true)]);
        assert!(page.module_problem(0).is_some());
        assert!(!page.state.modules[0].enabled_at_start);
        assert!(page.state.modules[1].enabled_at_start);
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
        assert_eq!((page.kind(), page.message()), (MessageKind::Error, startup.as_str()));
        assert!(!page.can_restart());
        // Once nothing else is said, the switch is said to hold for this run only.
        page.state.external_error = None;
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
        page.state.restart_requested = true;
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
        let (rows, boxes) = (page.rows_version(), page.boxes_version());
        page.edit(Location::Dotnet, "typing");
        assert_eq!(page.rows_version(), rows, "a keystroke leaves the module rows alone");
        assert_ne!(page.boxes_version(), boxes);
        let boxes = page.boxes_version();
        page.send(Intent::ToggleModule(0, false));
        assert_ne!(page.rows_version(), rows);
        assert_eq!(page.boxes_version(), boxes, "a switch leaves the boxes alone");
    }
}
