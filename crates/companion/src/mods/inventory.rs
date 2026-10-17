//! One coherent file-backed observation for Mods. No module code is loaded or executed to describe a DLL.

use super::{
    assembly::{self, LoaderVersion, MelonAssembly},
    description::ModSettingsDescription,
    library::{InstalledMod, ModChanges, ModLibrary, latest_log_path, preferences_path},
    loader_state::{self, LoaderInstall, LoaderPreparation},
    log::{MelonLog, ModLogRun},
    preferences::{MelonPreferences, ModSetting, PreferenceKind, PreferenceValue},
};
use crate::install::{GameIdentity, GameInstall};
use crate::loader::{self, ModLoaderState, ModLoaderSwitch};
use std::path::{Path, PathBuf};

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
            (true, false) => "On from the next game with mods",
            (false, true) => "Off from the next game with mods",
            (true, true) if self.build_waiting => "This build replaces the one in the Mods folder at the next game with mods",
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
            error: Some("Last Epoch was not found, so its mods can't be read. Choose the game executable in Settings.".into()),
            ..Default::default()
        };
    };
    let root = loader_root.unwrap_or(install.directory());
    let mut inventory = Inventory {
        mods_folder: Some(root.join("Mods")),
        ..Default::default()
    };
    let records = (|| Ok::<_, std::io::Error>((library.reconcile(root)?, library.profile()?, library.placed(root)?)))();
    let (changes, profile, placed) = match records {
        Ok(value) => value,
        Err(error) => {
            inventory.error = Some(format!("The mods could not be read: {error}"));
            return inventory;
        }
    };
    inventory.changes = changes;
    inventory.last_launch = placed.last_launch;
    let log_path = latest_log_path(root);
    let log = MelonLog::read(&log_path).ok();
    inventory.loader_log = log_path.is_file().then_some(log_path);
    let proxy = ModLoaderSwitch::new(install.directory(), Some(root)).state();
    inventory.loader = LoaderInstall::find(root).filter(|_| proxy != ModLoaderState::NotInstalled).map(|loader| LoaderReport {
        preparation: loader.preparation_for(install),
        version: loader.version,
        proxy,
        core_module_needs_repair: loader_state::needs_repair(root),
        last_game_without_support: log.as_ref().is_some_and(MelonLog::support_failed),
    });
    let preferences = MelonPreferences::load(&preferences_path(root)).unwrap_or_else(|error| {
        inventory.settings_problem = Some(format!("Settings could not be read: {error}"));
        MelonPreferences::parse("").expect("empty preferences")
    });
    let game = install.read_identity().unwrap_or_else(|| GameIdentity {
        developer: GameIdentity::LAST_EPOCH_DEVELOPER.into(),
        product: GameIdentity::LAST_EPOCH_PRODUCT.into(),
    });
    let mut descriptions = Vec::new();
    let mut strings = Vec::new();
    for installed in profile.mods {
        let path = library.path_of(&installed);
        let (melon, read_problem) = match MelonAssembly::read(&path) {
            Ok(Some(melon)) => (Some(melon), None),
            Ok(None) => (
                None,
                Some(format!(
                    "{} has no MelonInfo, so it is not a MelonLoader mod. If a mod needs it, it belongs in UserLibs.",
                    installed.file
                )),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => (None, Some(format!("{} is not a .NET assembly, so MelonLoader can't load it.", installed.file))),
            Err(error) => (None, Some(format!("The app's copy of {} could not be read: {error}", installed.file))),
        };
        let description = if melon.is_some() {
            assembly::read_settings_resource(&path).and_then(|resource| resource.map(|bytes| ModSettingsDescription::from_bytes(&bytes)).transpose())
        } else {
            Ok(None)
        };
        descriptions.push(description.unwrap_or_else(|error| {
            inventory.settings_problem.get_or_insert_with(|| {
                format!(
                    "{} describes its settings in a way the app can't read, so they show as MelonPreferences.cfg has them. {error}",
                    installed.file
                )
            });
            None
        }));
        strings.push(assembly::read_strings(&path));
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
                    .and_then(|(melon, loader)| melon.obstacle(&game, &loader.version))
            })
            .or_else(|| {
                if loaded == Some(false) {
                    log.as_ref().and_then(|log| log.problem_for(&name, &installed.file))
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
    }
    for category in &preferences.categories {
        let mut claims: Vec<_> = descriptions
            .iter()
            .enumerate()
            .filter(|(_, description)| description.as_ref().is_some_and(|description| description.category(&category.name).is_some()))
            .map(|(index, _)| index)
            .collect();
        if claims.is_empty() {
            claims = strings
                .iter()
                .enumerate()
                .filter(|(_, strings)| strings.contains(&category.name))
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
    pub text: String,
    pub warning: bool,
    pub can_play: bool,
}
impl Inventory {
    pub fn readiness(&self, version: Option<&str>, running: bool, modded_run: bool, starting: bool, now: jiff::Timestamp) -> Readiness {
        let report = self.loader.as_ref();
        let missing = report.is_none_or(|report| report.proxy == ModLoaderState::Conflict);
        let can_play = self.error.is_none() && !missing && !running && !starting;
        let failure = |text| Readiness { text, warning: true, can_play };
        if let Some(error) = &self.error {
            return failure(error.clone());
        }
        if missing {
            return failure(if report.is_some() {
                format!("{} Delete the older one.", loader::CONFLICT)
            } else {
                "MelonLoader is not in the game folder, so no mod can load. Play offline with mods needs it.".into()
            });
        }
        let report = report.unwrap();
        if report.last_game_without_support && (!running || modded_run) {
            return failure(match (running,report.core_module_needs_repair) {
                (true,true) => "In this game MelonLoader could not start its Unity support, so no mod runs. Close Last Epoch: the next game with mods repairs the cause, a broken UnityEngine.CoreModule.dll MelonLoader generated for this game build, and loads them.",
                (true,false) => "In this game MelonLoader could not start its Unity support, so no mod runs. See its log for why.",
                (false,true) => "In the last game with mods, MelonLoader could not start its Unity support, so no mod ran. The cause, a broken UnityEngine.CoreModule.dll MelonLoader generated for this game build, is repaired when you next play with mods.",
                _ => "In the last game with mods, MelonLoader could not start its Unity support, so no mod ran. See its log for why."
            }.into());
        }
        let failing: Vec<_> = self.mods.iter().filter(|item| item.installed.enabled && item.problem.is_some()).collect();
        if report.preparation == LoaderPreparation::WillPrepare && running && modded_run {
            return Readiness { text: "Last Epoch is running while MelonLoader prepares for this game build, which takes about a minute. This game runs without mods: close it once it is up, and the next game with mods loads them.".into(), warning: !failing.is_empty(), can_play };
        }
        if let Some(first) = failing.first() {
            return failure(if failing.len() == 1 {
                if first.loaded_last_time == Some(false) {
                    format!("{} did not load in the last game with mods. Turn it off or replace it before you play.", first.name)
                } else {
                    format!(
                        "{} can't load: {} Turn it off or replace it before you play.",
                        first.name,
                        first.problem.as_deref().unwrap()
                    )
                }
            } else {
                format!(
                    "{} mods that are on can't load: {} and {}. Turn them off or replace them.",
                    failing.len(),
                    failing[..failing.len() - 1].iter().map(|item| item.name.as_str()).collect::<Vec<_>>().join(", "),
                    failing.last().unwrap().name
                )
            });
        }
        if running {
            return Readiness {
                text: "Last Epoch is running. Play with mods once it has closed.".into(),
                warning: false,
                can_play,
            };
        }
        let mut text = format!(
            "MelonLoader {} · Last Epoch {}",
            report.version,
            version.unwrap_or("(version unknown until this build has run once)")
        );
        if report.preparation == LoaderPreparation::WillPrepare {
            text.push_str(". MelonLoader prepares for this game build in the next game with mods: it takes about a minute to start, and runs without mods. Close it once it is up; the game after loads them.");
        } else if report.core_module_needs_repair {
            text.push_str(". MelonLoader's UnityEngine.CoreModule.dll for this game build would stop every mod; Play offline with mods repairs it first.");
        } else if let Some(last) = self.last_launch {
            let zone = jiff::tz::TimeZone::system();
            let local = last.to_zoned(zone.clone());
            let today = now.to_zoned(zone).date();
            let day = if local.date() == today {
                "today".into()
            } else if today.yesterday().ok() == Some(local.date()) {
                "yesterday".into()
            } else {
                format!("on {}", local.strftime("%-d %B"))
            };
            text.push_str(&format!(" · last game with mods {day} at {}", local.strftime("%H:%M")));
        }
        Readiness { text, warning: false, can_play }
    }
}
