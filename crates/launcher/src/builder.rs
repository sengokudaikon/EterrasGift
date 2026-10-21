//! Builds immutable generations from a checkout with cargo, into a [`GenerationStore`] (the C# `GenerationBuilder`).
//! Failed or cancelled builds never become active.
//!
//! Translated from the C# launcher: `dotnet publish` of the desktop becomes `cargo build --release -p eterras-gift`. The
//! trainer helper the C# launcher built with CMake/MSVC is an artifact dependency of the desktop, embedded in its exe,
//! so one build makes both. The pinned toolchain's identity is `rustc -Vv`. cargo's target folder is the build cache
//! the C# store kept under `components\`.

use crate::process::{Failure, Output, ProcessRequest, ProcessRunner};
use crate::store::{self, GenerationStore};
use eg_core::hosting::{self, Manifest};
use eg_core::operations::Cancellation;
use eg_core::{hashes, paths};
use eg_ui::preparation::Stage;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A generation ready to run: where, its key (`packaged` in an install), and whether this preparation built it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub directory: PathBuf,
    pub key: String,
    pub built: bool,
}

/// Where preparation is, in the C# launcher's words.
pub type Progress<'a> = &'a dyn Fn(Stage, &str);

/// What makes a generation ready; the launcher's supervisor runs it, tests stand in their own.
pub trait Preparer {
    fn prepare(&self, root: &Path, packaged: &Path, progress: Progress<'_>, output: Output<'_>, cancel: &Cancellation) -> Result<Prepared, Failure>;
}

pub struct GenerationBuilder<R> {
    runner: R,
    runtime: PathBuf,
}

impl<R: ProcessRunner> GenerationBuilder<R> {
    pub fn new(runner: R, runtime: &Path) -> GenerationBuilder<R> {
        GenerationBuilder {
            runner,
            runtime: runtime.to_path_buf(),
        }
    }

    fn build(&self, root: &Path, key: &str, tools: &str, staging: &Path, output: Output<'_>, cancel: &Cancellation) -> Result<PathBuf, Failure> {
        let app = staging.join("app");
        fs::create_dir_all(&app)?;
        // The desktop builds its trainer helper too, and carries it in its exe.
        let messages = self.cargo(
            root,
            &["build", "--release", "-p", "eterras-gift", "--message-format=json-render-diagnostics"],
            output,
            cancel,
        )?;
        let executable = desktop_artifact(&messages)?;
        fs::copy(&executable, app.join(paths::EXECUTABLE))?;
        if key != input_key(root, tools, cancel)? {
            return Err(Failure::Failed("Source changed during preparation. Retry to build the new source.".into()));
        }
        let mut files = BTreeMap::new();
        for entry in fs::read_dir(&app)? {
            let path = entry?.path();
            if path.is_file() {
                files.insert(path.file_name().unwrap_or_default().to_string_lossy().into_owned(), hosting::hash_file(&path)?);
            }
        }
        Manifest { key: key.to_owned(), files }.write(&app)?;
        Ok(app)
    }

    fn cargo(&self, root: &Path, arguments: &[&str], output: Output<'_>, cancel: &Cancellation) -> Result<String, Failure> {
        self.runner.run(&ProcessRequest::new("cargo", arguments, root.to_path_buf()).quiet(), output, cancel)
    }
}

impl<R: ProcessRunner> Preparer for GenerationBuilder<R> {
    fn prepare(&self, root: &Path, packaged: &Path, progress: Progress<'_>, output: Output<'_>, cancel: &Cancellation) -> Result<Prepared, Failure> {
        progress(Stage::Checking, "Checking application components");
        if !paths::is_checkout(root) {
            if store::required_files().iter().any(|file| !packaged.join(file).is_file()) {
                return Err(Failure::Failed("The application package is incomplete. Reinstall Eterra's Gift.".into()));
            }
            return Ok(Prepared {
                directory: packaged.to_path_buf(),
                key: hosting::PACKAGED.into(),
                built: false,
            });
        }
        let tools = self.runner.run(&ProcessRequest::new("rustc", &["-Vv"], root.to_path_buf()), output, cancel)?;
        fs::create_dir_all(&self.runtime)?;
        // Two first launches take turns at building.
        let _lock = acquire_lock(&self.runtime.join("build.lock"), cancel)?;
        let key = input_key(root, &tools, cancel)?;
        let store = GenerationStore::new(&self.runtime);
        store.remove_leftovers();
        let destination = store.locate(&key);
        if store::valid(&destination, &key) {
            store.use_generation(&key, &destination)?;
            progress(Stage::Ready, "Using prepared application components");
            return Ok(Prepared {
                directory: destination,
                key,
                built: false,
            });
        }
        let staging = store.new_staging()?;
        progress(Stage::Building, "Building application and native helper");
        let result = self
            .build(root, &key, &tools, &staging, output, cancel)
            .and_then(|app| if cancel.is_cancelled() { Err(Failure::Cancelled) } else { Ok(app) })
            .and_then(|app| store.add(&app, &key, &destination).map_err(Failure::from));
        store::try_delete(&staging);
        match result {
            Ok(directory) => {
                progress(Stage::Ready, "Application components prepared");
                Ok(Prepared { directory, key, built: true })
            }
            Err(failure) => {
                progress(Stage::Blocked, "Preparation failed. Retry after resolving the reported problem.");
                Err(failure)
            }
        }
    }
}

/// The app's executable as cargo built it.
fn desktop_artifact(messages: &str) -> Result<PathBuf, Failure> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact" && message["target"]["name"] == paths::EXECUTABLE.trim_end_matches(".exe"))
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
        .ok_or_else(|| Failure::Failed("The application build did not report its executable.".into()))
}

/// The folders and files a build reads, under the root: the workspace's crates and cargo's configuration, and its
/// manifest, lock file and toolchain pin. As the C# key covers `src`, `native`, the vendored MinHook and the solution
/// files, and not the docs, design sources or user data beside them.
const INPUT_FOLDERS: [&str; 2] = ["crates", ".cargo"];
const INPUT_FILES: [&str; 3] = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"];
/// Build output and data, wherever they sit: never a source.
const EXCLUDED: [&str; 4] = ["target", ".git", "artifacts", "data"];

/// The SHA-256 of every source file a build reads, by its `/`-separated path, and of the tools' identity.
pub fn input_key(root: &Path, tools: &str, cancel: &Cancellation) -> Result<String, Failure> {
    let mut files = Vec::new();
    for folder in INPUT_FOLDERS {
        collect(root, &root.join(folder), &mut files);
    }
    files.extend(INPUT_FILES.iter().map(|file| ((*file).to_owned(), root.join(file))).filter(|(_, path)| path.is_file()));
    files.sort_by(|first, second| first.0.cmp(&second.0));
    let mut hash = Sha256::new();
    hash.update(format!("generation-v1:{tools}").as_bytes());
    for (relative, path) in files {
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        // Gone since the listing (an editor's safe-write temporary file), so not part of the source.
        let Ok(contents) = fs::read(&path) else { continue };
        hash.update(relative.as_bytes());
        hash.update(&contents);
    }
    Ok(hashes::hex_upper(hash.finalize().as_slice()))
}

fn collect(root: &Path, folder: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if !EXCLUDED.iter().any(|name| entry.file_name() == *name) {
                collect(root, &path, files);
            }
        } else if kind.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            files.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

/// Waits while another preparation holds the lock; any other failure (a bad path, a full disk) is said at once.
fn acquire_lock(path: &Path, cancel: &Cancellation) -> Result<fs::File, Failure> {
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    loop {
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(250)),
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
