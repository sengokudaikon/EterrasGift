//! The Mods page as a feature. Interactive drafts live in its state; every library read and write goes through one ordered
//! worker as an [`Effect::Enqueue`], and each answer comes back as an intent, handled strictly in the order asked. The
//! page reads the disk, runs the compiler and starts the game only through effects.

use super::{
    inventory::{self, Inventory, ModReport},
    library::{InstalledMod, ModLibrary},
    page::{by_hand, follow_preparation, installed_message, keep_each, running_state},
    preferences::ModSetting,
    sources::{ModBuild, ModProject, SourceError, SourceSnapshot},
};
use crate::launch::host::LaunchMode;
use crate::launch::launcher::{GameLauncher, LaunchReport, StartingWithMods};
use eg_core::settings_service::SettingsService;
use melon_loader::assembly::MelonAssembly;
use melon_loader::loader_state::LoaderPreparation;
use melon_loader::preferences::PreferenceKind;
use phrases::phrases;
use serde_json::Value as JsonValue;
use spiderland::{CloseAnswer, Effects, Error, Feature, Tracked, Version};
use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};

phrases! {
    KeptFailed: "The change could not be kept: {error}"
    Reading: "Reading the mods…"
    OpenFolder: "Open the Mods folder"
    NoGame: "Last Epoch wasn't found. Choose the game executable in Settings."
    OpenLog: "Open MelonLoader's log from the last game with mods"
    NoLog: "No log yet. MelonLoader writes one when you play with mods."
    ChangesApply: "Changes apply the next time you play with mods."
    NoVersion: "No version"
    DidNotLoad: "Didn't load"
    CantLoad: "Can't load"
    Version: "Version {version}"
    VersionBy: "Version {version} by {author}"
    NoVersionBy: "No version by {author}"
    DidNotLoadLastTime: "Didn't load last time. {problem}"
    Building: "Building…"
    Update: "Update"
    Rebuild: "Rebuild"
    InstallVersion: "Install {version}"
    InstallBuild: "Install build"
    FromSources: "From sources in {folder}"
    BuildNewer: "Build the newer version and install it"
    BuildFromProject: "Build it from its project and install it"
    UpdateAvailable: "Update available."
    NotBuilt: "Not built yet."
    LatestNotInstalled: "The latest build isn't installed."
    LatestNotInstalledChanged: "The latest build isn't installed. Its sources changed since."
    LatestVersionNotInstalled: "The latest build ({version}) isn't installed."
    LatestVersionNotInstalledChanged: "The latest build ({version}) isn't installed. Its sources changed since."
    SourcesChanged: "Its sources changed since this build."
    OtherSettings: "Other settings"
    Repeated: "Repeated {count} times, last at {time}"
    LibraryStopped: "The mod library stopped. Restart the app to use it again."
    NoSdk: "Building mods from their sources needs the .NET SDK, which was not found. Install it, or choose its folder in Settings, to see and build the mod projects here."
    SourcesUnreadable: "The mod sources in {folder} could not be read: {error}"
    Removed: "{name} is removed. Its DLL leaves the Mods folder the next time you play with mods; its settings stay in MelonPreferences.cfg."
    Added: "Added {name}. It loads the next time you play with mods."
    BuildNeedsGame: "{name} is built against the game's MelonLoader, and Last Epoch was not found. Choose the game executable in Settings."
    BuildingName: "Building {name}…"
    StoppedUnexpectedly: "The start stopped unexpectedly. Details are in the fault log."
    BuildStopped: "The build stopped unexpectedly. Details are in the fault log."
    ReadStopped: "The mod sources could not be read. Details are in the fault log."
    OpenStopped: "That could not be opened. Details are in the fault log."
}

/// The Mods page as a feature: see the module's documentation.
pub struct Mods;

type SettingKey = (String, String);

/// What the project rows read: the sources, the build under way and the folder they are named from.
pub type SourcesKey = (Version, Option<String>, Option<PathBuf>);
/// What the selected mod's pane reads: the library, the sources, the selection, the build under way and the folder.
pub type SelectedKey = (Version, Version, usize, Option<String>, Option<PathBuf>);

/// How a failed edit is said. An edit of the library's own records (on or off, a setting, Remove, Undo, Revert) says the
/// change could not be kept; adding a DLL or installing a build says the failure as it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    Kept,
    Raw,
}

impl Failure {
    fn message(self, error: &str) -> String {
        match self {
            Failure::Kept => KeptFailed.with(&[("error", &error)]),
            Failure::Raw => error.to_owned(),
        }
    }
}

/// A job for the ordered worker: a change to the library, followed by a fresh look at everything.
#[derive(Debug)]
pub enum Change {
    /// Nothing but the fresh look.
    Read,
    SetEnabled {
        file: String,
        enabled: bool,
    },
    Settings(Vec<(String, String, io::Result<Option<JsonValue>>)>),
    Remove {
        file: String,
        name: String,
    },
    Restore {
        item: InstalledMod,
        index: usize,
    },
    Revert {
        root: PathBuf,
        visible: Vec<SettingKey>,
    },
    Add {
        path: PathBuf,
    },
    /// A build to install. `keep_on` keeps an installed mod as it is; `before` is the DLL's hash it replaces.
    InstallBuilt {
        path: PathBuf,
        keep_on: bool,
        before: Option<String>,
    },
}

/// What a change reports back, besides the fresh look.
#[derive(Debug, Default)]
pub struct ChangeResult {
    pub message: Option<String>,
    pub select: Option<String>,
    pub removed: Option<(InstalledMod, usize)>,
}

fn apply(change: Change, library: &ModLibrary) -> io::Result<ChangeResult> {
    match change {
        Change::Read => Ok(ChangeResult::default()),
        Change::SetEnabled { file, enabled } => {
            library.set_enabled(&file, enabled)?;
            Ok(ChangeResult::default())
        }
        Change::Settings(changes) => {
            // One inaccessible key must not discard later independent edits. Retrying a failed batch is idempotent.
            keep_each(changes, |(category, key, value)| value.and_then(|value| library.set_setting(&category, &key, value)))?;
            Ok(ChangeResult::default())
        }
        Change::Remove { file, name } => Ok(ChangeResult {
            removed: library.remove(&file)?,
            message: Some(Removed.with(&[("name", &name)])),
            ..Default::default()
        }),
        Change::Restore { item, index } => {
            library.restore(&item, index)?;
            Ok(ChangeResult {
                select: Some(item.file),
                ..Default::default()
            })
        }
        Change::Revert { root, visible } => {
            library.revert(&root, &visible)?;
            Ok(ChangeResult::default())
        }
        Change::Add { path } => {
            let item = library.add(&path, false)?;
            let name = MelonAssembly::read(&library.path_of(&item))
                .ok()
                .flatten()
                .map_or_else(|| item.file.clone(), |assembly| assembly.name);
            Ok(ChangeResult {
                message: Some(Added.with(&[("name", &name)])),
                select: Some(item.file),
                ..Default::default()
            })
        }
        Change::InstallBuilt { path, keep_on, before } => {
            let item = library.add(&path, keep_on)?;
            let assembly = MelonAssembly::read(&library.path_of(&item)).ok().flatten();
            let name = assembly.as_ref().map(|assembly| assembly.name.as_str()).unwrap_or(&item.file);
            let version = assembly.as_ref().map(|assembly| assembly.version.as_str());
            let message = installed_message(name, version, before.as_ref().is_some_and(|hash| hash.eq_ignore_ascii_case(&item.hash)), item.enabled);
            Ok(ChangeResult {
                message: Some(message),
                select: Some(item.file),
                ..Default::default()
            })
        }
    }
}

/// One coherent look at the library, the game and the settings the page reads.
#[derive(Debug)]
pub struct Observation {
    pub inventory: Inventory,
    pub version: Option<String>,
    pub change: ChangeResult,
    pub running: bool,
    pub modded: bool,
    pub starting: bool,
    pub undo_held: bool,
    pub folder: Option<PathBuf>,
    pub sdk: Option<PathBuf>,
    pub now: jiff::Timestamp,
}

/// Runs on the worker: makes the change, then looks at everything. On a write failure the page keeps its error and asks
/// for a fresh look after all later edits.
pub fn run(change: Change, undo: Option<InstalledMod>, library: &ModLibrary, launcher: &GameLauncher, settings: &SettingsService) -> io::Result<Observation> {
    let change = apply(change, library)?;
    let install = launcher.host().find_install();
    let root = install.as_ref().map(|install| launcher.host().loader_root(install));
    let version = install.as_ref().and_then(|install| launcher.host().read_game_version(install));
    let inventory = inventory::read(install.as_ref(), library, root.as_deref());
    let (running, modded) = running_state(launcher, inventory.loader_log.as_deref());
    let undo_held = undo.as_ref().is_none_or(|item| library.holds(item));
    let snapshot = settings.snapshot();
    Ok(Observation {
        inventory,
        version,
        change,
        running,
        modded,
        starting: launcher.is_starting(),
        undo_held,
        folder: snapshot
            .store
            .saved()
            .mod_sources_directory
            .clone()
            .or_else(|| snapshot.store.paths().mod_sources_directory()),
        sdk: snapshot.store.saved().dotnet_directory.clone(),
        now: jiff::Timestamp::now(),
    })
}

/// How the worker answered.
#[derive(Debug)]
pub enum Answer {
    Observed(Box<Observation>),
    /// The change failed, and later edits are still possible.
    Failed(String),
    /// The worker took no job or stopped: asking again would fail the same way.
    Stopped(String),
}

impl Answer {
    /// The worker's result, or why there is none.
    pub fn of(result: Result<io::Result<Observation>, Error>) -> Answer {
        match result {
            Ok(Ok(observation)) => Answer::Observed(Box::new(observation)),
            Ok(Err(error)) => Answer::Failed(error.to_string()),
            Err(_) => Answer::Stopped(LibraryStopped.into()),
        }
    }

    pub fn stopped(error: &io::Error) -> Answer {
        Answer::Stopped(error.to_string())
    }

    pub fn library_stopped() -> Answer {
        Answer::Stopped(LibraryStopped.into())
    }
}

/// What the page says when the worker has stopped, for work that isn't a job of its own.
pub fn library_stopped_words() -> &'static str {
    LibraryStopped.get()
}

/// Named for the fault trail (`eg_core::fault::TRAIL`).
#[derive(strum::IntoStaticStr)]
pub enum Intent {
    /// Which page came into view: this one, or another.
    PageChanged {
        shown: bool,
        now: jiff::Timestamp,
    },
    /// Time to look at the game again, while the page shows.
    Watch(jiff::Timestamp),
    Answered {
        revision: u64,
        answer: Answer,
    },
    SourcesRead {
        revision: u64,
        result: Result<(Result<SourceSnapshot, SourceError>, bool), Error>,
    },
    Watched {
        generation: u64,
        result: Result<(bool, bool, bool), Error>,
    },
    Select(i32),
    Toggle(i32),
    Refresh,
    CommitSettings,
    SettingEdited(i32, String),
    SettingToggled(i32, bool),
    SettingChoice(i32, String),
    Remove(i32),
    UndoRemove,
    RevertDrafts,
    PickMod,
    Add(PathBuf),
    Reveal(i32),
    Located(Result<PathBuf, String>),
    OpenFolder,
    OpenLog,
    /// How opening a file or folder went.
    Opened(Result<Result<(), String>, Error>),
    /// Something outside the worker failed, in the words of what failed.
    Failed(String),
    Play,
    Launched(Result<LaunchReport, Error>),
    Rebuild(i32),
    InstallBuild(i32),
    BuildSource(i32),
    Built(Result<Result<PathBuf, String>, Error>),
    StopWork,
    CloseRequested,
    /// The window stays after the page closed for it: a build the close stopped starts again.
    CloseCalledOff,
}

#[derive(Debug)]
pub enum Effect {
    Enqueue {
        revision: u64,
        change: Change,
        undo: Option<InstalledMod>,
    },
    ReadSources {
        revision: u64,
        folder: Option<PathBuf>,
        sdk: Option<PathBuf>,
        files: Vec<String>,
    },
    WatchGame {
        generation: u64,
        log: Option<PathBuf>,
    },
    Locate {
        item: InstalledMod,
        folder: Option<PathBuf>,
    },
    Open {
        path: PathBuf,
        reveal: bool,
        create: bool,
    },
    PickMod,
    Launch(LaunchMode),
    Build {
        project: ModProject,
        root: Option<PathBuf>,
        sdk: Option<PathBuf>,
        install_only: bool,
    },
    CancelBuild,
}

/// What the page tells the rest of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Bring the page into view, to show what kept the window open.
    ShowPage,
    /// The answer to [`Intent::CloseRequested`]: nothing is left to wait for, or the page has a failure to show and the window
    /// stays.
    Closed(CloseAnswer),
}

struct Pending {
    revision: u64,
    settings: Vec<SettingKey>,
    clears_build: bool,
    failure: Failure,
    /// The mod whose switch this job flips, shown flipped already.
    toggled: Option<String>,
    answer: Option<Answer>,
}

/// What Install needs to know once a build has finished.
struct BuildJob {
    keep_on: bool,
    before: Option<String>,
}

pub struct State {
    /// The library's worker stopped: nothing more can change or read the library in this run, so the page is read-only.
    stopped: bool,
    inventory: Tracked<Inventory>,
    version: Option<String>,
    sources: Tracked<SourceSnapshot>,
    /// The folder projects are found in, and the .NET SDK's, as the settings last said.
    folder: Option<PathBuf>,
    sdk: Option<PathBuf>,
    now: jiff::Timestamp,
    selection: usize,
    revision: u64,
    action_revision: u64,
    preferred_selection: Option<String>,
    pending: VecDeque<Pending>,
    unsaved: BTreeSet<SettingKey>,
    editing: BTreeSet<SettingKey>,
    removed: Option<(InstalledMod, usize)>,
    message: Option<(String, bool)>,
    read: bool,
    visible: bool,
    running: bool,
    modded_run: bool,
    launching: bool,
    /// The one launcher is starting the game, for this page or another.
    starting: bool,
    building: Option<String>,
    build_job: Option<BuildJob>,
    /// What the build under way was asked as: its row, whether from sources, and whether it only installs.
    build_request: Option<(i32, bool, bool)>,
    /// The build a close stopped, to start again if the close is called off.
    stopped_build: Option<(i32, bool, bool)>,
    sdk_said: bool,
    watch_pending: bool,
    watch_generation: u64,
    follow: u8,
    adopted: Vec<String>,
    deleted: Vec<String>,
    close_error: bool,
    closing: bool,
}

impl State {
    /// The page at `now`, showing from the first or not: `start` reads the library at once if it does.
    pub fn new(now: jiff::Timestamp, shown: bool) -> Self {
        State {
            inventory: Tracked::default(),
            version: None,
            sources: Tracked::default(),
            folder: None,
            sdk: None,
            now,
            selection: 0,
            revision: 0,
            action_revision: 0,
            preferred_selection: None,
            pending: VecDeque::new(),
            unsaved: BTreeSet::new(),
            editing: BTreeSet::new(),
            removed: None,
            message: None,
            stopped: false,
            read: false,
            visible: shown,
            running: false,
            modded_run: false,
            launching: false,
            starting: false,
            building: None,
            build_job: None,
            build_request: None,
            stopped_build: None,
            sdk_said: false,
            watch_pending: false,
            watch_generation: 0,
            follow: 0,
            adopted: Vec::new(),
            deleted: Vec::new(),
            close_error: false,
            closing: false,
        }
    }

    /// What the mod rows read.
    pub fn list_version(&self) -> Version {
        self.inventory.version()
    }

    /// What the project rows read.
    pub fn sources_key(&self) -> SourcesKey {
        (self.sources.version(), self.building.clone(), self.folder.clone())
    }

    /// What the selected mod's pane reads.
    pub fn selected_key(&self) -> SelectedKey {
        (self.inventory.version(), self.sources.version(), self.selection, self.building.clone(), self.folder.clone())
    }

    fn selected(&self) -> Option<&ModReport> {
        self.inventory.mods.get(self.selection)
    }
}

fn holds(keys: &BTreeSet<SettingKey>, setting: &ModSetting) -> bool {
    keys.iter().any(|(category, key)| *category == setting.category && *key == setting.key)
}

/// A failed read of the sources folder, said with the folder.
fn sources_unreadable(folder: Option<&Path>, error: &SourceError) -> String {
    match folder {
        Some(folder) => SourcesUnreadable.with(&[("folder", &folder.display()), ("error", error)]),
        None => error.to_string(),
    }
}

fn say(state: &mut State, text: impl Into<String>, error: bool) {
    state.message = Some((text.into(), error));
    state.removed = None;
    if error && state.closing {
        state.close_error = true;
    }
}

fn enqueue<O>(state: &mut State, effects: &mut Effects<Effect, O>, change: Change, settings: Vec<SettingKey>, clears_build: bool, mutation: bool, failure: Failure) {
    state.revision += 1;
    let revision = state.revision;
    if mutation {
        state.action_revision = revision;
    }
    state.pending.push_back(Pending {
        revision,
        settings,
        clears_build,
        failure,
        toggled: None,
        answer: None,
    });
    effects.push(Effect::Enqueue {
        revision,
        change,
        undo: state.removed.as_ref().map(|(item, _)| item.clone()),
    });
}

fn edit(state: &mut State, effects: &mut Effects<Effect, Output>, change: Change) {
    enqueue(state, effects, change, Vec::new(), false, true, Failure::Kept);
}

fn read_again<O>(state: &mut State, effects: &mut Effects<Effect, O>) {
    enqueue(state, effects, Change::Read, Vec::new(), false, false, Failure::Kept);
}

fn refresh(state: &mut State, effects: &mut Effects<Effect, Output>) {
    commit_settings(state, effects);
    state.editing.clear();
    read_again(state, effects);
}

fn finish_build(state: &mut State) {
    state.building = None;
    state.build_job = None;
    state.build_request = None;
}

fn retain_failed_settings(state: &mut State, settings: Vec<SettingKey>) {
    state.editing.extend(settings.iter().cloned());
    state.unsaved.extend(settings);
}

fn commit_settings(state: &mut State, effects: &mut Effects<Effect, Output>) {
    // Nothing can keep them now; asking would only be refused, and a close would wait on that each time.
    if state.stopped {
        return;
    }
    let unsaved = std::mem::take(&mut state.unsaved);
    if unsaved.is_empty() {
        return;
    }
    let changes: Vec<_> = state
        .inventory
        .mods
        .iter()
        .flat_map(|item| &item.settings)
        .filter(|setting| holds(&unsaved, setting))
        .map(|setting| (setting.category.clone(), setting.key.clone(), setting.draft().map(|value| value.to_json()).transpose()))
        .collect();
    enqueue(state, effects, Change::Settings(changes), unsaved.into_iter().collect(), false, true, Failure::Kept);
}

fn edit_setting(state: &mut State, index: i32, value: String, toggle: Option<bool>) {
    let selection = state.selection;
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    if state.inventory.mods.get(selection).and_then(|item| item.settings.get(index)).is_none() {
        return;
    }
    let setting = &mut state.inventory.mods[selection].settings[index];
    if let Some(on) = toggle {
        setting.is_on = on;
    } else {
        setting.value = value;
    }
    let key = (setting.category.clone(), setting.key.clone());
    state.unsaved.insert(key.clone());
    state.editing.insert(key);
    state.message = None;
    state.removed = None;
}

/// Handles the worker's answers strictly in the order asked: an answer waits for those before it.
fn settle(state: &mut State, effects: &mut Effects<Effect, Output>) {
    let mut read = false;
    while state.pending.front().is_some_and(|job| job.answer.is_some()) {
        let Some(job) = state.pending.pop_front() else {
            break;
        };
        let revision = job.revision;
        if job.clears_build {
            finish_build(state);
        }
        match job.answer.expect("the loop's condition") {
            Answer::Stopped(error) => {
                let said = std::mem::replace(&mut state.stopped, true);
                // Nothing will read the library again to put this right: the switch goes back to what the library holds.
                if let Some(item) = job.toggled.and_then(|file| state.inventory.mods.iter_mut().find(|item| item.installed.file == file)) {
                    item.installed.enabled = !item.installed.enabled;
                }
                retain_failed_settings(state, job.settings);
                // Once is enough for a closing page: saying it again would hold the window open for what can't change.
                if !(said && state.closing) {
                    say(state, job.failure.message(&error), true);
                }
            }
            Answer::Failed(error) => {
                retain_failed_settings(state, job.settings);
                say(state, job.failure.message(&error), true);
                read = true;
            }
            Answer::Observed(observation) => take_observation(state, effects, revision, *observation),
        }
    }
    if read {
        read_again(state, effects);
    }
    if state.closing {
        finish_close(state, effects);
    }
}

fn take_observation(state: &mut State, effects: &mut Effects<Effect, Output>, revision: u64, observation: Observation) {
    // What any read found changed in the Mods folder is said once, by the read that is shown.
    state.adopted.extend(observation.inventory.changes.adopted.iter().cloned());
    state.deleted.extend(observation.inventory.changes.removed.iter().cloned());
    // A later read invalidates an old snapshot, not the action's receipt or its Undo offer.
    if revision >= state.action_revision {
        if let Some(message) = observation.change.message {
            say(state, message, false);
        }
        if let Some(removed) = observation.change.removed {
            state.removed = Some(removed);
        }
        if let Some(select) = observation.change.select {
            state.preferred_selection = Some(select);
        }
    }
    if revision != state.revision {
        return;
    }
    state.watch_generation += 1;
    state.running = observation.running;
    state.modded_run = observation.modded;
    state.starting = observation.starting;
    state.folder = observation.folder;
    state.sdk = observation.sdk;
    state.now = observation.now;
    if !observation.undo_held {
        state.removed = None;
    }
    let selected = state.preferred_selection.take().or_else(|| state.selected().map(|item| item.installed.file.clone()));
    let mut inventory = observation.inventory;
    if inventory.error.is_some() && !state.unsaved.is_empty() {
        // A failed profile read must not erase the only editable copy of a failed save.
        inventory.mods = state.inventory.mods.clone();
    }
    // Preserve text that is still being typed; it has not been submitted to the library yet.
    for item in &mut inventory.mods {
        for setting in &mut item.settings {
            if holds(&state.editing, setting)
                && let Some(old) = state
                    .inventory
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
    state.selection = selected
        .and_then(|file| inventory.mods.iter().position(|item| file_ops::case::same(&item.installed.file, &file)))
        .unwrap_or(0);
    let adopted = std::mem::take(&mut state.adopted);
    let deleted = std::mem::take(&mut state.deleted);
    if let Some(changed) = by_hand(&adopted, &deleted) {
        say(state, changed, false);
    } else if let Some(problem) = &inventory.settings_problem {
        say(state, problem, true);
    }
    *state.inventory = inventory;
    state.version = observation.version;
    state.read = true;
    effects.push(Effect::ReadSources {
        revision,
        folder: state.folder.clone(),
        sdk: state.sdk.clone(),
        files: state.inventory.mods.iter().map(|item| item.installed.file.clone()).collect(),
    });
}

/// With nothing left to wait for, a close goes ahead, unless what it waited for failed: then the page shows that first.
fn finish_close(state: &mut State, effects: &mut Effects<Effect, Output>) {
    if !state.pending.is_empty() || state.building.is_some() {
        return;
    }
    state.closing = false;
    if std::mem::take(&mut state.close_error) {
        effects.emit(Output::ShowPage);
        effects.emit(Output::Closed(CloseAnswer::StayingOpen));
    } else {
        effects.emit(Output::Closed(CloseAnswer::Closed));
    }
}

fn build(state: &mut State, effects: &mut Effects<Effect, Output>, index: i32, source: bool, install_only: bool) {
    if state.building.is_some() {
        return;
    }
    let request = (index, source, install_only);
    commit_settings(state, effects);
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    // A project not installed yet goes by its own name; a rebuild by the installed mod's, as its row shows it.
    let project = if source {
        state.sources.not_installed.get(index).cloned().map(|project| (project.name.clone(), project))
    } else {
        state.inventory.mods.get(index).and_then(|item| {
            state
                .sources
                .builds
                .get(&file_ops::case::key(&item.installed.file))
                .map(|build| (item.name.clone(), build.project.clone()))
        })
    };
    let Some((name, project)) = project else {
        return;
    };
    let root = state.inventory.mods_folder.as_ref().and_then(|folder| folder.parent()).map(PathBuf::from);
    if root.is_none() && !install_only {
        say(state, BuildNeedsGame.with(&[("name", &name)]), true);
        return;
    }
    let before = if source {
        None
    } else {
        state.inventory.mods.get(index).map(|item| item.installed.hash.clone())
    };
    state.building = Some(project.file());
    state.build_job = Some(BuildJob { keep_on: !source, before });
    state.build_request = Some(request);
    // An install only reads the mods again; a build says what it builds while the compiler runs.
    if !install_only {
        say(state, BuildingName.with(&[("name", &name)]), false);
    }
    effects.push(Effect::Build {
        project,
        root,
        sdk: state.sdk.clone(),
        install_only,
    });
}

/// What runs while the state wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscription {
    /// A tick every [`GameLauncher::WATCH_INTERVAL`](crate::launch::launcher::GameLauncher::WATCH_INTERVAL), sending
    /// [`Intent::Watch`]. The page looks at the game only while it shows.
    Tick,
}

impl Feature for Mods {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = Output;
    type Subscription = Subscription;

    fn subscriptions(state: &State) -> impl IntoIterator<Item = Subscription> {
        state.visible.then_some(Subscription::Tick)
    }

    fn start(state: &mut State, effects: &mut Effects<Effect>) {
        if state.visible {
            read_again(state, effects);
        }
    }

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect, Output>) {
        match intent {
            Intent::PageChanged { shown, now } => {
                state.now = now;
                commit_settings(state, effects);
                state.visible = shown;
                if shown {
                    refresh(state, effects);
                }
            }
            Intent::Watch(now) => {
                state.now = now;
                if state.visible && !state.watch_pending {
                    state.watch_pending = true;
                    state.watch_generation += 1;
                    effects.push(Effect::WatchGame {
                        generation: state.watch_generation,
                        log: state.inventory.loader_log.clone(),
                    });
                }
            }
            Intent::Answered { revision, answer } => {
                if let Some(job) = state.pending.iter_mut().find(|job| job.revision == revision) {
                    job.answer = Some(answer);
                }
                settle(state, effects);
            }
            Intent::SourcesRead { revision, result } => {
                if state.revision != revision || state.closing {
                    return;
                }
                match result {
                    Ok((Ok(sources), sdk_missing)) => {
                        *state.sources = sources;
                        if sdk_missing && !std::mem::replace(&mut state.sdk_said, true) {
                            say(state, NoSdk.get(), true);
                        }
                    }
                    // A read cut short by the app's close says nothing.
                    Ok((Err(SourceError::Cancelled), _)) => {}
                    Ok((Err(error), _)) => {
                        *state.sources = SourceSnapshot::default();
                        let said = sources_unreadable(state.folder.as_deref(), &error);
                        say(state, said, true);
                    }
                    Err(_) => say(state, ReadStopped.get(), true),
                }
            }
            Intent::Watched { generation, result } => {
                state.watch_pending = false;
                let Ok((running, modded, starting)) = result else {
                    return;
                };
                if generation != state.watch_generation || state.closing {
                    return;
                }
                let was_running = std::mem::replace(&mut state.running, running);
                state.modded_run = modded;
                state.starting = starting;
                let preparing = state.inventory.loader.as_ref().is_some_and(|loader| loader.preparation == LoaderPreparation::WillPrepare);
                let typing = state
                    .inventory
                    .mods
                    .iter()
                    .flat_map(|item| &item.settings)
                    .any(|setting| setting.kind != PreferenceKind::Switch && !setting.is_choice() && holds(&state.unsaved, setting));
                let (again, follow) = follow_preparation(was_running, running, modded, preparing, typing, state.follow);
                // Do not pile up full inventory reads behind a slow source copy or profile write.
                if again && state.pending.is_empty() {
                    state.follow = follow;
                    refresh(state, effects);
                } else if !again {
                    state.follow = follow;
                }
            }
            Intent::Select(index) => {
                let Ok(index) = usize::try_from(index) else {
                    return;
                };
                if index >= state.inventory.mods.len() {
                    return;
                }
                commit_settings(state, effects);
                state.selection = index;
            }
            Intent::Toggle(index) => {
                commit_settings(state, effects);
                let Some(index) = usize::try_from(index).ok().filter(|index| *index < state.inventory.mods.len()) else {
                    return;
                };
                let item = &mut state.inventory.mods[index];
                item.installed.enabled = !item.installed.enabled;
                let (file, enabled) = (item.installed.file.clone(), item.installed.enabled);
                state.message = None;
                state.removed = None;
                edit(state, effects, Change::SetEnabled { file: file.clone(), enabled });
                if let Some(job) = state.pending.back_mut() {
                    job.toggled = Some(file);
                }
            }
            Intent::Refresh => refresh(state, effects),
            Intent::CommitSettings => commit_settings(state, effects),
            Intent::SettingEdited(index, value) => edit_setting(state, index, value, None),
            Intent::SettingToggled(index, on) => {
                edit_setting(state, index, String::new(), Some(on));
                commit_settings(state, effects);
            }
            Intent::SettingChoice(index, value) => {
                edit_setting(state, index, value, None);
                commit_settings(state, effects);
            }
            Intent::Remove(index) => {
                commit_settings(state, effects);
                let Some(index) = usize::try_from(index).ok().filter(|index| *index < state.inventory.mods.len()) else {
                    return;
                };
                let (file, name) = {
                    let item = &state.inventory.mods[index];
                    (item.installed.file.clone(), item.name.clone())
                };
                // The selection stays where the removed row was: on the next mod, or on the one before at the end.
                let next = state
                    .inventory
                    .mods
                    .get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|before| state.inventory.mods.get(before)))
                    .map(|item| item.installed.file.clone());
                state.preferred_selection = next;
                edit(state, effects, Change::Remove { file, name });
            }
            Intent::UndoRemove => {
                if let Some((item, index)) = state.removed.take() {
                    // Put back and selected again; the removal's message goes with its Undo.
                    state.message = None;
                    edit(state, effects, Change::Restore { item, index });
                }
            }
            Intent::RevertDrafts => {
                state.unsaved.clear();
                state.editing.clear();
                // The drafts going is the latest event: it takes the message and the removal's Undo with it.
                if state.message.take().is_some() {
                    state.removed = None;
                }
                let Some(root) = state.inventory.mods_folder.as_ref().and_then(|folder| folder.parent()).map(PathBuf::from) else {
                    return;
                };
                let visible = state
                    .inventory
                    .mods
                    .iter()
                    .flat_map(|item| &item.settings)
                    .map(|setting| (setting.category.clone(), setting.key.clone()))
                    .collect();
                edit(state, effects, Change::Revert { root, visible });
            }
            Intent::PickMod if state.stopped => say(state, LibraryStopped.get(), true),
            Intent::PickMod => effects.push(Effect::PickMod),
            Intent::Add(path) => {
                commit_settings(state, effects);
                enqueue(state, effects, Change::Add { path }, Vec::new(), false, true, Failure::Raw);
            }
            Intent::Reveal(index) => {
                let Some(item) = usize::try_from(index).ok().and_then(|index| state.inventory.mods.get(index)) else {
                    return;
                };
                effects.push(Effect::Locate {
                    item: item.installed.clone(),
                    folder: state.inventory.mods_folder.clone(),
                });
            }
            Intent::Located(result) => match result {
                Ok(path) => effects.push(Effect::Open {
                    path,
                    reveal: true,
                    create: false,
                }),
                Err(error) => say(state, error, true),
            },
            Intent::OpenFolder => {
                if let Some(path) = state.inventory.mods_folder.clone() {
                    effects.push(Effect::Open {
                        path,
                        reveal: false,
                        create: true,
                    });
                }
            }
            Intent::OpenLog => {
                if let Some(path) = state.inventory.loader_log.clone() {
                    effects.push(Effect::Open {
                        path,
                        reveal: false,
                        create: false,
                    });
                }
            }
            Intent::Opened(result) => match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => say(state, error, true),
                Err(_) => say(state, OpenStopped.get(), true),
            },
            Intent::Failed(error) => say(state, error, true),
            Intent::Play => {
                if state.launching || state.running {
                    return;
                }
                commit_settings(state, effects);
                state.launching = true;
                state.starting = true;
                say(state, StartingWithMods.get(), false);
                // Admission of every draft above precedes the host's preparation on the same library worker.
                effects.push(Effect::Launch(LaunchMode::OfflineWithMods));
            }
            Intent::Launched(result) => {
                state.launching = false;
                state.starting = false;
                refresh(state, effects);
                let report = result.unwrap_or_else(|_| LaunchReport {
                    started: false,
                    message: StoppedUnexpectedly.into(),
                });
                say(state, report.message, !report.started);
            }
            Intent::Rebuild(index) => build(state, effects, index, false, false),
            Intent::InstallBuild(index) => build(state, effects, index, false, true),
            Intent::BuildSource(index) => build(state, effects, index, true, false),
            Intent::Built(result) => {
                let result = result.unwrap_or_else(|_| Err(BuildStopped.into()));
                match result {
                    Ok(path) => {
                        let job = state.build_job.take();
                        let change = Change::InstallBuilt {
                            path,
                            keep_on: job.as_ref().is_some_and(|job| job.keep_on),
                            before: job.and_then(|job| job.before),
                        };
                        enqueue(state, effects, change, Vec::new(), true, true, Failure::Raw);
                    }
                    Err(error) => {
                        finish_build(state);
                        if state.closing {
                            finish_close(state, effects);
                        } else {
                            say(state, error, true);
                        }
                    }
                }
            }
            Intent::StopWork => effects.push(Effect::CancelBuild),
            Intent::CloseRequested => {
                if !std::mem::replace(&mut state.closing, true) {
                    state.stopped_build = state.build_request.filter(|_| state.building.is_some());
                    effects.push(Effect::CancelBuild);
                    commit_settings(state, effects);
                }
                finish_close(state, effects);
            }
            Intent::CloseCalledOff => {
                if let Some((index, source, install_only)) = state.stopped_build.take() {
                    build(state, effects, index, source, install_only);
                }
            }
        }
    }
}

/// A path for a tip, which wraps at words: a zero-width space after each separator lets a long path break between its
/// folders rather than run past the bubble.
fn breakable_path(path: &str) -> String {
    path.replace('\\', "\\\u{200B}").replace('/', "/\u{200B}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModRowView {
    pub name: String,
    pub version: String,
    pub author: String,
    pub enabled: bool,
    pub drafted: bool,
    pub problem: String,
    pub problem_label: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRowView {
    pub name: String,
    pub project: String,
    pub version: String,
    pub folder: String,
    pub busy: bool,
}

/// What the page says whatever is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusView {
    pub readiness: String,
    pub facts: Vec<String>,
    pub warning: bool,
    pub can_play: bool,
    pub is_read: bool,
    pub can_open_folder: bool,
    pub can_open_log: bool,
    pub open_folder_tip: &'static str,
    pub open_log_tip: &'static str,
    pub installed: usize,
    pub enabled: usize,
    pub selected_index: i32,
    pub busy: bool,
    pub building: bool,
    pub draft_count: usize,
    pub can_undo: bool,
    pub change_is_draft: bool,
    pub change_message: String,
    pub change_is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingView {
    pub category: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub group: String,
    pub kind: &'static str,
    pub value: String,
    pub is_on: bool,
    pub unit: String,
    pub drafted: bool,
    pub was: String,
    pub error: String,
    pub choice_label: String,
    pub choice_index: i32,
    /// Each choice's value and label.
    pub choices: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogView {
    pub time: String,
    pub text: String,
    pub error: bool,
    pub count: usize,
    pub count_tip: String,
}

/// The selected mod's pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectedView {
    pub name: String,
    pub detail: String,
    pub file: String,
    pub description: String,
    pub problem: String,
    pub state: &'static str,
    pub enabled: bool,
    pub previous_build: bool,
    pub rebuild_text: &'static str,
    pub rebuild_tip: &'static str,
    pub install_build_text: String,
    pub source: String,
    pub source_path: String,
    pub has_sources: bool,
    pub can_install_build: bool,
    pub build_status: String,
    pub settings: Vec<SettingView>,
    pub log: Vec<LogView>,
}

fn clock(seconds: u64) -> String {
    format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

fn build_status(build: &ModBuild, item: &ModReport, update: bool) -> String {
    if update {
        UpdateAvailable.into()
    } else if !build.is_built() {
        NotBuilt.into()
    } else if !build.is_installed(&item.installed.hash) {
        let version = build.version.as_ref().filter(|version| Some(version.as_str()) != item.version.as_deref());
        match (version, build.changed_since_build) {
            (Some(version), true) => LatestVersionNotInstalledChanged.with(&[("version", version)]),
            (Some(version), false) => LatestVersionNotInstalled.with(&[("version", version)]),
            (None, true) => LatestNotInstalledChanged.into(),
            (None, false) => LatestNotInstalled.into(),
        }
    } else if build.changed_since_build {
        SourcesChanged.into()
    } else {
        String::new()
    }
}

fn settings_of(item: &ModReport) -> Vec<SettingView> {
    let mut group: Option<&str> = None;
    item.settings
        .iter()
        .map(|setting| {
            let next = setting.group.as_deref().or(group.map(|_| OtherSettings.get()));
            let heading = if next == group { "" } else { next.unwrap_or_default() };
            group = next;
            SettingView {
                category: setting.category.clone(),
                key: setting.key.clone(),
                name: setting.name.clone(),
                description: setting.description.clone(),
                group: heading.to_owned(),
                kind: if setting.kind == PreferenceKind::Switch {
                    "switch"
                } else if setting.is_choice() {
                    "choice"
                } else {
                    "text"
                },
                value: setting.value.clone(),
                is_on: setting.is_on,
                unit: setting.unit.clone().unwrap_or_default(),
                drafted: setting.is_drafted(),
                was: setting.was_text(),
                error: setting.error().unwrap_or_default().to_string(),
                choice_label: setting
                    .choices
                    .iter()
                    .find(|choice| choice.value == setting.value)
                    .map_or_else(|| setting.value.clone(), |choice| choice.label.clone()),
                // A value the mod's list doesn't name shows as it is, with no choice in the list held.
                choice_index: setting.choices.iter().position(|choice| choice.value == setting.value).map_or(-1, |index| index as i32),
                choices: setting.choices.iter().map(|choice| (choice.value.clone(), choice.label.clone())).collect(),
            }
        })
        .collect()
}

impl State {
    pub fn status(&self) -> StatusView {
        let inventory = &*self.inventory;
        let readiness = inventory.readiness(self.version.as_deref(), self.running, self.modded_run, self.launching || self.starting, self.now);
        let draft_count = inventory.mods.iter().map(ModReport::draft_count).sum::<usize>();
        let folder_tip = if inventory.mods_folder.is_some() {
            OpenFolder.get()
        } else if self.read {
            NoGame.get()
        } else {
            Reading.get()
        };
        let (change_message, change_is_error) = match &self.message {
            Some((message, error)) => (message.clone(), *error),
            None => (if draft_count > 0 { ChangesApply.get() } else { "" }.to_owned(), false),
        };
        StatusView {
            readiness: if self.read { readiness.text } else { Reading.get().to_owned() },
            facts: if self.read { readiness.facts } else { Vec::new() },
            warning: readiness.warning,
            can_play: self.read && readiness.can_play && !self.closing && !self.stopped,
            is_read: self.read,
            can_open_folder: inventory.mods_folder.is_some(),
            can_open_log: inventory.loader_log.is_some(),
            open_folder_tip: folder_tip,
            open_log_tip: if inventory.loader_log.is_some() {
                OpenLog.get()
            } else if inventory.mods_folder.is_none() {
                folder_tip
            } else {
                NoLog.get()
            },
            installed: inventory.mods.len(),
            enabled: inventory.mods.iter().filter(|item| item.installed.enabled).count(),
            selected_index: if inventory.mods.is_empty() { -1 } else { self.selection as i32 },
            busy: !self.read || self.stopped,
            building: self.building.is_some(),
            draft_count,
            can_undo: self.removed.is_some(),
            change_is_draft: self.message.is_none() && draft_count > 0,
            change_message,
            change_is_error,
        }
    }

    pub fn mod_rows(&self) -> Vec<ModRowView> {
        self.inventory
            .mods
            .iter()
            .map(|item| ModRowView {
                name: item.name.clone(),
                version: item.version.as_deref().unwrap_or(NoVersion.get()).to_owned(),
                author: item.author.clone().unwrap_or_default(),
                enabled: item.installed.enabled,
                drafted: item.draft_count() > 0,
                problem: if item.installed.enabled {
                    item.problem.clone().unwrap_or_default()
                } else {
                    String::new()
                },
                problem_label: if item.installed.enabled && item.problem.is_some() {
                    if item.loaded_last_time == Some(false) { DidNotLoad.get() } else { CantLoad.get() }
                } else {
                    ""
                },
            })
            .collect()
    }

    pub fn source_rows(&self) -> Vec<SourceRowView> {
        self.sources
            .not_installed
            .iter()
            .map(|project| SourceRowView {
                name: project.name.clone(),
                project: breakable_path(&project.project_file.display().to_string()),
                version: project.version.clone().unwrap_or_default(),
                folder: self.folder.as_deref().map(|folder| project.source_folder(folder)).unwrap_or_default(),
                busy: self.building.as_ref().is_some_and(|file| file_ops::case::same(file, &project.file())),
            })
            .collect()
    }

    pub fn selected_view(&self) -> SelectedView {
        let Some(item) = self.selected() else {
            return SelectedView {
                rebuild_text: Rebuild.get(),
                rebuild_tip: BuildFromProject.get(),
                install_build_text: InstallBuild.into(),
                ..SelectedView::default()
            };
        };
        let build = self.sources.builds.get(&file_ops::case::key(&item.installed.file));
        // Its project now has a newer <Version> than the installed DLL was built with: Update builds and installs it.
        let update = build.is_some_and(|build| build.project.update_available(item.project_version.as_deref()));
        SelectedView {
            name: item.name.clone(),
            // The pane's line under the name; the DLL's file name has a line of its own.
            detail: match (&item.version, &item.author) {
                (Some(version), Some(author)) => VersionBy.with(&[("version", version), ("author", author)]),
                (Some(version), None) => Version.with(&[("version", version)]),
                (None, Some(author)) => NoVersionBy.with(&[("author", author)]),
                (None, None) => NoVersion.into(),
            },
            file: item.installed.file.clone(),
            description: item.description.clone().unwrap_or_default(),
            problem: item
                .problem
                .as_ref()
                .map(|problem| {
                    if item.loaded_last_time == Some(false) {
                        DidNotLoadLastTime.with(&[("problem", problem)])
                    } else {
                        problem.clone()
                    }
                })
                .unwrap_or_default(),
            state: item.state_text(),
            enabled: item.installed.enabled,
            previous_build: item.log_from_other_build,
            rebuild_text: if self.building.as_ref().is_some_and(|file| file_ops::case::same(file, &item.installed.file)) {
                Building.get()
            } else if update {
                Update.get()
            } else {
                Rebuild.get()
            },
            rebuild_tip: if update { BuildNewer.get() } else { BuildFromProject.get() },
            install_build_text: build
                .and_then(|build| build.version.as_ref().filter(|version| Some(version.as_str()) != item.version.as_deref()))
                .map_or_else(|| InstallBuild.into(), |version| InstallVersion.with(&[("version", version)])),
            source: build
                .zip(self.folder.as_deref())
                .map(|(build, folder)| FromSources.with(&[("folder", &build.project.source_folder(folder))]))
                .unwrap_or_default(),
            source_path: build.map(|build| breakable_path(&build.project.project_file.display().to_string())).unwrap_or_default(),
            has_sources: build.is_some(),
            can_install_build: build.is_some_and(|build| build.can_install(&item.installed.hash, item.project_version.as_deref())),
            build_status: build.map(|build| build_status(build, item, update)).unwrap_or_default(),
            settings: settings_of(item),
            log: item
                .log
                .iter()
                .map(|run| LogView {
                    time: clock(run.line.time.as_secs()),
                    text: run.line.text.clone(),
                    error: run.line.is_error,
                    count: run.count,
                    count_tip: Repeated.with(&[("count", &run.count), ("time", &clock(run.last.as_secs()))]),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiderland::step;

    fn send(state: &mut State, intent: Intent) -> Vec<Effect> {
        step::<Mods>(state, intent).effects
    }

    fn state() -> State {
        State::new(jiff::Timestamp::UNIX_EPOCH, false)
    }

    #[test]
    fn a_page_that_shows_from_the_first_reads_at_once() {
        assert!(spiderland::start::<Mods>(&mut state()).is_empty());
        let mut shown = State::new(jiff::Timestamp::UNIX_EPOCH, true);
        assert_eq!(revisions(&spiderland::start::<Mods>(&mut shown)).len(), 1);
        assert_eq!(spiderland::subscriptions::<Mods>(&shown), [Subscription::Tick]);
    }

    fn observation() -> Observation {
        Observation {
            inventory: Inventory::default(),
            version: None,
            change: ChangeResult::default(),
            running: false,
            modded: false,
            starting: false,
            undo_held: true,
            folder: None,
            sdk: None,
            now: jiff::Timestamp::UNIX_EPOCH,
        }
    }

    fn observed() -> Answer {
        Answer::Observed(Box::new(observation()))
    }

    fn revisions(effects: &[Effect]) -> Vec<u64> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Enqueue { revision, .. } => Some(*revision),
                _ => None,
            })
            .collect()
    }

    fn shown(state: &mut State) -> u64 {
        let effects = send(
            state,
            Intent::PageChanged {
                shown: true,
                now: jiff::Timestamp::UNIX_EPOCH,
            },
        );
        revisions(&effects)[0]
    }

    #[test]
    fn showing_reads_and_nothing_is_said_until_the_first_answer() {
        let mut state = state();
        let revision = shown(&mut state);
        assert_eq!(spiderland::subscriptions::<Mods>(&state), [Subscription::Tick]);
        assert!(!state.status().is_read && !state.status().can_play);
        let effects = send(&mut state, Intent::Answered { revision, answer: observed() });
        assert!(state.status().is_read);
        assert!(effects.iter().any(|effect| matches!(effect, Effect::ReadSources { .. })), "a read goes on to the sources");
    }

    #[test]
    fn answers_are_handled_in_the_order_asked() {
        let mut state = state();
        let first = shown(&mut state);
        let second = revisions(&send(&mut state, Intent::Refresh))[0];
        // The later answer arrives first, and waits.
        let mut late = observation();
        late.running = true;
        send(
            &mut state,
            Intent::Answered {
                revision: second,
                answer: Answer::Observed(Box::new(late)),
            },
        );
        assert!(!state.status().is_read, "it waits for the one before it");
        send(
            &mut state,
            Intent::Answered {
                revision: first,
                answer: observed(),
            },
        );
        assert!(state.status().is_read);
        assert!(state.running, "the latest read is the one shown");
    }

    #[test]
    fn a_stale_read_keeps_its_actions_receipt_but_not_its_snapshot() {
        let mut state = state();
        let first = shown(&mut state);
        let second = revisions(&send(&mut state, Intent::Refresh))[0];
        let mut stale = observation();
        stale.running = true;
        send(
            &mut state,
            Intent::Answered {
                revision: first,
                answer: Answer::Observed(Box::new(stale)),
            },
        );
        assert!(!state.running, "a read that isn't the latest shows nothing");
        send(
            &mut state,
            Intent::Answered {
                revision: second,
                answer: observed(),
            },
        );
        assert!(state.status().is_read);
    }

    #[test]
    fn a_failed_change_is_said_and_reads_again_but_a_stopped_worker_does_not() {
        let mut state = state();
        let revision = shown(&mut state);
        let effects = send(
            &mut state,
            Intent::Answered {
                revision,
                answer: Answer::Failed("Access is denied.".into()),
            },
        );
        assert_eq!(state.status().change_message, "The change could not be kept: Access is denied.");
        assert_eq!(revisions(&effects).len(), 1, "a fresh read follows a failed change");

        let revision = revisions(&effects)[0];
        let effects = send(
            &mut state,
            Intent::Answered {
                revision,
                answer: Answer::library_stopped(),
            },
        );
        assert!(revisions(&effects).is_empty(), "asking a stopped worker again would never end");
        assert!(state.status().change_is_error);
    }

    #[test]
    fn a_stopped_worker_leaves_the_page_read_only_with_the_switch_as_the_library_holds_it() {
        let mut state = state();
        let revision = shown(&mut state);
        let mut held = observation();
        held.inventory.mods.push(ModReport {
            installed: InstalledMod {
                file: "Beta.dll".into(),
                hash: String::new(),
                enabled: true,
            },
            name: "Beta".into(),
            version: None,
            author: None,
            description: None,
            link: None,
            was_on: true,
            loaded_last_time: None,
            problem: None,
            log: Vec::new(),
            settings: Vec::new(),
            log_from_other_build: false,
            build_waiting: false,
            project_version: None,
        });
        send(
            &mut state,
            Intent::Answered {
                revision,
                answer: Answer::Observed(Box::new(held)),
            },
        );
        assert!(!state.status().busy);

        let effects = send(&mut state, Intent::Toggle(0));
        assert!(!state.mod_rows()[0].enabled, "the switch moves at once");
        send(
            &mut state,
            Intent::Answered {
                revision: revisions(&effects)[0],
                answer: Answer::library_stopped(),
            },
        );
        assert!(state.mod_rows()[0].enabled, "the change never reached the library");
        let status = state.status();
        assert!(status.busy && !status.can_play && status.change_is_error);
        assert!(
            !send(&mut state, Intent::PickMod).iter().any(|effect| matches!(effect, Effect::PickMod)),
            "nothing could add what it picks"
        );
    }

    #[test]
    fn a_stopped_worker_never_holds_the_window_open() {
        let mut state = state();
        let revision = shown(&mut state);
        send(
            &mut state,
            Intent::Answered {
                revision,
                answer: Answer::library_stopped(),
            },
        );
        state.unsaved.insert(("Category".into(), "Key".into()));
        let told = step::<Mods>(&mut state, Intent::CloseRequested);
        assert!(revisions(&told.effects).is_empty(), "a setting nothing can keep isn't asked for again");
        assert_eq!(told.outputs, [Output::Closed(CloseAnswer::Closed)]);
    }

    #[test]
    fn a_library_that_stops_during_a_close_is_said_once_and_the_next_close_goes_ahead() {
        let mut state = state();
        let first = shown(&mut state);
        let second = revisions(&send(&mut state, Intent::Refresh))[0];
        send(&mut state, Intent::CloseRequested);
        send(
            &mut state,
            Intent::Answered {
                revision: first,
                answer: Answer::library_stopped(),
            },
        );
        let told = step::<Mods>(
            &mut state,
            Intent::Answered {
                revision: second,
                answer: Answer::library_stopped(),
            },
        );
        assert_eq!(told.outputs, [Output::ShowPage, Output::Closed(CloseAnswer::StayingOpen)], "the page shows it once");
        assert_eq!(step::<Mods>(&mut state, Intent::CloseRequested).outputs, [Output::Closed(CloseAnswer::Closed)]);
    }

    #[test]
    fn a_failed_settings_save_keeps_its_keys_for_another_try() {
        let mut state = state();
        state.unsaved.insert(("Category".into(), "Key".into()));
        let effects = send(&mut state, Intent::CommitSettings);
        assert!(state.unsaved.is_empty());
        let revision = revisions(&effects)[0];
        send(
            &mut state,
            Intent::Answered {
                revision,
                answer: Answer::Failed("denied".into()),
            },
        );
        assert!(state.unsaved.contains(&("Category".into(), "Key".into())));
        assert!(state.editing.contains(&("Category".into(), "Key".into())));
    }

    #[test]
    fn a_read_of_the_sources_for_an_old_revision_is_dropped() {
        let mut state = state();
        let revision = shown(&mut state);
        send(&mut state, Intent::Refresh);
        send(
            &mut state,
            Intent::SourcesRead {
                revision,
                result: Ok((Err(SourceError::Failed("old".into())), false)),
            },
        );
        assert_eq!(state.status().change_message, "", "only the latest revision's read counts");
    }

    #[test]
    fn a_cancelled_read_says_nothing_and_a_missing_sdk_is_said_once() {
        let mut state = state();
        let revision = shown(&mut state);
        send(
            &mut state,
            Intent::SourcesRead {
                revision,
                result: Ok((Err(SourceError::Cancelled), false)),
            },
        );
        assert_eq!(state.status().change_message, "");
        let read = |sdk_missing| Intent::SourcesRead {
            revision,
            result: Ok((Ok(SourceSnapshot::default()), sdk_missing)),
        };
        send(&mut state, read(true));
        assert!(state.status().change_message.starts_with("Building mods from their sources"));
        state.message = None;
        send(&mut state, read(true));
        assert_eq!(state.status().change_message, "");
    }

    #[test]
    fn looks_at_the_game_never_overlap_and_stop_with_the_page() {
        let mut state = state();
        shown(&mut state);
        let now = jiff::Timestamp::UNIX_EPOCH;
        assert_eq!(send(&mut state, Intent::Watch(now)).len(), 1);
        assert!(send(&mut state, Intent::Watch(now)).is_empty(), "one is still out");
        let generation = state.watch_generation;
        send(
            &mut state,
            Intent::Watched {
                generation,
                result: Ok((true, false, false)),
            },
        );
        assert!(state.running);
        send(&mut state, Intent::PageChanged { shown: false, now });
        assert!(spiderland::subscriptions::<Mods>(&state).is_empty());
        assert!(send(&mut state, Intent::Watch(now)).is_empty(), "a tick already on its way is ignored");
    }

    #[test]
    fn a_look_that_panics_ends_quietly_and_the_next_one_can_go() {
        let mut state = state();
        shown(&mut state);
        let now = jiff::Timestamp::UNIX_EPOCH;
        send(&mut state, Intent::Watch(now));
        let generation = state.watch_generation;
        send(
            &mut state,
            Intent::Watched {
                generation,
                result: Err(Error::Panicked),
            },
        );
        assert_eq!(send(&mut state, Intent::Watch(now)).len(), 1);
    }

    #[test]
    fn a_start_holds_until_it_ends_and_then_reads_again() {
        let mut state = state();
        let revision = shown(&mut state);
        send(&mut state, Intent::Answered { revision, answer: observed() });
        assert!(
            send(&mut state, Intent::Play)
                .iter()
                .any(|effect| matches!(effect, Effect::Launch(LaunchMode::OfflineWithMods)))
        );
        assert!(send(&mut state, Intent::Play).is_empty(), "one start at a time");
        let effects = send(
            &mut state,
            Intent::Launched(Ok(LaunchReport {
                started: false,
                message: "refused".into(),
            })),
        );
        assert_eq!(revisions(&effects).len(), 1);
        let status = state.status();
        assert_eq!((status.change_message.as_str(), status.change_is_error), ("refused", true));
    }

    #[test]
    fn a_start_that_panics_says_so_and_frees_the_page() {
        let mut state = state();
        let revision = shown(&mut state);
        send(&mut state, Intent::Answered { revision, answer: observed() });
        send(&mut state, Intent::Play);
        send(&mut state, Intent::Launched(Err(Error::Panicked)));
        assert!(!state.launching);
        assert_eq!(state.status().change_message, StoppedUnexpectedly.get());
    }

    #[test]
    fn closing_cancels_the_build_and_waits_for_the_worker() {
        let mut state = state();
        let revision = shown(&mut state);
        let told = step::<Mods>(&mut state, Intent::CloseRequested);
        assert!(told.effects.iter().any(|effect| matches!(effect, Effect::CancelBuild)));
        assert!(told.outputs.is_empty(), "a read is still out");
        assert!(!state.status().can_play, "Play goes while the page closes");
        assert_eq!(
            step::<Mods>(&mut state, Intent::Answered { revision, answer: observed() }).outputs,
            [Output::Closed(CloseAnswer::Closed)]
        );
    }

    #[test]
    fn closing_with_nothing_out_goes_ahead_at_once() {
        let mut state = state();
        assert_eq!(step::<Mods>(&mut state, Intent::CloseRequested).outputs, [Output::Closed(CloseAnswer::Closed)]);
    }

    #[test]
    fn a_failure_while_closing_keeps_the_window_and_shows_the_page() {
        let mut state = state();
        let revision = shown(&mut state);
        send(&mut state, Intent::CloseRequested);
        let told = step::<Mods>(
            &mut state,
            Intent::Answered {
                revision,
                answer: Answer::Failed("denied".into()),
            },
        );
        // The failed change reads again first, so the close waits for that read too.
        let reread = revisions(&told.effects)[0];
        assert!(told.outputs.is_empty());
        let told = step::<Mods>(
            &mut state,
            Intent::Answered {
                revision: reread,
                answer: observed(),
            },
        );
        assert_eq!(told.outputs, [Output::ShowPage, Output::Closed(CloseAnswer::StayingOpen)]);
        // The failure was shown: the next close goes ahead.
        assert_eq!(step::<Mods>(&mut state, Intent::CloseRequested).outputs, [Output::Closed(CloseAnswer::Closed)]);
    }

    #[test]
    fn a_build_a_close_stopped_starts_again_when_the_close_is_called_off() {
        let mut state = state();
        state.inventory.mods_folder = Some(PathBuf::from(r"C:\Game\Mods"));
        state.sources.not_installed = vec![ModProject {
            project_file: PathBuf::from(r"C:\Sources\Fixture\Fixture.csproj"),
            target_path: PathBuf::from(r"C:\Sources\Fixture\bin\Fixture.dll"),
            name: "Fixture".into(),
            version: None,
            is_mod: true,
        }];
        let builds = |effects: &[Effect]| effects.iter().filter(|effect| matches!(effect, Effect::Build { .. })).count();
        assert_eq!(builds(&send(&mut state, Intent::BuildSource(0))), 1);
        assert!(step::<Mods>(&mut state, Intent::CloseRequested).outputs.is_empty(), "the compiler is still running");
        assert_eq!(
            step::<Mods>(&mut state, Intent::Built(Ok(Err("cancelled".into())))).outputs,
            [Output::Closed(CloseAnswer::Closed)]
        );
        assert_eq!(builds(&send(&mut state, Intent::CloseCalledOff)), 1, "the build starts again");
        assert!(send(&mut state, Intent::CloseCalledOff).is_empty(), "once");
    }

    #[test]
    fn a_build_cancelled_by_closing_says_nothing_and_lets_the_close_finish() {
        let mut state = state();
        state.building = Some("Fixture.dll".into());
        assert!(step::<Mods>(&mut state, Intent::CloseRequested).outputs.is_empty(), "the compiler is still running");
        assert_eq!(
            step::<Mods>(&mut state, Intent::Built(Ok(Err("cancelled".into())))).outputs,
            [Output::Closed(CloseAnswer::Closed)]
        );
        assert_eq!(state.status().change_message, "");
    }

    #[test]
    fn a_build_that_panics_says_so_and_ends_the_build() {
        let mut state = state();
        state.building = Some("Fixture.dll".into());
        send(&mut state, Intent::Built(Err(Error::Panicked)));
        assert!(!state.status().building);
        assert_eq!(state.status().change_message, BuildStopped.get());
    }

    #[test]
    fn a_finished_build_is_installed_through_the_worker_and_only_then_ends() {
        let mut state = state();
        state.building = Some("Fixture.dll".into());
        state.build_job = Some(BuildJob {
            keep_on: true,
            before: Some("hash".into()),
        });
        let effects = send(&mut state, Intent::Built(Ok(Ok(PathBuf::from("Fixture.dll")))));
        let [Effect::Enqueue { revision, change, .. }] = &effects[..] else {
            panic!("one install expected, not {effects:?}");
        };
        assert!(matches!(
            change,
            Change::InstallBuilt {
                keep_on: true,
                before: Some(_),
                ..
            }
        ));
        assert!(state.status().building, "the build isn't over until it's installed");
        send(
            &mut state,
            Intent::Answered {
                revision: *revision,
                answer: observed(),
            },
        );
        assert!(!state.status().building);
    }

    #[test]
    fn a_keystroke_leaves_the_project_rows_alone() {
        let mut state = state();
        let revision = shown(&mut state);
        send(&mut state, Intent::Answered { revision, answer: observed() });
        let sources = state.sources_key();
        let list = state.list_version();
        send(&mut state, Intent::Select(5));
        assert_eq!(state.sources_key(), sources);
        assert_eq!(state.list_version(), list, "selecting past the end changes nothing");
    }

    #[test]
    fn an_unreadable_sources_folder_is_named() {
        let error = SourceError::Failed("Access is denied.".into());
        assert_eq!(
            sources_unreadable(Some(Path::new(r"C:\mods\src")), &error),
            r"The mod sources in C:\mods\src could not be read: Access is denied."
        );
        assert_eq!(sources_unreadable(None, &error), "Access is denied.");
    }

    #[test]
    fn only_edits_of_the_library_say_the_change_could_not_be_kept() {
        assert_eq!(Failure::Kept.message("Access is denied."), "The change could not be kept: Access is denied.");
        assert_eq!(Failure::Raw.message("Access is denied."), "Access is denied.");
    }
}
