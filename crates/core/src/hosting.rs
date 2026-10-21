//! What the launcher and the app agree on (`spec/architecture.md` §1.1–1.2): the exit code that asks for a restart, the
//! policy that takes it, and a generation's `generation.json`, which the launcher writes and the app checks its own
//! generation with. The C# app keeps these in `Application/Hosting` and `Infrastructure/Hosting`.

use crate::{atomic, hashes};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::{Component, Path};
use std::time::Duration;

/// The exit code with which the app asks its launcher to prepare and start it again (Settings' Restart now, or a
/// generation that failed its own check).
pub const REQUESTED_EXIT_CODE: i32 = 42;

/// How long the app must have run for its restart request to be the user's rather than a start that failed.
pub const MINIMUM_RUN: Duration = Duration::from_secs(10);

/// The launcher's executable: the entry point of an install, beside the app's (`paths::EXECUTABLE`).
pub const LAUNCHER_EXECUTABLE: &str = "eterras-gift.exe";

/// The manifest each prepared generation holds.
pub const MANIFEST: &str = "generation.json";

/// The key of a generation that runs beside the launcher in an install: no build and no manifest check.
pub const PACKAGED: &str = "packaged";

/// Only an explicit request restarts the app: the user's Restart now in Settings, or a generation that failed its own
/// check; ordinary exits and faults do not. A request right after the app started counts once per generation, so a
/// generation that keeps asking stops instead of looping; one after the app has run a while (the user's) is always taken.
#[derive(Debug, Default)]
pub struct RestartPolicy {
    restarted: HashSet<String>,
}

impl RestartPolicy {
    pub fn should_restart(&mut self, exit_code: i32, generation: &str, ran: Duration) -> bool {
        exit_code == REQUESTED_EXIT_CODE && (ran >= MINIMUM_RUN || self.restarted.insert(generation.to_owned()))
    }
}

/// What a generation holds, as its `generation.json` lists it: each file with its SHA-256 (paths relative to the
/// generation, `/`-separated), and the input key it was built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub key: String,
    pub files: BTreeMap<String, String>,
}

impl Manifest {
    pub fn to_json(&self) -> String {
        let files: Map<String, Value> = self.files.iter().map(|(path, hash)| (path.clone(), Value::from(hash.as_str()))).collect();
        let mut root = Map::new();
        root.insert("key".into(), self.key.as_str().into());
        root.insert("files".into(), Value::Object(files));
        serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Manifest> {
        let value: Value = serde_json::from_str(text).ok()?;
        let key = value.get("key")?.as_str()?.to_owned();
        let files = value
            .get("files")?
            .as_object()?
            .iter()
            .map(|(path, hash)| hash.as_str().map(|hash| (path.clone(), hash.to_owned())))
            .collect::<Option<_>>()?;
        Some(Manifest { key, files })
    }

    /// Reads a generation's manifest; None when it is missing or unreadable.
    pub fn read(directory: &Path) -> Option<Manifest> {
        Manifest::parse(&std::fs::read_to_string(directory.join(MANIFEST)).ok()?)
    }

    /// Writes the manifest into `directory` in one step.
    pub fn write(&self, directory: &Path) -> std::io::Result<()> {
        atomic::write(&directory.join(MANIFEST), self.to_json().as_bytes())
    }
}

/// The SHA-256 of a file's contents, as the manifest spells it.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    hashes::of_reader(File::open(path)?)
}

/// Whether `directory` holds the generation built from `key`: its manifest names that key and lists every `required`
/// file, and every file it lists lies inside the folder and matches its SHA-256. A path Windows can't take, or a file it
/// may not read, makes the generation invalid rather than the start fail.
pub fn valid(directory: &Path, key: &str, required: &[&str]) -> bool {
    let Some(manifest) = Manifest::read(directory) else {
        return false;
    };
    if manifest.key != key || required.iter().any(|file| !manifest.files.contains_key(*file)) {
        return false;
    }
    manifest.files.iter().all(|(file, hash)| {
        let relative = Path::new(file);
        // Only plain names inside the folder: no root, drive, `..` or `.`.
        let inside = relative.components().all(|part| matches!(part, Component::Normal(_)));
        inside && hash_file(&directory.join(relative)).is_ok_and(|actual| actual == *hash)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_request_restarts_and_a_quick_one_once_per_generation() {
        let mut policy = RestartPolicy::default();
        let quick = Duration::from_secs(1);
        assert!(!policy.should_restart(0, "A", MINIMUM_RUN));
        assert!(!policy.should_restart(1, "A", MINIMUM_RUN));
        assert!(!policy.should_restart(-1, "A", quick));
        assert!(policy.should_restart(REQUESTED_EXIT_CODE, "A", quick));
        assert!(!policy.should_restart(REQUESTED_EXIT_CODE, "A", quick), "a generation that keeps asking stops");
        assert!(policy.should_restart(REQUESTED_EXIT_CODE, "B", quick), "another generation has its own once");
        assert!(
            policy.should_restart(REQUESTED_EXIT_CODE, "A", MINIMUM_RUN),
            "the user's request after a run is always taken"
        );
        assert!(policy.should_restart(REQUESTED_EXIT_CODE, "A", MINIMUM_RUN));
    }

    fn generation(directory: &Path, key: &str) -> Manifest {
        std::fs::create_dir_all(directory).unwrap();
        let mut files = BTreeMap::new();
        for (name, contents) in [("app.exe", "app"), ("helper.json", "{}")] {
            std::fs::write(directory.join(name), contents).unwrap();
            files.insert(name.to_owned(), hashes::of_text(contents));
        }
        let manifest = Manifest { key: key.into(), files };
        manifest.write(directory).unwrap();
        manifest
    }

    #[test]
    fn a_generation_is_valid_only_whole_and_for_its_own_key() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("gen");
        let manifest = generation(&directory, "KEY");
        assert_eq!(Manifest::read(&directory), Some(manifest.clone()));
        assert!(valid(&directory, "KEY", &["app.exe", "helper.json"]));
        assert!(!valid(&directory, "OTHER", &["app.exe"]), "another key");
        assert!(!valid(&directory, "KEY", &["app.exe", "missing.dll"]), "a required file it does not list");
        assert!(!valid(&temp.path().join("none"), "KEY", &[]), "no manifest");

        std::fs::write(directory.join("app.exe"), "changed").unwrap();
        assert!(!valid(&directory, "KEY", &["app.exe"]), "a file that no longer matches");
        std::fs::write(directory.join("app.exe"), "app").unwrap();
        std::fs::remove_file(directory.join("helper.json")).unwrap();
        assert!(!valid(&directory, "KEY", &["app.exe"]), "a listed file that is gone");
    }

    #[test]
    fn a_manifest_never_reaches_outside_its_folder() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("gen");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(temp.path().join("outside.txt"), "x").unwrap();
        for path in ["../outside.txt", "./outside.txt", r"C:\Windows\win.ini", "/outside.txt"] {
            let manifest = Manifest {
                key: "KEY".into(),
                files: BTreeMap::from([(path.to_owned(), hashes::of_text("x"))]),
            };
            manifest.write(&directory).unwrap();
            assert!(!valid(&directory, "KEY", &[]), "{path}");
        }
        std::fs::write(directory.join(MANIFEST), "not json").unwrap();
        assert!(!valid(&directory, "KEY", &[]));
    }
}
