//! The prepared generations in the runtime folder (the C# `GenerationStore`). Each is an immutable app folder named by
//! its input key (a repaired one adds a `.repair-` suffix), with an index per key naming the folder that holds it;
//! `active.json` names the one the last start ran. Only a build adds to it, under its `build.lock`.
//!
//! The C# store also caches native helpers under `components\`; cargo's own target folder is that cache here.

use crate::HELPER_PREFIX;
use eg_core::{atomic, hashes, hosting, paths};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub struct GenerationStore {
    runtime: PathBuf,
}

impl GenerationStore {
    pub fn new(runtime: &Path) -> GenerationStore {
        GenerationStore { runtime: runtime.to_path_buf() }
    }

    /// The generation the last start ran, when it is still there; None when none is recorded.
    pub fn active_directory(&self) -> Option<PathBuf> {
        let name = self.active_name()?;
        let directory = self.runtime.join(&name);
        (is_generation(&name) && directory.is_dir()).then_some(directory)
    }

    /// Staging and trash folders: under the build lock, any found were left by an interrupted run.
    pub fn remove_leftovers(&self) {
        for entry in fs::read_dir(&self.runtime).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with(".stage-") || name.starts_with(".trash-")) && entry.path().is_dir() {
                try_delete(&entry.path());
            }
        }
    }

    pub fn new_staging(&self) -> std::io::Result<PathBuf> {
        let staging = self.runtime.join(format!(".stage-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]));
        fs::create_dir_all(&staging)?;
        Ok(staging)
    }

    /// The folder that holds the key's generation: the one its index names (a repaired folder), else the key's own.
    pub fn locate(&self, key: &str) -> PathBuf {
        let saved = fs::read_to_string(self.runtime.join(format!("{key}.json")))
            .ok()
            .and_then(|text| serde_json::from_str::<String>(&text).ok())
            .filter(|saved| Path::new(saved).file_name().is_some_and(|name| name == saved.as_str()) && saved.starts_with(key));
        self.runtime.join(saved.unwrap_or_else(|| key.to_owned()))
    }

    /// A valid generation about to run becomes the active one, so the next build keeps it as the previous generation.
    pub fn use_generation(&self, key: &str, directory: &Path) -> std::io::Result<()> {
        if self.active_name().as_deref() != directory.file_name().and_then(|name| name.to_str()) {
            self.write_active(key, directory)?;
        }
        Ok(())
    }

    /// Moves a finished app folder in as the key's generation and makes it active. An invalid folder already there,
    /// which may be loaded, is left intact and the new one goes beside it as a repair. Generations older than the
    /// previous one go. Returns where the generation now is.
    pub fn add(&self, app: &Path, key: &str, destination: &Path) -> std::io::Result<PathBuf> {
        let mut destination = destination.to_path_buf();
        if destination.exists() {
            let mut name = destination.file_name().unwrap_or_default().to_owned();
            name.push(format!(".repair-{}", uuid::Uuid::new_v4().simple()));
            destination.set_file_name(name);
        }
        let previous = self.active_name();
        fs::rename(app, &destination)?;
        let name = file_name(&destination);
        atomic::write(&self.runtime.join(format!("{key}.json")), json!(name).to_string().as_bytes())?;
        self.write_active(key, &destination)?;
        self.prune(&name, previous.as_deref());
        Ok(destination)
    }

    fn active_name(&self) -> Option<String> {
        let text = fs::read_to_string(self.runtime.join("active.json")).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        value.get("directory")?.as_str().map(str::to_owned)
    }

    fn write_active(&self, key: &str, directory: &Path) -> std::io::Result<()> {
        let active = json!({ "key": key, "directory": file_name(directory) });
        atomic::write(&self.runtime.join("active.json"), active.to_string().as_bytes())
    }

    /// Removes generations other than the new and the previous one, with their index files. A generation still in use
    /// (an app running from it, or a game holding its helper) stays until a later build.
    fn prune(&self, current: &str, previous: Option<&str>) {
        for entry in fs::read_dir(&self.runtime).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let generation = entry.path();
            if name == current || Some(name.as_str()) == previous || !is_generation(&name) || !generation.is_dir() || in_use(&generation) {
                continue;
            }
            // Renamed first, so an interrupted removal is never mistaken for a generation and is retried by the next build.
            let trash = self.runtime.join(format!(".trash-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]));
            if fs::rename(&generation, &trash).is_ok() {
                try_delete(&trash);
            }
        }
        let names: Vec<String> = fs::read_dir(&self.runtime)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        for entry in fs::read_dir(&self.runtime).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(key) = name.strip_suffix(".json") else { continue };
            // An index without its generation names nothing; a failed removal is retried by the next build.
            if is_generation(key) && !names.iter().any(|folder| folder.starts_with(key)) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Whether a process runs or has loaded a generation's files: the app's executable or a helper DLL. Windows lets a
/// folder holding a running exe or a loaded DLL be renamed, but not the file itself be opened for writing.
fn in_use(generation: &Path) -> bool {
    let helpers = fs::read_dir(generation)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(HELPER_PREFIX)) && path.extension().is_some_and(|extension| extension == "dll"));
    std::iter::once(generation.join(paths::EXECUTABLE)).chain(helpers).any(|file| file_in_use(&file))
}

fn file_in_use(file: &Path) -> bool {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    match options.open(file) {
        Ok(_) => false,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// A generation folder is named by its input key, and a repaired one adds a `.repair-` suffix.
pub fn is_generation(name: &str) -> bool {
    name.len() >= 64 && name.is_char_boundary(64) && hashes::is_sha256(&name[..64]) && (name.len() == 64 || name[64..].starts_with(".repair-"))
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Cleanup never fails preparation; an interrupted removal is retried by the next build.
pub fn try_delete(directory: &Path) {
    if directory.is_dir() {
        let _ = fs::remove_dir_all(directory);
    }
}

/// The manifest's required files, as the app checks them too (`crates/app/src/main.rs`).
pub fn required_files() -> [&'static str; 2] {
    [paths::EXECUTABLE, crate::HELPER_MANIFEST]
}

/// Whether `directory` holds the key's generation, whole.
pub fn valid(directory: &Path, key: &str) -> bool {
    hosting::valid(directory, key, &required_files())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_core::hosting::Manifest;
    use std::collections::BTreeMap;

    fn key(n: u8) -> String {
        hashes::of_bytes(&[n])
    }

    /// A finished app folder for `key`, as a build leaves it in staging.
    fn app(runtime: &Path, key: &str) -> PathBuf {
        let app = runtime.join(format!(".stage-{}", &key[..8])).join("app");
        fs::create_dir_all(&app).unwrap();
        let mut files = BTreeMap::new();
        for name in required_files().into_iter().chain([&*format!("{HELPER_PREFIX}-{key}.dll")]) {
            fs::write(app.join(name), name).unwrap();
            files.insert(name.to_owned(), hashes::of_text(name));
        }
        Manifest { key: key.into(), files }.write(&app).unwrap();
        app
    }

    fn add(store: &GenerationStore, runtime: &Path, key: &str) -> PathBuf {
        store.add(&app(runtime, key), key, &store.locate(key)).unwrap()
    }

    #[test]
    fn generation_names_are_keys_or_their_repairs() {
        assert!(is_generation(&key(1)));
        assert!(is_generation(&format!("{}.repair-abc", key(1))));
        assert!(!is_generation(&format!("{}.json", key(1))));
        assert!(!is_generation(".stage-1234567890"));
        assert!(!is_generation("components"));
    }

    #[test]
    fn a_new_generation_becomes_active_and_keeps_only_the_previous_one() {
        let temp = tempfile::tempdir().unwrap();
        let store = GenerationStore::new(temp.path());
        assert_eq!(store.active_directory(), None);
        let first = add(&store, temp.path(), &key(1));
        assert_eq!(first, temp.path().join(key(1)));
        assert!(valid(&first, &key(1)));
        assert_eq!(store.active_directory(), Some(first.clone()));
        let second = add(&store, temp.path(), &key(2));
        let third = add(&store, temp.path(), &key(3));
        assert_eq!(store.active_directory(), Some(third.clone()));
        assert!(second.is_dir(), "the previous generation stays");
        assert!(!first.exists(), "older ones go");
        assert!(!temp.path().join(format!("{}.json", key(1))).exists(), "with their index");
        assert!(temp.path().join(format!("{}.json", key(2))).exists());
    }

    #[test]
    fn a_generation_in_use_is_kept_until_a_later_build() {
        let temp = tempfile::tempdir().unwrap();
        let store = GenerationStore::new(temp.path());
        let first = add(&store, temp.path(), &key(1));
        // An app running from it (or a game holding its helper) holds the file open.
        let running = fs::File::open(first.join(paths::EXECUTABLE)).unwrap();
        add(&store, temp.path(), &key(2));
        add(&store, temp.path(), &key(3));
        assert!(first.is_dir(), "in use: kept");
        drop(running);
        add(&store, temp.path(), &key(4));
        assert!(!first.exists(), "removed by the next build once free");
    }

    #[test]
    fn an_invalid_folder_in_the_way_is_left_and_the_repair_is_located_by_its_index() {
        let temp = tempfile::tempdir().unwrap();
        let store = GenerationStore::new(temp.path());
        let broken = temp.path().join(key(1));
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join(paths::EXECUTABLE), "half written").unwrap();
        // It may be loaded: an app still runs from it.
        let running = fs::File::open(broken.join(paths::EXECUTABLE)).unwrap();
        assert_eq!(store.locate(&key(1)), broken);
        let repaired = add(&store, temp.path(), &key(1));
        assert!(repaired.file_name().unwrap().to_string_lossy().starts_with(&format!("{}.repair-", key(1))));
        assert_eq!(store.locate(&key(1)), repaired);
        assert!(valid(&repaired, &key(1)));
        assert!(broken.is_dir(), "the folder in the way is never overwritten, and stays while in use");
        drop(running);
        add(&store, temp.path(), &key(2));
        add(&store, temp.path(), &key(3));
        assert!(!broken.exists(), "a later build removes it");
    }

    #[test]
    fn an_index_naming_another_place_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let store = GenerationStore::new(temp.path());
        for saved in [r"..\elsewhere", "other", r"C:\x"] {
            fs::write(temp.path().join(format!("{}.json", key(1))), json!(saved).to_string()).unwrap();
            assert_eq!(store.locate(&key(1)), temp.path().join(key(1)), "{saved}");
        }
    }

    #[test]
    fn leftovers_of_an_interrupted_run_are_removed() {
        let temp = tempfile::tempdir().unwrap();
        let store = GenerationStore::new(temp.path());
        let staging = store.new_staging().unwrap();
        fs::create_dir_all(temp.path().join(".trash-0123456789")).unwrap();
        fs::create_dir_all(temp.path().join(key(1))).unwrap();
        store.remove_leftovers();
        assert!(!staging.exists());
        assert!(!temp.path().join(".trash-0123456789").exists());
        assert!(temp.path().join(key(1)).exists());
    }

    #[test]
    fn using_a_generation_makes_it_active() {
        let temp = tempfile::tempdir().unwrap();
        let store = GenerationStore::new(temp.path());
        let first = add(&store, temp.path(), &key(1));
        add(&store, temp.path(), &key(2));
        store.use_generation(&key(1), &first).unwrap();
        assert_eq!(store.active_directory(), Some(first));
    }
}
