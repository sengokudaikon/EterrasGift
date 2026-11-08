//! The Settings page as a feature: module switches that save the moment they flip, and six locations that wait for "Save
//! locations".
//!
//! Saving is an effect, and each finished save comes back as a snapshot (`Intent::Observed`). Closing the window is a small
//! state machine here too: the pages that go first (Mods) hear of it as an output and answer, then a close waits for
//! accepted saves, and a save that failed in the meantime is shown instead of being lost with the window. It names no module: each one describes itself as a [`ModuleChoice`] (`eg_core::module`).

use eg_core::module::{LiveStatus, ModuleChoice, ModuleSection};
use eg_core::settings::Locations;
use eg_core::settings_service::{Change, SaveOutcome, SettingsService, Snapshot};
use phrases::phrases;
use spiderland::{CloseAnswer, Closing, Effects, Feature, Lifecycle, Tracked, Version};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod location;
mod view;

use location::location_text;
pub use location::{FOLDER_CHECKS, Location};
pub use view::{LocationView, ModuleRowView, SectionBody, SectionView};

phrases! {
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

/// Named for the fault trail (`eg_core::fault::TRAIL`).
#[derive(strum::IntoStaticStr)]
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
    /// The window's close was asked for: the pages that go first finish, and then saves pause.
    CloseRequested,
    /// A page that goes first in the window's close, by its index among them, answered [`Lifecycle::CloseRequested`].
    CloseAnswered(usize, CloseAnswer),
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
    /// Read the saves' snapshot again, answered with [`Intent::Observed`]: a save may have finished before the page began
    /// hearing of them.
    Observe,
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
    /// Pause saves if none is running, and say which.
    TryPause,
    /// Accept saves again: the window stays to show one that failed.
    Resume,
    /// Close the window.
    Close,
    /// End the event loop: the settings writer has faulted.
    Quit,
    /// Ask the app to start itself again once it has closed, or stop asking.
    RequestRestart(bool),
}

/// What the page tells the rest of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Bring the Settings page into view, to show a failed save.
    ShowPage,
    /// What the pages that go first in the window's close hear. [`Lifecycle::CloseRequested`]: they finish their work and
    /// keep what they must, through saves that this page then waits for, before it closes the window.
    /// [`Lifecycle::CloseCalledOff`]: the window stays after they were asked, so they start again what they stopped.
    Lifecycle(Lifecycle),
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
    /// How many pages go first in the window's close.
    close_first: usize,
    /// The window's close waits for the pages that go first to answer.
    asking: Option<Closing>,
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
            close_first: 0,
            asking: None,
            closing: false,
            restarting: false,
            restart_requested: false,
        }
    }

    /// Says how many pages go first in the window's close, each answering [`Output::Lifecycle`] with
    /// [`Intent::CloseAnswered`] before saves pause.
    pub fn set_close_first(&mut self, pages: usize) {
        self.close_first = pages;
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
}

/// What runs while the state wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscription {
    /// Hearing each finished save, whoever asked for it (the trainer saves values of its own), as [`Intent::Observed`]. It
    /// lasts as long as the page.
    Saves,
}

impl Feature for SettingsPage {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = Output;
    type Subscription = Subscription;

    fn subscriptions(_: &State) -> impl IntoIterator<Item = Subscription> {
        [Subscription::Saves]
    }

    /// A save that finished before the page heard of saves, and what linked modules say to begin with.
    fn start(state: &mut State, effects: &mut Effects<Effect>) {
        effects.push(Effect::Observe);
        read_statuses(state, effects);
    }

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect, Output>) {
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
            Intent::LinkChanged => read_statuses(state, effects),
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
            Intent::CloseRequested => request_close(state, effects),
            Intent::CloseAnswered(page, answer) => {
                let Some(answer) = state.asking.as_mut().and_then(|asking| asking.hear(page, answer)) else {
                    return;
                };
                state.asking = None;
                match answer {
                    CloseAnswer::Closed => begin_close(state, effects),
                    // A page that went first has a failure to show, so the window stays and nothing restarts.
                    CloseAnswer::StayingOpen => {
                        if std::mem::take(&mut state.restart_requested) {
                            effects.push(Effect::RequestRestart(false));
                        }
                        effects.emit(Output::Lifecycle(Lifecycle::CloseCalledOff));
                    }
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
                    effects.emit(Output::ShowPage);
                    // The pages that went first closed for nothing.
                    if state.close_first > 0 {
                        effects.emit(Output::Lifecycle(Lifecycle::CloseCalledOff));
                    }
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

/// Asks each started module's status sections what they say now.
fn read_statuses<O>(state: &State, effects: &mut Effects<Effect, O>) {
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

/// Restart now closes the window as any close does, once no save is in flight; the app then starts itself again.
fn restart_when_saved(state: &mut State, effects: &mut Effects<Effect, Output>) {
    if !state.restarting || state.saving() {
        return;
    }
    state.restarting = false;
    state.restart_requested = true;
    effects.push(Effect::RequestRestart(true));
    request_close(state, effects);
}

/// The pages that go first in the window's close hear of it, and saves pause once they have all answered. A close already
/// waiting for them asks no one again.
fn request_close(state: &mut State, effects: &mut Effects<Effect, Output>) {
    if state.asking.is_some() {
        return;
    }
    // While saves are what the close waits for, the pages that went first have answered already.
    if state.close_first == 0 || state.closing {
        begin_close(state, effects);
        return;
    }
    state.asking = Some(Closing::asking(0..state.close_first));
    effects.emit(Output::Lifecycle(Lifecycle::CloseRequested));
}

fn begin_close(state: &mut State, effects: &mut Effects<Effect, Output>) {
    state.closing = true;
    let completed = state.snapshot.completed;
    state.close_observed.get_or_insert(completed);
    effects.push(Effect::TryPause);
}

mod tests;
