//! One coherent file-backed observation for Mods. No module code is loaded or executed to describe a DLL.

use super::{
    assembly::obstacle,
    description::ModSettingsDescription,
    library::{InstalledMod, ModChanges, ModLibrary},
    preferences::ModSetting,
};
use crate::game::install::{GameInstall, LAST_EPOCH_DEVELOPER, LAST_EPOCH_PRODUCT};
use crate::launch::loader::{ModLoaderState, ModLoaderSwitch, melon_paths};
use melon_loader::assembly::{GameIdentity, LoaderVersion};
use melon_loader::loader_state::{self, LoaderInstall, LoaderPreparation};
use melon_loader::log::{LogProblem, MelonLog, ModLogRun};
use melon_loader::preferences::{MelonPreferences, PreferenceKind, PreferenceValue};
use spiderland_phrases::{Conjunction, list, phrases};
use std::path::{Path, PathBuf};

phrases! {
    TurnsOn: "Turns on the next time you play with mods"
    TurnsOff: "Turns off the next time you play with mods"
    BuildWaiting: "This build is installed the next time you play with mods"
    NoGame: "Last Epoch wasn't found. Choose the game executable in Settings."
    Unreadable: "The mods could not be read: {error}"
    SettingsUnreadable: "Settings could not be read: {error}"
    NoMelonInfo: "{file} has no MelonInfo, so it is not a MelonLoader mod. If a mod needs it, it belongs in UserLibs."
    NotAssembly: "{file} is not a .NET assembly, so MelonLoader can't load it."
    CopyUnreadable: "The app's copy of {file} could not be read: {error}"
    DescriptionUnreadable: "{file} describes its settings in a way the app can't read, so they show as MelonPreferences.cfg has them. {error}"
    Conflict: "The game folder has both version.dll and version.dll.disabled. Delete the older one."
    NoLoader: "MelonLoader isn't in the game folder, so no mod can load."
    NoSupportRepairRunning: "MelonLoader couldn't start its Unity support, so no mod runs. Close Last Epoch and play with mods again to repair it."
    NoSupportRunning: "MelonLoader couldn't start its Unity support, so no mod runs. See its log."
    NoSupportRepair: "In the last game with mods, MelonLoader couldn't start its Unity support. Playing with mods again repairs it."
    NoSupport: "In the last game with mods, MelonLoader couldn't start its Unity support. See its log."
    Preparing: "MelonLoader is preparing for this game build, so this game runs without mods. Close it once it's up and play with mods again."
    DidNotLoad: "{name} didn't load last time. Turn it off or replace it."
    CantLoad: "{name} can't load: {problem} Turn it off or replace it."
    ManyCantLoad: "{n} mod can't load: {names}. Turn it off or replace it." | "{n} mods can't load: {names}. Turn them off or replace them."
    Running: "Last Epoch is running. Play with mods once it closes."
    LoaderVersion: "MelonLoader {version}"
    GameVersion: "Last Epoch {version}"
    WillPrepare: "The next game with mods prepares MelonLoader for this game build and runs without mods. Play again once it's up."
    NeedsRepair: "MelonLoader's Unity support needs a repair. Play offline with mods repairs it first."
    LastPlayedToday: "Last played with mods today at {time}"
    LastPlayedYesterday: "Last played with mods yesterday at {time}"
    LastPlayedOn: "Last played with mods on {date} at {time}"
    ClosedEarly: "The game closed before MelonLoader finished loading mods."
    NoReason: "MelonLoader read it but did not load it, and its log gives no reason."
    NotFound: "MelonLoader did not find it in the Mods folder."
}

/// Why the log says a mod did not load, as a sentence; a line from the log is shown as MelonLoader wrote it.
fn log_problem(problem: LogProblem) -> String {
    match problem {
        LogProblem::ClosedEarly => ClosedEarly.into(),
        LogProblem::Logged(line) => line,
        LogProblem::NoReason => NoReason.into(),
        LogProblem::NotFound => NotFound.into(),
    }
}

#[derive(Clone, Debug)]
pub struct LoaderReport {
    pub version: LoaderVersion,
    pub preparation: LoaderPreparation,
    pub proxy: ModLoaderState,
    pub core_module_needs_repair: bool,
    pub last_game_without_support: bool,
}

#[derive(Clone, Debug)]
pub struct ModReport {
    pub installed: InstalledMod,
    pub name: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub was_on: bool,
    pub loaded_last_time: Option<bool>,
    pub problem: Option<String>,
    pub log: Vec<ModLogRun>,
    pub settings: Vec<ModSetting>,
    pub log_from_other_build: bool,
    pub build_waiting: bool,
    pub project_version: Option<String>,
}

impl ModReport {
    pub fn draft_count(&self) -> usize {
        usize::from(self.installed.enabled != self.was_on) + self.settings.iter().filter(|setting| setting.is_drafted()).count()
    }
    pub fn state_text(&self) -> &'static str {
        match (self.installed.enabled, self.was_on) {
            (true, false) => TurnsOn.get(),
            (false, true) => TurnsOff.get(),
            (true, true) if self.build_waiting => BuildWaiting.get(),
            _ => "",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub mods_folder: Option<PathBuf>,
    pub loader_log: Option<PathBuf>,
    pub loader: Option<LoaderReport>,
    pub last_launch: Option<jiff::Timestamp>,
    pub mods: Vec<ModReport>,
    pub changes: ModChanges,
    pub error: Option<String>,
    pub settings_problem: Option<String>,
}

pub fn read(install: Option<&GameInstall>, library: &ModLibrary, loader_root: Option<&Path>) -> Inventory {
    let Some(install) = install else {
        return Inventory {
            error: Some(NoGame.into()),
            ..Default::default()
        };
    };
    let root = loader_root.unwrap_or(install.directory());
    let mut inventory = Inventory {
        mods_folder: Some(melon_paths::mods(root)),
        ..Default::default()
    };
    let records = (|| Ok::<_, std::io::Error>((library.reconcile(root)?, library.profile()?, library.placed(root)?)))();
    let (changes, profile, placed) = match records {
        Ok(value) => value,
        Err(error) => {
            inventory.error = Some(Unreadable.with(&[("error", &error)]));
            return inventory;
        }
    };
    inventory.changes = changes;
    inventory.last_launch = placed.last_launch;
    let log_path = melon_paths::latest_log(root);
    let log = MelonLog::read(&log_path).ok();
    inventory.loader_log = log_path.is_file().then_some(log_path);
    let proxy = ModLoaderSwitch::new(install.directory(), Some(root)).state();
    inventory.loader = LoaderInstall::find(root).filter(|_| proxy != ModLoaderState::NotInstalled).map(|loader| LoaderReport {
        preparation: loader.preparation_for(&install.game_assembly(), install.read_unity_version().as_deref()),
        version: loader.version,
        proxy,
        core_module_needs_repair: loader_state::needs_repair(root),
        last_game_without_support: log.as_ref().is_some_and(MelonLog::support_failed),
    });
    let preferences = MelonPreferences::load(&melon_paths::preferences(root)).unwrap_or_else(|error| {
        inventory.settings_problem = Some(SettingsUnreadable.with(&[("error", &error)]));
        MelonPreferences::parse("").expect("empty preferences")
    });
    let game = install.read_identity().unwrap_or_else(|| GameIdentity {
        developer: LAST_EPOCH_DEVELOPER.into(),
        product: LAST_EPOCH_PRODUCT.into(),
    });
    let mut descriptions = Vec::new();
    let mut read = Vec::new();
    library.keep_facts(&profile.mods);
    for installed in profile.mods {
        let facts = library.facts(&installed);
        let (melon, read_problem) = match &facts.melon {
            Ok(Some(melon)) => (Some(melon), None),
            Ok(None) => (None, Some(NoMelonInfo.with(&[("file", &installed.file)]))),
            Err((std::io::ErrorKind::InvalidData, _)) => (None, Some(NotAssembly.with(&[("file", &installed.file)]))),
            Err((_, error)) => (None, Some(CopyUnreadable.with(&[("file", &installed.file), ("error", error)]))),
        };
        let description = match &facts.resource {
            Ok(resource) => resource
                .as_ref()
                .map(|bytes| ModSettingsDescription::from_bytes(bytes))
                .transpose()
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.clone()),
        };
        descriptions.push(description.unwrap_or_else(|error| {
            inventory
                .settings_problem
                .get_or_insert_with(|| DescriptionUnreadable.with(&[("file", &installed.file), ("error", &error)]));
            None
        }));
        let name = melon
            .as_ref()
            .map(|melon| melon.name.clone())
            .unwrap_or_else(|| Path::new(&installed.file).file_stem().unwrap().to_string_lossy().into_owned());
        let was_on = placed.hash_of(&installed.file).is_some();
        let loaded = log.as_ref().and_then(|log| log.loaded_last_time(&installed.file, &installed.hash));
        let problem = read_problem
            .or_else(|| {
                melon
                    .as_ref()
                    .zip(inventory.loader.as_ref())
                    .and_then(|(melon, loader)| obstacle(melon, &game, &loader.version))
            })
            .or_else(|| {
                if loaded == Some(false) {
                    log.as_ref().and_then(|log| log.problem_for(&name, &installed.file)).map(log_problem)
                } else {
                    None
                }
            });
        let lines = if was_on {
            log.as_ref().map(|log| log.runs_for(&name, &installed.file)).unwrap_or_default()
        } else {
            Vec::new()
        };
        let other_build = !lines.is_empty()
            && log
                .as_ref()
                .and_then(|log| log.hash_of(&installed.file))
                .is_some_and(|hash| !hash.eq_ignore_ascii_case(&installed.hash));
        let waiting = placed.hash_of(&installed.file).is_some_and(|hash| !hash.eq_ignore_ascii_case(&installed.hash));
        inventory.mods.push(ModReport {
            installed,
            name,
            version: melon.as_ref().map(|melon| melon.version.clone()),
            author: melon.as_ref().map(|melon| melon.author.clone()),
            description: melon.as_ref().and_then(|melon| melon.description.clone()),
            link: melon.as_ref().and_then(|melon| melon.download_link.clone()),
            project_version: melon.as_ref().and_then(|melon| melon.project_version.clone()),
            was_on,
            loaded_last_time: loaded,
            problem,
            log: lines,
            settings: Vec::new(),
            log_from_other_build: other_build,
            build_waiting: waiting,
        });
        read.push(facts);
    }
    for category in &preferences.categories {
        let mut claims: Vec<_> = descriptions
            .iter()
            .enumerate()
            .filter(|(_, description)| description.as_ref().is_some_and(|description| description.category(&category.name).is_some()))
            .map(|(index, _)| index)
            .collect();
        if claims.is_empty() {
            claims = read
                .iter()
                .enumerate()
                .filter(|(_, facts)| facts.strings.contains(&category.name))
                .map(|(index, _)| index)
                .collect();
        }
        let Some(first) = claims.first().copied() else {
            continue;
        };
        let squeeze = |name: &str| name.replace(' ', "").to_uppercase();
        let owner = claims
            .into_iter()
            .find(|index| squeeze(&inventory.mods[*index].name) == squeeze(&category.name))
            .unwrap_or(first);
        let described = descriptions[owner].as_ref().and_then(|description| description.category(&category.name));
        let mut entries: Vec<_> = category.entries.iter().filter(|entry| entry.kind != PreferenceKind::Other).collect();
        if let Some(described) = described {
            entries.retain(|entry| described.setting(&entry.key).is_some());
            entries.sort_by_key(|entry| described.settings.iter().position(|setting| setting.key == entry.key).unwrap());
        }
        for entry in entries {
            let draft = profile
                .settings
                .get(&category.name)
                .and_then(|settings| settings.get(&entry.key))
                .and_then(PreferenceValue::from_json)
                .filter(|value| entry.kind.accepts(value));
            inventory.mods[owner].settings.push(ModSetting::new(
                &category.name,
                entry,
                draft.as_ref(),
                described.and_then(|described| described.setting(&entry.key)),
            ));
        }
    }
    inventory
}

pub struct Readiness {
    /// What to do or know, as a sentence; empty when all is well.
    pub text: String,
    /// Short facts shown side by side, each its own text: the loader, the game, the last game with mods.
    pub facts: Vec<String>,
    pub warning: bool,
    pub can_play: bool,
}
impl Inventory {
    pub fn readiness(&self, version: Option<&str>, running: bool, modded_run: bool, starting: bool, now: jiff::Timestamp) -> Readiness {
        let report = self.loader.as_ref();
        let missing = report.is_none_or(|report| report.proxy == ModLoaderState::Conflict);
        let can_play = self.error.is_none() && !missing && !running && !starting;
        let failure = |text| Readiness {
            text,
            facts: Vec::new(),
            warning: true,
            can_play,
        };
        if let Some(error) = &self.error {
            return failure(error.clone());
        }
        if missing {
            return failure(if report.is_some() { Conflict.into() } else { NoLoader.into() });
        }
        let report = report.unwrap();
        if report.last_game_without_support && (!running || modded_run) {
            return failure(
                match (running, report.core_module_needs_repair) {
                    (true, true) => NoSupportRepairRunning,
                    (true, false) => NoSupportRunning,
                    (false, true) => NoSupportRepair,
                    _ => NoSupport,
                }
                .into(),
            );
        }
        let failing: Vec<_> = self.mods.iter().filter(|item| item.installed.enabled && item.problem.is_some()).collect();
        if report.preparation == LoaderPreparation::WillPrepare && running && modded_run {
            return Readiness {
                text: Preparing.into(),
                facts: Vec::new(),
                warning: !failing.is_empty(),
                can_play,
            };
        }
        if let Some(first) = failing.first() {
            return failure(if failing.len() == 1 {
                if first.loaded_last_time == Some(false) {
                    DidNotLoad.with(&[("name", &first.name)])
                } else {
                    CantLoad.with(&[("name", &first.name), ("problem", &first.problem.as_deref().unwrap())])
                }
            } else {
                let names: Vec<&str> = failing.iter().map(|item| item.name.as_str()).collect();
                ManyCantLoad.with(failing.len(), &[("names", &list(&names, Conjunction::And))])
            });
        }
        if running {
            return Readiness {
                text: Running.into(),
                facts: Vec::new(),
                warning: false,
                can_play,
            };
        }
        let mut facts = vec![LoaderVersion.with(&[("version", &report.version)])];
        facts.extend(version.map(|version| GameVersion.with(&[("version", &version)])));
        let mut text = String::new();
        if report.preparation == LoaderPreparation::WillPrepare {
            text = WillPrepare.into();
        } else if report.core_module_needs_repair {
            text = NeedsRepair.into();
        } else if let Some(last) = self.last_launch {
            let zone = jiff::tz::TimeZone::system();
            let local = last.to_zoned(zone.clone());
            let today = now.to_zoned(zone).date();
            let time = local.strftime("%H:%M").to_string();
            facts.push(if local.date() == today {
                LastPlayedToday.with(&[("time", &time)])
            } else if today.yesterday().ok() == Some(local.date()) {
                LastPlayedYesterday.with(&[("time", &time)])
            } else {
                LastPlayedOn.with(&[("date", &local.strftime("%-d %B")), ("time", &time)])
            });
        }
        Readiness {
            text,
            facts,
            warning: false,
            can_play,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_log_problem_has_its_sentence() {
        assert_eq!(log_problem(LogProblem::ClosedEarly), "The game closed before MelonLoader finished loading mods.");
        assert_eq!(log_problem(LogProblem::NoReason), "MelonLoader read it but did not load it, and its log gives no reason.");
        assert_eq!(log_problem(LogProblem::NotFound), "MelonLoader did not find it in the Mods folder.");
        assert_eq!(log_problem(LogProblem::Logged("Failed to load.".into())), "Failed to load.");
    }
}
