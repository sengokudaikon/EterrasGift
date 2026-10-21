//! `settings.json`, schema version 2 (`spec/data-and-persistence.md` §4): the locations the app uses and, for each
//! module, whether it is on and its own settings, which only the module reads. The file is the C# app's format, read
//! case-insensitively and written camelCase, so either app can read what the other wrote.

use crate::atomic;
use crate::paths::{self, AppPaths};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The settings file's version. Version 1 kept a module's settings at the top level; opening it moves them into the
/// module's own slice once.
pub const CURRENT_SCHEMA: i64 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppSettings {
    /// Unrecognized top-level fields survive a save by an older app.
    pub extra: Map<String, Value>,
    /// None finds the game by itself. Takes effect after a restart.
    pub game_executable: Option<PathBuf>,
    /// None is the default user-data folder. Takes effect after a restart, which copies the old folder there.
    pub user_data_directory: Option<PathBuf>,
    /// The user data's old folder while that restart waits.
    pub migration_source: Option<PathBuf>,
    /// The player's own mod projects; None is the default (mods\ in a checkout, none in a release). At once.
    pub mod_sources_directory: Option<PathBuf>,
    /// None is mods\ in the user data. Takes effect after a restart, which copies the library there.
    pub mod_library_directory: Option<PathBuf>,
    /// The library's old folder while that restart waits.
    pub mod_library_migration_source: Option<PathBuf>,
    /// The folder with the dotnet.exe that builds mods; None finds the .NET SDK. At once.
    pub dotnet_directory: Option<PathBuf>,
    /// Where a game with mods runs MelonLoader from; None is the game folder. After a restart.
    pub melon_loader_directory: Option<PathBuf>,
    /// Each module's on/off choice by its id, stored only when it differs from the module's default. After a restart.
    pub modules: BTreeMap<String, bool>,
    /// Each module's own record by its id, as the module writes it.
    pub module_settings: Map<String, Value>,
}

/// The location fields in the file's order, with their JSON names.
const LOCATIONS: [&str; 8] = [
    "gameExecutable",
    "userDataDirectory",
    "migrationSource",
    "modSourcesDirectory",
    "modLibraryDirectory",
    "modLibraryMigrationSource",
    "dotnetDirectory",
    "melonLoaderDirectory",
];

impl AppSettings {
    fn location(&self, index: usize) -> &Option<PathBuf> {
        [
            &self.game_executable,
            &self.user_data_directory,
            &self.migration_source,
            &self.mod_sources_directory,
            &self.mod_library_directory,
            &self.mod_library_migration_source,
            &self.dotnet_directory,
            &self.melon_loader_directory,
        ][index]
    }

    fn location_mut(&mut self, index: usize) -> &mut Option<PathBuf> {
        match index {
            0 => &mut self.game_executable,
            1 => &mut self.user_data_directory,
            2 => &mut self.migration_source,
            3 => &mut self.mod_sources_directory,
            4 => &mut self.mod_library_directory,
            5 => &mut self.mod_library_migration_source,
            6 => &mut self.dotnet_directory,
            _ => &mut self.melon_loader_directory,
        }
    }

    /// Reads the settings from the file's object, matching known keys whatever their case.
    pub fn from_json(document: &Map<String, Value>) -> Result<AppSettings, String> {
        let mut settings = AppSettings::default();
        for (key, value) in document {
            if let Some(index) = LOCATIONS.iter().position(|name| name.eq_ignore_ascii_case(key)) {
                *settings.location_mut(index) = match value {
                    Value::Null => None,
                    Value::String(text) => Some(PathBuf::from(text)),
                    _ => return Err(format!("{} must be text.", LOCATIONS[index])),
                };
            } else if key.eq_ignore_ascii_case("modules") {
                match value {
                    Value::Null => {}
                    Value::Object(choices) => {
                        for (id, on) in choices {
                            let on = on.as_bool().ok_or_else(|| format!("The modules' choice for {id} must be true or false."))?;
                            settings.modules.insert(id.clone(), on);
                        }
                    }
                    _ => return Err("modules must be an object.".into()),
                }
            } else if key.eq_ignore_ascii_case("moduleSettings") {
                match value {
                    Value::Null => {}
                    Value::Object(slices) => settings.module_settings = slices.clone(),
                    _ => return Err("moduleSettings must be an object.".into()),
                }
            } else if !key.eq_ignore_ascii_case("schemaVersion") {
                settings.extra.insert(key.clone(), value.clone());
            }
        }
        Ok(settings)
    }

    /// The file's object, always as the current version; absent values are left out.
    pub fn to_json(&self) -> Map<String, Value> {
        let mut document = self.extra.clone();
        document.insert("schemaVersion".into(), CURRENT_SCHEMA.into());
        for (index, name) in LOCATIONS.iter().enumerate() {
            if let Some(path) = self.location(index) {
                document.insert((*name).into(), path.to_string_lossy().into_owned().into());
            }
        }
        if !self.modules.is_empty() {
            document.insert("modules".into(), Value::Object(self.modules.iter().map(|(id, &on)| (id.clone(), on.into())).collect()));
        }
        if !self.module_settings.is_empty() {
            document.insert("moduleSettings".into(), Value::Object(self.module_settings.clone()));
        }
        document
    }
}

/// Where a module's settings stood in a file from before modules had their own (version 1): its on/off choice and the
/// keys of its settings at the top level. Opening such a file moves them into the module's slice once.
#[derive(Clone, Debug)]
pub struct LegacyModuleSettings {
    pub module_id: &'static str,
    /// The top-level key that held the on/off choice, if there was one.
    pub enabled_key: Option<&'static str>,
    /// The top-level keys that held the module's settings.
    pub keys: &'static [&'static str],
}

/// The locations as the Settings page saves them. Each is a full path, or empty for its default; `None` keeps the saved
/// choice (only the last four may be `None`).
#[derive(Clone, Debug, Default)]
pub struct Locations {
    pub game_executable: String,
    pub user_data_directory: String,
    pub mod_sources_directory: Option<String>,
    pub mod_library_directory: Option<String>,
    pub dotnet_directory: Option<String>,
    pub melon_loader_directory: Option<String>,
}

/// The app's settings as saved, and the saves Settings and the modules make. Saves are made one at a time by whoever
/// holds the store, each applying its change over what the save before it left (`spec/shell-and-settings.md` §10.3).
#[derive(Clone, Debug)]
pub struct SettingsStore {
    file_path: PathBuf,
    saved: AppSettings,
    /// Validated intent retained after an IO failure; it is never reported as saved.
    pending: Option<AppSettings>,
    planning: bool,
    pending_explicit: bool,
    paths: AppPaths,
    active_game_executable: Option<PathBuf>,
    active_melon_loader_directory: Option<PathBuf>,
    startup_modules: BTreeMap<String, bool>,
    startup_error: Option<String>,
    unreadable: bool,
}

impl SettingsStore {
    /// The settings file the app uses: in its local folder.
    pub fn default_file() -> PathBuf {
        paths::local_directory().join("settings.json")
    }

    /// Opens the settings, then runs a user-data or mod-library move that waits on this start. Never fails: a file that
    /// can't be read gives the defaults, a startup error and an unreadable store.
    pub fn open(root: &Path, file_path: &Path, legacy: &[LegacyModuleSettings]) -> SettingsStore {
        let (settings, upgraded, error) = match read(file_path, legacy) {
            Ok((settings, upgraded)) => (settings, upgraded, None),
            Err(reason) => (
                AppSettings::default(),
                false,
                Some(format!(
                    "Settings could not be loaded: {reason} The file is left as it is, so changes last until Eterra's Gift closes; Save locations replaces it."
                )),
            ),
        };
        let mut store = SettingsStore {
            file_path: file_path.to_path_buf(),
            paths: paths::app_paths(
                root,
                settings.migration_source.as_deref().or(settings.user_data_directory.as_deref()),
                settings.mod_library_migration_source.as_deref().or(settings.mod_library_directory.as_deref()),
            ),
            active_game_executable: settings.game_executable.clone(),
            active_melon_loader_directory: settings.melon_loader_directory.clone(),
            startup_modules: settings.modules.clone(),
            unreadable: error.is_some(),
            startup_error: error,
            saved: settings,
            pending: None,
            planning: false,
            pending_explicit: false,
        };
        if upgraded {
            // Read as upgraded all the same when this fails; the next save writes the new shape.
            let _ = store.write(|_| {}, false);
        }
        if let Some(executable) = &store.saved.game_executable
            && !(executable.is_file() && has_extension(executable, "exe"))
        {
            store.startup_error = Some("The configured game executable was not found. Choose another executable or use automatic detection.".into());
        }
        store.run_pending_moves();
        store
    }

    fn run_pending_moves(&mut self) {
        let user_data = self.saved.user_data_directory.clone();
        let library = self.saved.mod_library_directory.clone();
        let library_source = self.saved.mod_library_migration_source.clone();
        if self.saved.migration_source.is_some() {
            let destination = self.paths.with(user_data.as_deref(), None).user_data_directory;
            match migrate(&self.paths.user_data_directory, &destination).and_then(|()| self.write(|saved| saved.migration_source = None, false)) {
                Ok(()) => self.paths = self.paths.with(user_data.as_deref(), library_source.as_deref().or(library.as_deref())),
                Err(error) => {
                    // Failed migration bookkeeping is not user intent. Keep the marker until a later startup
                    // copies the source again, including anything this run still writes there.
                    self.pending = None;
                    self.startup_error = Some(format!("Data migration failed; the previous folder remains active. {error}"));
                }
            }
        }
        // Saved alone, never with a user-data move, so the library's new folder does not depend on one.
        if library_source.is_some() && self.saved.migration_source.is_none() {
            let destination = self.paths.with(Some(&self.paths.user_data_directory.clone()), library.as_deref());
            match migrate(&self.paths.mod_library_directory, &destination.mod_library_directory).and_then(|()| self.write(|saved| saved.mod_library_migration_source = None, false))
            {
                Ok(()) => self.paths = destination,
                Err(error) => {
                    self.pending = None;
                    self.startup_error = Some(format!("The mod library could not be moved; its previous folder remains active. {error}"));
                }
            }
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn saved(&self) -> &AppSettings {
        &self.saved
    }

    pub fn desired(&self) -> &AppSettings {
        self.pending.as_ref().unwrap_or(&self.saved)
    }

    /// None means the file could not be read and the current settings are only in memory.
    pub fn persisted(&self) -> Option<&AppSettings> {
        (!self.unreadable).then_some(&self.saved)
    }

    pub fn has_pending_write(&self) -> bool {
        self.pending.is_some()
    }

    /// Retries local settings persistence only. This is never a game-write retry.
    pub fn retry(&mut self) -> io::Result<()> {
        self.write(|_| {}, false)
    }

    /// Computes and validates a command on a private copy. Used only by the serialized writer.
    pub(crate) fn plan(&self, change: impl FnOnce(&mut Self) -> Result<(), String>) -> Result<AppSettings, String> {
        let mut candidate = self.clone();
        candidate.planning = true;
        change(&mut candidate)?;
        Ok(candidate.desired().clone())
    }

    pub(crate) fn stage(&mut self, desired: AppSettings, explicit: bool) {
        self.pending = Some(desired);
        self.pending_explicit |= explicit;
    }

    pub(crate) fn persist(&mut self, replace_unreadable: bool) -> io::Result<()> {
        self.write(|_| {}, replace_unreadable)
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// The game executable this run uses; a new choice takes effect after a restart.
    pub fn active_game_executable(&self) -> Option<&Path> {
        self.active_game_executable.as_deref()
    }

    /// The MelonLoader folder this run uses (None: the game folder); a new choice takes effect after a restart.
    pub fn active_melon_loader_directory(&self) -> Option<&Path> {
        self.active_melon_loader_directory.as_deref()
    }

    /// The modules' on/off choices as they stood when the app started: this run has those modules.
    pub fn startup_modules(&self) -> &BTreeMap<String, bool> {
        &self.startup_modules
    }

    /// Where the Mods page looks for the player's own mod projects: the saved folder, else the default; None for none.
    pub fn mod_sources_directory(&self) -> Option<PathBuf> {
        self.saved.mod_sources_directory.clone().or_else(|| self.paths.mod_sources_directory())
    }

    pub fn startup_error(&self) -> Option<&str> {
        self.startup_error.as_deref()
    }

    /// The settings file is there but could not be read; nothing is written over it until the locations are saved.
    pub fn unreadable(&self) -> bool {
        self.unreadable
    }

    /// A module's own settings as saved; None when it has saved none.
    pub fn module_settings(&self, id: &str) -> Option<&Value> {
        self.saved.module_settings.get(id)
    }

    /// Saves a module's own settings record. While the file is unreadable it lasts for this run only.
    pub fn save_module_settings(&mut self, id: &str, record: Value) -> io::Result<()> {
        self.write(
            |saved| {
                saved.module_settings.insert(id.to_owned(), record);
            },
            false,
        )
    }

    /// Stores switches alone: the saved locations, and any move they wait on, stay as they are. While the file is
    /// unreadable they are kept for this run only.
    pub fn save_switches(&mut self, change: impl FnOnce(&mut AppSettings)) -> io::Result<()> {
        self.write(change, false)
    }

    /// Stores the locations, checked first. It is the explicit save that may replace a file that could not be read.
    /// The error is the sentence the page shows after "Locations were not saved: ".
    pub fn save_locations(&mut self, locations: &Locations) -> Result<(), String> {
        let executable = normalize(&locations.game_executable, "game executable")?;
        if let Some(executable) = &executable
            && !(executable.is_file() && has_extension(executable, "exe"))
        {
            return Err("Choose an existing game executable (.exe), or use automatic detection.".into());
        }
        let sources = keep_or(&self.desired().mod_sources_directory, &locations.mod_sources_directory, "mod sources folder")?;
        if let Some(sources) = &sources
            && !sources.is_dir()
        {
            return Err(if self.paths.mod_sources_directory().is_none() {
                "Choose an existing folder for mod sources, or leave it empty for none."
            } else {
                "Choose an existing folder for mod sources, or leave it empty for the mods folder in the source checkout."
            }
            .into());
        }
        let dotnet = keep_or(&self.desired().dotnet_directory, &locations.dotnet_directory, ".NET SDK folder")?;
        if let Some(dotnet) = &dotnet
            && !dotnet.join("dotnet.exe").is_file()
        {
            return Err("Choose the folder that holds dotnet.exe, or leave it empty to find the .NET SDK automatically.".into());
        }
        let loader = keep_or(&self.desired().melon_loader_directory, &locations.melon_loader_directory, "MelonLoader folder")?;
        if let Some(loader) = &loader
            && !loader.join("MelonLoader").is_dir()
        {
            return Err("Choose a folder with MelonLoader in it (a copy of the game's MelonLoader folder), or leave it empty for the game folder.".into());
        }
        let directory = normalize(&locations.user_data_directory, "user-data folder")?;
        let library = keep_or(&self.desired().mod_library_directory, &locations.mod_library_directory, "mod library folder")?;
        let destination = self.paths.with(directory.as_deref(), library.as_deref());
        let changed = !paths::same_folder(&self.paths.user_data_directory, &destination.user_data_directory);
        // The library's default folder is in the user data, so it follows a user-data move; a folder of its own moves alone.
        let library_destination = self.paths.with(Some(&self.paths.user_data_directory), library.as_deref());
        let library_moves = !paths::same_folder(&self.paths.mod_library_directory, &library_destination.mod_library_directory);
        if changed && library_moves {
            return Err("Move the user-data folder and the mod library one at a time: save one, restart Eterra's Gift, then change the other.".into());
        }
        // Checked now against everything the move copies, so it cannot fail at the next start and leave the move waiting.
        if changed && let Some(conflict) = migration_conflict(&self.paths.user_data_directory, &destination.user_data_directory).map_err(|e| e.to_string())? {
            return Err(format!(
                "The selected folder already holds {} with other content. Choose an empty user-data folder.",
                relative(&destination.user_data_directory, &conflict)
            ));
        }
        if library_moves && let Some(clash) = migration_conflict(&self.paths.mod_library_directory, &library_destination.mod_library_directory).map_err(|e| e.to_string())? {
            return Err(format!(
                "The selected folder already holds {} with other content. Choose an empty folder for the mod library.",
                relative(&library_destination.mod_library_directory, &clash)
            ));
        }
        probe(&destination.user_data_directory).map_err(|e| e.to_string())?;
        if library_moves {
            probe(&library_destination.mod_library_directory).map_err(|e| e.to_string())?;
        }
        let user_data = directory.is_some().then(|| destination.user_data_directory.clone());
        let migration_source = changed.then(|| self.paths.user_data_directory.clone());
        let library_source = library_moves.then(|| self.paths.mod_library_directory.clone());
        self.write(
            |saved| {
                saved.game_executable = executable;
                saved.user_data_directory = user_data;
                saved.migration_source = migration_source;
                saved.mod_sources_directory = sources;
                saved.mod_library_directory = library;
                saved.mod_library_migration_source = library_source;
                saved.dotnet_directory = dotnet;
                saved.melon_loader_directory = loader;
            },
            true,
        )
        .map_err(|e| e.to_string())
    }

    /// `change` applies to the settings as the saves before it left them, so two saves of different fields both keep
    /// their change. `replace_unreadable` is the explicit save, which may replace a file that could not be read.
    fn write(&mut self, change: impl FnOnce(&mut AppSettings), replace_unreadable: bool) -> io::Result<()> {
        let replace_unreadable = replace_unreadable || self.pending_explicit;
        let mut settings = self.desired().clone();
        change(&mut settings);
        if self.planning {
            self.pending = Some(settings);
            self.pending_explicit = replace_unreadable;
            return Ok(());
        }
        if self.unreadable && !replace_unreadable {
            self.saved = settings;
            self.pending = None;
            self.pending_explicit = false;
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&Value::Object(settings.to_json())).map_err(io::Error::other)?;
        if let Err(error) = atomic::write(&self.file_path, text.as_bytes()) {
            self.pending = Some(settings);
            self.pending_explicit = replace_unreadable;
            return Err(io::Error::new(error.kind(), describe_io(&error, &self.file_path)));
        }
        self.saved = settings;
        self.pending = None;
        self.pending_explicit = false;
        self.unreadable = false;
        Ok(())
    }
}

/// An IO failure on `path` as .NET words it (`Win32Marshal`), the way the C# page shows it: "Access to the path '…' is
/// denied." rather than Rust's "Access is denied. (os error 5)".
pub fn describe_io(error: &io::Error, path: &Path) -> String {
    let path = path.display();
    match error.raw_os_error() {
        Some(2) => format!("Could not find file '{path}'."),
        Some(3) => format!("Could not find a part of the path '{path}'."),
        Some(5) => format!("Access to the path '{path}' is denied."),
        Some(32) => format!("The process cannot access the file '{path}' because it is being used by another process."),
        Some(80 | 183) => format!("The file '{path}' already exists."),
        Some(206) => format!("The path '{path}' is too long, or a component of the specified path is too long."),
        _ => {
            let text = error.to_string();
            let text = text.split(" (os error ").next().unwrap_or(&text).trim_end();
            format!("{text} : '{path}'")
        }
    }
}

/// The settings in the file, and whether they were upgraded from version 1. A missing file is the defaults.
fn read(file_path: &Path, legacy: &[LegacyModuleSettings]) -> Result<(AppSettings, bool), String> {
    let text = match fs::read(file_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((AppSettings::default(), false)),
        Err(error) => return Err(error.to_string()),
    };
    let text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&text);
    let mut document = match serde_json::from_slice::<Value>(text) {
        Ok(Value::Object(document)) => document,
        Ok(_) => return Err("The settings file is empty.".into()),
        Err(error) => return Err(format!("The settings file is not valid JSON ({error}).")),
    };
    let version = find(&document, "schemaVersion").and_then(Value::as_i64).unwrap_or(0);
    let upgraded = version == 1;
    if upgraded {
        upgrade(&mut document, legacy);
    } else if version != CURRENT_SCHEMA {
        return Err("Unsupported settings version.".into());
    }
    let settings = AppSettings::from_json(&document)?;
    for index in 0..LOCATIONS.len() {
        if let Some(path) = settings.location(index)
            && (!paths::is_fully_qualified(path) || std::path::absolute(path).ok().as_deref() != Some(path.as_path()))
        {
            return Err("Settings paths must be absolute and normalized.".into());
        }
    }
    Ok((settings, upgraded))
}

/// Moves a version 1 file's module keys into the modules' slices: their on/off choice and their own settings.
fn upgrade(document: &mut Map<String, Value>, legacy: &[LegacyModuleSettings]) {
    let mut modules = Map::new();
    let mut slices = Map::new();
    for module in legacy {
        if let Some(Value::Bool(on)) = module.enabled_key.and_then(|key| take(document, key)) {
            modules.insert(module.module_id.into(), on.into());
        }
        let slice: Map<String, Value> = module.keys.iter().filter_map(|&key| take(document, key).map(|value| (key.to_owned(), value))).collect();
        if !slice.is_empty() {
            slices.insert(module.module_id.into(), Value::Object(slice));
        }
    }
    take(document, "schemaVersion");
    document.insert("schemaVersion".into(), CURRENT_SCHEMA.into());
    if !modules.is_empty() {
        document.insert("modules".into(), Value::Object(modules));
    }
    if !slices.is_empty() {
        document.insert("moduleSettings".into(), Value::Object(slices));
    }
}

/// A module's switch in its settings record (`moduleSettings.<id>`), matched whatever its case, as the C# app reads it
/// and as people type it by hand: `default` when the record does not say. A record that is unclear, its spellings
/// disagreeing or the value not true or false, reads as off: for every module switch off is the cautious side (the
/// trainer then connects only when asked, and writes only what the game would make). Settings shows and the app uses
/// this one reading, so the two cannot disagree.
pub fn module_flag(record: Option<&Value>, key: &str, default: bool) -> bool {
    let Some(fields) = record.and_then(Value::as_object) else {
        return default;
    };
    let mut values = fields.iter().filter(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value.as_bool());
    let Some(first) = values.next() else {
        return default;
    };
    match first {
        Some(on) if values.all(|other| other == Some(on)) => on,
        _ => false,
    }
}

/// A top-level key's value, matched whatever its case.
fn find<'a>(document: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    document.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value)
}

/// Removes a top-level key, matched whatever its case, and returns its value.
fn take(document: &mut Map<String, Value>, key: &str) -> Option<Value> {
    let name = document.keys().find(|name| name.eq_ignore_ascii_case(key))?.clone();
    document.remove(&name)
}

/// A typed location as the settings keep it: a full path without a trailing separator; None when the box is empty.
fn normalize(typed: &str, what: &str) -> Result<Option<PathBuf>, String> {
    let trimmed = typed.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !paths::is_fully_qualified(Path::new(trimmed)) {
        return Err(format!("Enter the full path of the {what}, starting with its drive (such as C:\\), or use Browse."));
    }
    Ok(Some(paths::normalized(Path::new(trimmed))))
}

/// A location the save may leave as saved (`None`), or the typed one.
fn keep_or(saved: &Option<PathBuf>, typed: &Option<String>, what: &str) -> Result<Option<PathBuf>, String> {
    match typed {
        None => Ok(saved.clone()),
        Some(typed) => normalize(typed, what),
    }
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).to_string_lossy().into_owned()
}

/// Makes sure a folder a location moves to can be written, before anything waits on it.
fn probe(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let probe = directory.join(format!(".write-test-{}", uuid::Uuid::new_v4().simple()));
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}

/// What a move copied into its destination, by path within it and content hash. A move that fails partway is taken up
/// again at the next start, while the old folder stays in use and changes: it may then replace its own earlier copies,
/// still as it made them, but never a file that was there before or that changed after it was copied. Removed once the
/// move is done.
const MOVE_RECORD: &str = ".eterras-gift-move.json";

/// Copies a folder's files into its new place, leaving the old ones as they are (`spec/data-and-persistence.md` §4.4).
fn migrate(source: &Path, destination: &Path) -> io::Result<()> {
    if paths::same_folder(source, destination) {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    let record_path = destination.join(MOVE_RECORD);
    let mut record = move_record(destination);
    let mut copies = Vec::new();
    for (from, to) in migration_copies(source, destination)? {
        if to.exists() {
            if same_content(&from, &to)? {
                continue;
            }
            if !copied_before(&record, destination, &to)? {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("Destination file already exists: {}", to.display())));
            }
        }
        copies.push((from, to));
    }
    // Recorded before anything is copied, so a copy this run leaves behind is known as its own.
    for (from, to) in &copies {
        record.insert(relative(destination, to), crate::hashes::of_reader(fs::File::open(from)?)?);
    }
    if !copies.is_empty() {
        atomic::write(&record_path, &serde_json::to_vec_pretty(&record).map_err(io::Error::other)?)?;
    }
    for (from, to) in copies {
        if to.exists() {
            atomic::copy(&from, &to)?;
        } else {
            atomic::copy_new(&from, &to)?;
        }
    }
    // A record left behind only lets a later move into this folder replace files that are still this move's copies.
    let _ = fs::remove_file(&record_path);
    Ok(())
}

fn move_record(destination: &Path) -> BTreeMap<String, String> {
    fs::read(destination.join(MOVE_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Whether `to` is a copy an earlier, unfinished move made, still as it made it.
fn copied_before(record: &BTreeMap<String, String>, destination: &Path, to: &Path) -> io::Result<bool> {
    match record.get(&relative(destination, to)) {
        Some(hash) => Ok(*hash == crate::hashes::of_reader(fs::File::open(to)?)?),
        None => Ok(false),
    }
}

/// What moving a folder copies: everything in it, except what lies inside a destination within it.
fn migration_copies(source: &Path, destination: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut files = Vec::new();
    if source.is_dir() {
        collect_files(source, &mut files)?;
    }
    let inside = format!("{}{}", destination.to_string_lossy().to_uppercase(), std::path::MAIN_SEPARATOR);
    let record = source.join(MOVE_RECORD);
    Ok(files
        .into_iter()
        .filter(|file| !file.to_string_lossy().to_uppercase().starts_with(&inside) && *file != record)
        .map(|file| {
            let target = destination.join(file.strip_prefix(source).unwrap_or(&file));
            (file, target)
        })
        .collect())
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_files(&entry.path(), files)?;
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// The first file the destination holds with other content than the one moving the folder would copy there.
fn migration_conflict(source: &Path, destination: &Path) -> io::Result<Option<PathBuf>> {
    let record = move_record(destination);
    for (from, to) in migration_copies(source, destination)? {
        if to.exists() && !same_content(&from, &to)? && !copied_before(&record, destination, &to)? {
            return Ok(Some(to));
        }
    }
    Ok(None)
}

fn same_content(first: &Path, second: &Path) -> io::Result<bool> {
    Ok(fs::read(first)? == fs::read(second)?)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_module_switch_reads_whatever_its_case_and_unclear_reads_off() {
        let record = |json: &str| serde_json::from_str::<Value>(json).unwrap();
        let flag = |json: &str, default| module_flag(Some(&record(json)), "autoConnect", default);
        assert!(flag("{}", true));
        assert!(!module_flag(None, "autoConnect", false));
        assert!(!flag(r#"{"autoConnect": false}"#, true));
        // As the C# app's `AutoConnect` property, or typed by hand.
        assert!(!flag(r#"{"AutoConnect": false}"#, true));
        assert!(flag(r#"{"AUTOCONNECT": true, "autoConnect": true}"#, false));
        // Spellings that disagree, or a value that is no switch, read as off, whatever the default.
        assert!(!flag(r#"{"AutoConnect": false, "autoConnect": true}"#, true));
        assert!(!flag(r#"{"autoConnect": "false"}"#, true));
        assert!(!flag(r#"{"autoConnect": null}"#, true));
    }

    const TRAINER: LegacyModuleSettings = LegacyModuleSettings {
        module_id: "trainer",
        enabled_key: Some("trainerEnabled"),
        keys: &["autoConnect", "iKnowWhatImDoing", "hookValues"],
    };

    #[test]
    fn io_failures_read_as_the_c_sharp_app_words_them() {
        let path = Path::new(r"C:\data\settings.json");
        assert_eq!(
            describe_io(&io::Error::from_raw_os_error(5), path),
            r"Access to the path 'C:\data\settings.json' is denied."
        );
        let other = describe_io(&io::Error::from_raw_os_error(112), path);
        assert!(!other.contains("os error") && other.ends_with(r" : 'C:\data\settings.json'"), "{other}");
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        file: PathBuf,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app");
        fs::create_dir_all(&root).unwrap();
        let file = temp.path().join("local").join("settings.json");
        Fixture { root, file, _temp: temp }
    }

    fn open(fixture: &Fixture) -> SettingsStore {
        SettingsStore::open(&fixture.root, &fixture.file, &[TRAINER])
    }

    fn written(fixture: &Fixture) -> Value {
        serde_json::from_slice(&fs::read(&fixture.file).unwrap()).unwrap()
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_opening_writes_nothing() {
        let fixture = fixture();
        let store = open(&fixture);
        assert_eq!(store.saved(), &AppSettings::default());
        assert_eq!(store.startup_error(), None);
        assert!(!store.unreadable());
        assert!(!fixture.file.exists());
        assert_eq!(store.paths().user_data_directory, fixture.root.join("data"));
    }

    #[test]
    fn the_c_sharp_apps_file_reads_whatever_the_case_and_writes_back_camel_case() {
        let fixture = fixture();
        let game = fixture.root.join("Last Epoch.exe");
        fs::write(&game, b"").unwrap();
        let document = serde_json::json!({
            "SchemaVersion": 2,
            "GameExecutable": game.to_string_lossy(),
            "modules": { "trainer": false },
            "moduleSettings": { "trainer": { "autoConnect": false } },
            "somethingNewer": 1
        });
        fs::create_dir_all(fixture.file.parent().unwrap()).unwrap();
        fs::write(&fixture.file, document.to_string()).unwrap();
        let mut store = open(&fixture);
        assert_eq!(store.startup_error(), None);
        assert_eq!(store.saved().game_executable.as_deref(), Some(game.as_path()));
        assert_eq!(store.startup_modules().get("trainer"), Some(&false));
        store
            .save_switches(|saved| {
                saved.modules.remove("trainer");
            })
            .unwrap();
        assert_eq!(
            written(&fixture),
            serde_json::json!({
                "schemaVersion": 2,
                "gameExecutable": game.to_string_lossy(),
                "moduleSettings": { "trainer": { "autoConnect": false } },
                "somethingNewer": 1
            })
        );
        // The run keeps the modules it started with.
        assert_eq!(store.startup_modules().get("trainer"), Some(&false));
    }

    #[test]
    fn a_version_one_file_moves_its_module_keys_into_the_slice_once() {
        let fixture = fixture();
        fs::create_dir_all(fixture.file.parent().unwrap()).unwrap();
        fs::write(&fixture.file, r#"{"schemaVersion":1,"TrainerEnabled":true,"autoConnect":false,"hookValues":{"a":[2.0]}}"#).unwrap();
        let store = open(&fixture);
        assert_eq!(store.startup_error(), None);
        assert_eq!(store.startup_modules().get("trainer"), Some(&true));
        assert_eq!(
            written(&fixture),
            serde_json::json!({
                "schemaVersion": 2,
                "modules": { "trainer": true },
                "moduleSettings": { "trainer": { "autoConnect": false, "hookValues": { "a": [2.0] } } }
            })
        );
    }

    #[test]
    fn an_unreadable_file_is_never_replaced_until_the_locations_are_saved() {
        let fixture = fixture();
        fs::create_dir_all(fixture.file.parent().unwrap()).unwrap();
        fs::write(&fixture.file, "{ not json").unwrap();
        let mut store = open(&fixture);
        assert!(store.unreadable());
        assert!(store.startup_error().unwrap().starts_with("Settings could not be loaded: "));
        store
            .save_switches(|saved| {
                saved.modules.insert("trainer".into(), true);
            })
            .unwrap();
        assert_eq!(fs::read_to_string(&fixture.file).unwrap(), "{ not json");
        assert_eq!(store.saved().modules.get("trainer"), Some(&true));
        store.save_locations(&Locations::default()).unwrap();
        assert!(!store.unreadable());
        assert_eq!(written(&fixture), serde_json::json!({ "schemaVersion": 2, "modules": { "trainer": true } }));
    }

    #[test]
    fn a_newer_version_or_a_relative_path_is_not_read() {
        let fixture = fixture();
        fs::create_dir_all(fixture.file.parent().unwrap()).unwrap();
        fs::write(&fixture.file, r#"{"schemaVersion":3}"#).unwrap();
        assert!(open(&fixture).startup_error().unwrap().contains("Unsupported settings version."));
        fs::write(&fixture.file, r#"{"schemaVersion":2,"userDataDirectory":"data"}"#).unwrap();
        assert!(open(&fixture).startup_error().unwrap().contains("Settings paths must be absolute and normalized."));
    }

    #[test]
    fn locations_are_checked_and_saved_normalised() {
        let fixture = fixture();
        let mut store = open(&fixture);
        let refused = |store: &mut SettingsStore, locations: Locations| store.save_locations(&locations).unwrap_err();
        assert_eq!(
            refused(
                &mut store,
                Locations {
                    game_executable: "Last Epoch.exe".into(),
                    ..Locations::default()
                }
            ),
            "Enter the full path of the game executable, starting with its drive (such as C:\\), or use Browse."
        );
        assert_eq!(
            refused(
                &mut store,
                Locations {
                    game_executable: fixture.root.join("missing.exe").to_string_lossy().into(),
                    ..Locations::default()
                }
            ),
            "Choose an existing game executable (.exe), or use automatic detection."
        );
        assert_eq!(
            refused(
                &mut store,
                Locations {
                    dotnet_directory: Some(fixture.root.to_string_lossy().into()),
                    ..Locations::default()
                }
            ),
            "Choose the folder that holds dotnet.exe, or leave it empty to find the .NET SDK automatically."
        );
        let elsewhere = fixture.root.join("elsewhere");
        assert_eq!(
            refused(
                &mut store,
                Locations {
                    user_data_directory: elsewhere.to_string_lossy().into(),
                    mod_library_directory: Some(fixture.root.join("library").to_string_lossy().into()),
                    ..Locations::default()
                }
            ),
            "Move the user-data folder and the mod library one at a time: save one, restart Eterra's Gift, then change the other."
        );
        let game = fixture.root.join("Game").join("Last Epoch.exe");
        fs::create_dir_all(game.parent().unwrap()).unwrap();
        fs::write(&game, b"").unwrap();
        store
            .save_locations(&Locations {
                game_executable: format!("  {}  ", game.display()),
                user_data_directory: format!("{}\\", elsewhere.display()),
                ..Locations::default()
            })
            .unwrap();
        assert_eq!(store.saved().game_executable.as_deref(), Some(game.as_path()));
        assert_eq!(store.saved().user_data_directory.as_deref(), Some(elsewhere.as_path()));
        assert_eq!(store.saved().migration_source.as_deref(), Some(fixture.root.join("data").as_path()));
        // This run keeps what it started with; the move waits for the next start.
        assert_eq!(store.active_game_executable(), None);
        assert_eq!(store.paths().user_data_directory, fixture.root.join("data"));
    }

    #[test]
    fn a_user_data_move_copies_everything_at_the_next_start_and_leaves_the_old_folder() {
        let fixture = fixture();
        let old = fixture.root.join("data");
        fs::create_dir_all(old.join("logs")).unwrap();
        fs::write(old.join("logs").join("fault.json"), b"{}").unwrap();
        let new = fixture.root.join("moved");
        let mut store = open(&fixture);
        store
            .save_locations(&Locations {
                user_data_directory: new.to_string_lossy().into(),
                ..Locations::default()
            })
            .unwrap();
        let store = open(&fixture);
        assert_eq!(store.startup_error(), None);
        assert_eq!(store.paths().user_data_directory, new);
        assert_eq!(fs::read(new.join("logs").join("fault.json")).unwrap(), b"{}");
        assert!(old.join("logs").join("fault.json").exists());
        assert_eq!(store.saved().migration_source, None);
    }

    #[test]
    fn a_move_that_failed_partway_finishes_later_over_its_own_copies_only() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = (temp.path().join("data"), temp.path().join("moved"));
        fs::create_dir_all(old.join("trainer")).unwrap();
        fs::write(old.join("profile.json"), b"one").unwrap();
        fs::write(old.join("trainer").join("catalog.json"), b"catalog").unwrap();
        // The first attempt copies profile.json, then cannot make the trainer folder (a file is in its way).
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("trainer"), b"in the way").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert_eq!(fs::read(new.join("profile.json")).unwrap(), b"one");
        // The old folder stays in use until the move is done, and changes.
        fs::write(old.join("profile.json"), b"two").unwrap();
        fs::remove_file(new.join("trainer")).unwrap();
        assert_eq!(migration_conflict(&old, &new).unwrap(), None);
        migrate(&old, &new).unwrap();
        assert_eq!(fs::read(new.join("profile.json")).unwrap(), b"two");
        assert_eq!(fs::read(new.join("trainer").join("catalog.json")).unwrap(), b"catalog");
        assert!(!new.join(MOVE_RECORD).exists());
    }

    #[test]
    fn a_retried_move_never_replaces_a_file_changed_after_it_was_copied() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = (temp.path().join("data"), temp.path().join("moved"));
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(old.join("trainer")).unwrap();
        fs::write(old.join("a.json"), b"one").unwrap();
        fs::write(old.join("trainer").join("catalog.json"), b"catalog").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("trainer"), b"in the way").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert_eq!(fs::read(new.join("a.json")).unwrap(), b"one");
        fs::remove_file(new.join("trainer")).unwrap();
        fs::write(old.join("a.json"), b"two").unwrap();
        // Someone else's edit of the copy: no longer the move's own.
        fs::write(new.join("a.json"), b"edited").unwrap();
        assert_eq!(migration_conflict(&old, &new).unwrap(), Some(new.join("a.json")));
        assert_eq!(migrate(&old, &new).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(new.join("a.json")).unwrap(), b"edited");
    }

    #[test]
    fn a_move_into_a_folder_with_other_content_is_refused_before_it_is_saved() {
        let fixture = fixture();
        let old = fixture.root.join("data");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("labels.json"), b"mine").unwrap();
        let new = fixture.root.join("taken");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("labels.json"), b"theirs").unwrap();
        let mut store = open(&fixture);
        assert_eq!(
            store
                .save_locations(&Locations {
                    user_data_directory: new.to_string_lossy().into(),
                    ..Locations::default()
                })
                .unwrap_err(),
            "The selected folder already holds labels.json with other content. Choose an empty user-data folder."
        );
        assert!(!fixture.file.exists());
    }
}
