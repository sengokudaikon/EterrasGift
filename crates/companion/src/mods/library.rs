//! The mod library's two records. profile.json is what the player chose: each mod added (its DLL kept in the library
//! under its hash) and whether it is on, and the setting changes waiting for the next start. placed.json is what was
//! last put in a game's Mods folder, and when that game started. Drafts are durable here, and reach MelonLoader only
//! before a with-mods start.

use super::assembly::{AssemblyFacts, MelonAssembly};
use super::invalid;
use super::preferences::{MelonPreferences, PreferenceChange, PreferenceValue};
use crate::launch::loader::melon_paths;
use eg_core::{atomic, hashes, json, paths};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledMod {
    pub file: String,
    pub hash: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModProfile {
    pub mods: Vec<InstalledMod>,
    pub settings: BTreeMap<String, BTreeMap<String, Value>>,
}

impl ModProfile {
    /// Drops a waiting setting change, and its category once that has none left.
    fn clear_setting(&mut self, category: &str, key: &str) {
        if let Some(entries) = self.settings.get_mut(category) {
            entries.remove(key);
            if entries.is_empty() {
                self.settings.remove(category);
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlacedMods {
    pub root: PathBuf,
    pub files: BTreeMap<String, String>,
    pub last_launch: Option<jiff::Timestamp>,
}

impl PlacedMods {
    pub fn hash_of(&self, file: &str) -> Option<&str> {
        self.files.iter().find(|(name, _)| name.eq_ignore_ascii_case(file)).map(|(_, hash)| hash.as_str())
    }

    /// Records `file` as placed with `hash`, in place of any spelling of it recorded before.
    fn record(&mut self, file: String, hash: String) {
        self.forget(&file);
        self.files.insert(file, hash);
    }

    /// Forgets `file`, however its name was spelled when it was recorded.
    fn forget(&mut self, file: &str) {
        self.files.retain(|name, _| !same(name, file));
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModChanges {
    pub adopted: Vec<String>,
    pub removed: Vec<String>,
}

pub struct ModLibrary {
    directory: PathBuf,
    gate: Mutex<()>,
    /// The DLLs read so far, by their copy and its hash. A copy never changes under its name (a new build is a new copy),
    /// so the Mods page, refreshed every two seconds while it follows a start, reads each one once.
    facts: Mutex<HashMap<(PathBuf, String), Arc<AssemblyFacts>>>,
}

impl ModLibrary {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            gate: Mutex::new(()),
            facts: Mutex::new(HashMap::new()),
        }
    }
    /// What the library's copy of `installed` says, read once.
    pub fn facts(&self, installed: &InstalledMod) -> Arc<AssemblyFacts> {
        let path = self.path_of(installed);
        let key = (path.clone(), installed.hash.to_ascii_uppercase());
        if let Some(facts) = self.facts.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return facts.clone();
        }
        let facts = Arc::new(super::assembly::read_facts(&path));
        // Only what was read: a copy that could not be (it was still arriving, say) is read again next time.
        if facts.melon.as_ref().is_ok() || facts.melon.as_ref().is_err_and(|(kind, _)| *kind == io::ErrorKind::InvalidData) {
            self.facts.lock().unwrap_or_else(|p| p.into_inner()).insert(key, facts.clone());
        }
        facts
    }
    /// Forgets the DLLs no longer in the profile.
    pub fn keep_facts(&self, installed: &[InstalledMod]) {
        let kept: BTreeSet<_> = installed.iter().map(|installed| (self.path_of(installed), installed.hash.to_ascii_uppercase())).collect();
        self.facts.lock().unwrap_or_else(|p| p.into_inner()).retain(|key, _| kept.contains(key));
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn profile(&self) -> io::Result<ModProfile> {
        let _gate = self.gate.lock().unwrap();
        self.load_profile()
    }
    pub fn placed(&self, root: &Path) -> io::Result<PlacedMods> {
        let _gate = self.gate.lock().unwrap();
        self.load_placed(root)
    }
    pub fn path_of(&self, installed: &InstalledMod) -> PathBuf {
        self.directory
            .join("library")
            .join(Path::new(&installed.file).file_stem().unwrap_or_default())
            .join(installed.hash.get(..16).unwrap_or_default())
            .join(&installed.file)
    }
    pub fn holds(&self, installed: &InstalledMod) -> bool {
        self.path_of(installed).is_file()
    }
    pub fn locate(&self, installed: &InstalledMod, mods: Option<&Path>) -> PathBuf {
        mods.map(|mods| mods.join(&installed.file))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| self.path_of(installed))
    }
    pub fn add(&self, source: &Path, keep_on_off: bool) -> io::Result<InstalledMod> {
        let file = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if !is_dll(&file) {
            return Err(invalid(format!("{file} is not a DLL. A mod is a .dll file; take it out of a downloaded zip first.")));
        }
        if MelonAssembly::read(source)?.is_none() {
            return Err(invalid(format!(
                "{file} has no MelonInfo, so it is not a MelonLoader mod. If a mod needs it, it belongs in UserLibs."
            )));
        }
        let mut installed = InstalledMod {
            file,
            hash: hash_file(source)?,
            enabled: true,
        };
        let _gate = self.gate.lock().unwrap();
        let mut profile = self.load_profile()?;
        self.store(source, &installed)?;
        if let Some(index) = profile.mods.iter().position(|old| same(&old.file, &installed.file)) {
            if keep_on_off {
                installed.enabled = profile.mods[index].enabled;
            }
            profile.mods[index] = installed.clone();
        } else {
            profile.mods.push(installed.clone());
        }
        self.save_profile(&profile)?;
        Ok(installed)
    }
    pub fn set_enabled(&self, file: &str, enabled: bool) -> io::Result<()> {
        self.change(|profile| {
            if let Some(item) = profile.mods.iter_mut().find(|item| same(&item.file, file)) {
                item.enabled = enabled;
            }
            Ok(())
        })
    }
    pub fn set_setting(&self, category: &str, key: &str, value: Option<Value>) -> io::Result<()> {
        self.change(|profile| {
            if let Some(value) = value {
                profile.settings.entry(category.into()).or_default().insert(key.into(), value);
            } else {
                profile.clear_setting(category, key);
            }
            Ok(())
        })
    }
    pub fn remove(&self, file: &str) -> io::Result<Option<(InstalledMod, usize)>> {
        let mut removed = None;
        self.change(|profile| {
            if let Some(index) = profile.mods.iter().position(|item| same(&item.file, file)) {
                removed = Some((profile.mods.remove(index), index));
            }
            Ok(())
        })?;
        Ok(removed)
    }
    pub fn restore(&self, installed: &InstalledMod, index: usize) -> io::Result<()> {
        self.change(|profile| {
            if profile.mods.iter().any(|item| same(&item.file, &installed.file)) {
                return Ok(());
            }
            if !self.holds(installed) {
                return Err(invalid(format!(
                    "{} was cleared from the library by the last game with mods. Add the DLL again to install it.",
                    installed.file
                )));
            }
            profile.mods.insert(index.min(profile.mods.len()), installed.clone());
            Ok(())
        })
    }
    /// Reverts the page's visible drafts. Unclaimed categories and settings hidden by a newer description are not
    /// shown by the page, so Revert must not silently discard them; preparation releases obsolete entries later.
    pub fn revert(&self, root: &Path, visible_settings: &[(String, String)]) -> io::Result<()> {
        let _gate = self.gate.lock().unwrap();
        let mut profile = self.load_profile()?;
        let placed = self.load_placed(root)?;
        for installed in &mut profile.mods {
            installed.enabled = placed.hash_of(&installed.file).is_some();
        }
        for (category, key) in visible_settings {
            profile.clear_setting(category, key);
        }
        self.save_profile(&profile)
    }
    pub fn reconcile(&self, root: &Path) -> io::Result<ModChanges> {
        let _gate = self.gate.lock().unwrap();
        self.reconcile_locked(root)
    }
    fn reconcile_locked(&self, root: &Path) -> io::Result<ModChanges> {
        let mut profile = self.load_profile()?;
        let mut placed = self.load_placed(root)?;
        let mut changes = ModChanges::default();
        let mut files = mod_files(root)?;
        files.sort_by_key(|path| path.to_string_lossy().to_lowercase());
        let mut present = BTreeSet::new();
        for path in files {
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            present.insert(file.to_lowercase());
            let hash = hash_file(&path)?;
            if placed.hash_of(&file) == Some(&hash) {
                continue;
            }
            let installed = InstalledMod {
                file: file.clone(),
                hash: hash.clone(),
                enabled: true,
            };
            self.store(&path, &installed)?;
            if let Some(old) = profile.mods.iter_mut().find(|old| same(&old.file, &file)) {
                *old = installed;
            } else {
                profile.mods.push(installed);
            }
            placed.record(file.clone(), hash);
            changes.adopted.push(file);
        }
        placed.files.retain(|file, _| {
            if present.contains(&file.to_lowercase()) {
                return true;
            }
            if let Some(index) = profile.mods.iter().position(|item| same(&item.file, file)) {
                changes.removed.push(profile.mods.remove(index).file);
            }
            false
        });
        if !changes.adopted.is_empty() || !changes.removed.is_empty() {
            self.save_profile(&profile)?;
            self.save_placed(&placed)?;
        }
        Ok(changes)
    }
    /// Called off the UI thread, only after the host has checked that no game is running.
    pub fn prepare(&self, root: &Path, preferences_file: &Path) -> io::Result<Vec<String>> {
        let _gate = self.gate.lock().unwrap();
        self.reconcile_locked(root)?;
        let mut profile = self.load_profile()?;
        let mut placed = self.load_placed(root)?;
        let mut on = Vec::new();
        let mods = melon_paths::mods(root);
        let result = (|| {
            fs::create_dir_all(&mods)?;
            for installed in &profile.mods {
                let target = mods.join(&installed.file);
                if installed.enabled {
                    if !target.exists() || hash_file(&target)? != installed.hash {
                        copy_verified(&self.path_of(installed), &target, &installed.hash)?;
                    }
                    placed.record(installed.file.clone(), installed.hash.clone());
                    on.push(installed.file.clone());
                } else if placed.hash_of(&installed.file).is_some() {
                    remove_if_present(&target)?;
                    placed.forget(&installed.file);
                }
            }
            let removed: Vec<_> = placed
                .files
                .keys()
                .filter(|file| !profile.mods.iter().any(|item| same(&item.file, file)))
                .cloned()
                .collect();
            for file in removed {
                let target = mods.join(&file);
                if target.exists() && hash_file(&target)? == placed.files[&file] {
                    fs::remove_file(&target)?;
                }
                placed.files.remove(&file);
            }
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            let _ = self.save_placed(&placed);
            return Err(error);
        }
        self.save_placed(&placed)?;
        if !profile.settings.is_empty() {
            let preferences = MelonPreferences::load(preferences_file)?;
            let mut changes = Vec::new();
            for (category, entries) in &profile.settings {
                for (key, value) in entries {
                    if let Some(value) = PreferenceValue::from_json(value)
                        && let Some(entry) = preferences.category(category).and_then(|category| category.entry(key))
                        && entry.kind.accepts(&value)
                    {
                        changes.push(PreferenceChange {
                            category: category.clone(),
                            key: key.clone(),
                            value,
                        });
                    }
                }
            }
            if !changes.is_empty() {
                preferences.save(preferences_file, &changes)?;
            }
            profile.settings.clear();
            self.save_profile(&profile)?;
        }
        self.prune(&profile);
        Ok(on)
    }
    pub fn launched(&self, root: &Path, when: jiff::Timestamp) -> io::Result<()> {
        let _gate = self.gate.lock().unwrap();
        let mut placed = self.load_placed(root)?;
        placed.last_launch = Some(when);
        self.save_placed(&placed)
    }
    fn change(&self, change: impl FnOnce(&mut ModProfile) -> io::Result<()>) -> io::Result<()> {
        let _gate = self.gate.lock().unwrap();
        let mut profile = self.load_profile()?;
        change(&mut profile)?;
        self.save_profile(&profile)
    }
    fn store(&self, source: &Path, installed: &InstalledMod) -> io::Result<()> {
        let path = self.path_of(installed);
        if !path.exists() || hash_file(&path)? != installed.hash {
            copy_verified(source, &path, &installed.hash)?;
        }
        Ok(())
    }
    fn load_profile(&self) -> io::Result<ModProfile> {
        let Some(value) = load_record(&self.directory.join("profile.json"))? else {
            return Ok(ModProfile::default());
        };
        let mut profile = ModProfile::default();
        let damaged = || invalid("profile.json is damaged: a list in it is missing.");
        let mods = field(&value, "mods").and_then(Value::as_array).ok_or_else(damaged)?;
        let mut seen = BTreeSet::new();
        for item in mods {
            let file = field(item, "file").and_then(Value::as_str).unwrap_or_default();
            let hash = field(item, "hash").and_then(Value::as_str).unwrap_or_default();
            validate_entry("profile.json", file, hash, &mut seen)?;
            let enabled = field(item, "enabled").map(Value::as_bool).unwrap_or(Some(false)).ok_or_else(damaged)?;
            profile.mods.push(InstalledMod {
                file: file.into(),
                hash: hash.into(),
                enabled,
            });
        }
        if let Some(settings) = field(&value, "settings").filter(|value| !value.is_null()) {
            for (category, entries) in settings.as_object().ok_or_else(damaged)? {
                profile.settings.insert(
                    category.clone(),
                    entries.as_object().ok_or_else(damaged)?.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
                );
            }
        }
        Ok(profile)
    }
    fn load_placed(&self, root: &Path) -> io::Result<PlacedMods> {
        let root = paths::normalized(root);
        let empty = || PlacedMods {
            root: root.clone(),
            ..Default::default()
        };
        let Some(value) = load_record(&self.directory.join("placed.json"))? else {
            return Ok(empty());
        };
        let damaged = || invalid("placed.json is damaged: a list in it is missing.");
        let saved_root = field(&value, "root").and_then(Value::as_str).ok_or_else(damaged)?;
        let mut seen = BTreeSet::new();
        let mut placed = empty();
        for (file, hash) in field(&value, "files").and_then(Value::as_object).ok_or_else(damaged)? {
            let hash = hash.as_str().unwrap_or_default();
            validate_entry("placed.json", file, hash, &mut seen)?;
            placed.files.insert(file.clone(), hash.into());
        }
        if let Some(when) = field(&value, "lastLaunch").filter(|value| !value.is_null()) {
            placed.last_launch = Some(
                when.as_str()
                    .ok_or_else(damaged)?
                    .parse()
                    .map_err(|_| invalid("placed.json is damaged: its last launch is not a date."))?,
            );
        }
        if !paths::same_text(&root.to_string_lossy(), saved_root) {
            return Ok(empty());
        }
        Ok(placed)
    }
    fn save_profile(&self, profile: &ModProfile) -> io::Result<()> {
        let mods: Vec<_> = profile
            .mods
            .iter()
            .map(|item| json!({"file":item.file, "hash":item.hash, "enabled":item.enabled}))
            .collect();
        let mut value = json!({"schemaVersion":1,"mods":mods});
        if !profile.settings.is_empty() {
            value["settings"] = json!(profile.settings);
        }
        save(&self.directory.join("profile.json"), value)
    }
    fn save_placed(&self, placed: &PlacedMods) -> io::Result<()> {
        let mut value = json!({"schemaVersion":1,"root":placed.root,"files":placed.files});
        if let Some(when) = placed.last_launch {
            value["lastLaunch"] = Value::String(when.to_string());
        }
        save(&self.directory.join("placed.json"), value)
    }
    fn prune(&self, profile: &ModProfile) {
        let library = self.directory.join("library");
        let Ok(boundary) = library.canonicalize() else {
            return;
        };
        let kept: BTreeSet<_> = profile
            .mods
            .iter()
            .map(|item| self.path_of(item).parent().unwrap().to_string_lossy().to_lowercase())
            .collect();
        let Ok(mods) = fs::read_dir(&library) else {
            return;
        };
        let plain_directory = |path: &Path| fs::symlink_metadata(path).is_ok_and(|metadata| paths::is_plain_directory(&metadata));
        for directory in mods.flatten().map(|entry| entry.path()) {
            if !plain_directory(&directory) {
                continue;
            }
            let Ok(copies) = fs::read_dir(&directory) else {
                continue;
            };
            for copy in copies.flatten().map(|entry| entry.path()) {
                if !plain_directory(&copy) {
                    continue;
                }
                if kept.contains(&copy.to_string_lossy().to_lowercase()) {
                    continue;
                }
                // Never follow a linked directory outside our own library during housekeeping.
                if let Ok(resolved) = copy.canonicalize()
                    && resolved.starts_with(&boundary)
                    && resolved != boundary
                {
                    let _ = fs::remove_dir_all(&copy);
                }
            }
            let _ = fs::remove_dir(directory); // Only succeeds if empty.
        }
    }
}

/// MelonPreferences.cfg under the folder MelonLoader runs from, for callers that name it from the library.
/// The hash the library knows a DLL by.
use eg_core::hashes::of_file as hash_file;

/// A source may be rebuilt between its initial hash and its copy. Publish only the bytes that the record names.
fn copy_verified(source: &Path, target: &Path, expected: &str) -> io::Result<()> {
    atomic::replace(target, |temporary| {
        fs::copy(source, temporary)?;
        if hash_file(temporary)? != expected {
            return Err(invalid(format!(
                "{} changed while it was being copied. Try again once the build or copy has finished.",
                source.display()
            )));
        }
        Ok(())
    })
}
fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    json::find(value.as_object()?, key)
}
fn load_record(path: &Path) -> io::Result<Option<Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let name = path.file_name().unwrap().to_string_lossy();
    let value: Value = serde_json::from_str(eg_core::text::strip_bom(&text)).map_err(|error| invalid(format!("{name} is damaged: {error}")))?;
    if value.is_null() {
        return Err(invalid(format!("{name} is empty.")));
    }
    if field(&value, "schemaVersion").and_then(Value::as_i64) != Some(1) {
        return Err(invalid(format!("{name} has an unknown version.")));
    }
    Ok(Some(value))
}
fn validate_entry(record: &str, file: &str, hash: &str, seen: &mut BTreeSet<String>) -> io::Result<()> {
    if !is_dll(file) || !hashes::is_sha256(hash) {
        return Err(invalid(format!("{record} is damaged: its entry for \"{file}\" can't be used.")));
    }
    if !seen.insert(file.to_lowercase()) {
        return Err(invalid(format!("{record} is damaged: it lists \"{file}\" more than once.")));
    }
    Ok(())
}
fn is_dll(file: &str) -> bool {
    file.len() > 4
        && file.to_ascii_lowercase().ends_with(".dll")
        && !file[..file.len() - 4].trim_end_matches(['.', ' ']).is_empty()
        && !file.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c))
}
fn mod_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(melon_paths::mods(root)) {
        Ok(files) => files
            .filter_map(|entry| match entry {
                Ok(entry) if entry.path().is_file() && is_dll(&entry.file_name().to_string_lossy()) => Some(Ok(entry.path())),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn save(path: &Path, value: Value) -> io::Result<()> {
    atomic::write(path, serde_json::to_string_pretty(&value)?.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_changed_after_hash_is_never_published_and_existing_copy_is_checked() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Changing.dll");
        fs::write(&source, b"original build").unwrap();
        let installed = InstalledMod {
            file: "Changing.dll".into(),
            hash: hash_file(&source).unwrap(),
            enabled: true,
        };
        let library = ModLibrary::new(temp.path().join("app"));
        let target = library.path_of(&installed);

        // Deterministically recreate an external rebuild after the caller hashes but before Store copies.
        fs::write(&source, b"newer build").unwrap();
        assert_eq!(library.store(&source, &installed).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(!target.exists());
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 0);
        assert!(library.profile().unwrap().mods.is_empty());

        fs::write(&source, b"original build").unwrap();
        library.store(&source, &installed).unwrap();
        fs::write(&target, b"damaged cached copy").unwrap();
        fs::write(&source, b"newer build").unwrap();
        assert!(library.store(&source, &installed).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"damaged cached copy");
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);

        // A matching source can repair the damaged cached copy, but the hash remains authoritative.
        fs::write(&source, b"original build").unwrap();
        library.store(&source, &installed).unwrap();
        assert_eq!(hash_file(&target).unwrap(), installed.hash);
    }

    #[test]
    fn corrupt_library_copy_is_not_placed_in_game_or_recorded_as_prepared() {
        let temp = tempfile::tempdir().unwrap();
        let library = ModLibrary::new(temp.path().join("app"));
        let root = temp.path().join("fake-game");
        let source = temp.path().join("Mod.dll");
        fs::write(&source, b"expected").unwrap();
        let installed = InstalledMod {
            file: "Mod.dll".into(),
            hash: hash_file(&source).unwrap(),
            enabled: true,
        };
        library.store(&source, &installed).unwrap();
        library
            .save_profile(&ModProfile {
                mods: vec![installed.clone()],
                ..Default::default()
            })
            .unwrap();
        fs::write(library.path_of(&installed), b"corrupted").unwrap();

        assert!(library.prepare(&root, &melon_paths::preferences(&root)).is_err());
        assert!(!root.join("Mods/Mod.dll").exists());
        assert!(library.placed(&root).unwrap().files.is_empty());
        assert_eq!(fs::read_dir(root.join("Mods")).unwrap().count(), 0);
        assert_eq!(library.profile().unwrap().mods, vec![installed]);
    }
}
