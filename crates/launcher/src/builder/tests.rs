use super::*;
use serde_json::json;
use std::sync::Mutex;

/// A checkout with one crate, and build output and data beside its sources.
fn checkout() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (file, contents) in [
        ("Cargo.toml", "[workspace]"),
        ("rust-toolchain.toml", "[toolchain]"),
        ("Cargo.lock", "# lock"),
        ("crates/app/src/main.rs", "fn main() {}"),
        ("crates/app/target/debug/out.txt", "build output"),
        (".cargo/config.toml", "[alias]"),
        ("target/release/app.exe", "built"),
        ("artifacts/captures/a.png", "png"),
        ("data/settings.json", "{}"),
        ("design/tokens.json", "{}"),
        ("PORT.md", "notes"),
    ] {
        fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        fs::write(root.join(file), contents).unwrap();
    }
    temp
}

/// Builds nothing: answers rustc and cargo as they would, with files of its own in a fake target folder.
struct FakeCargo {
    target: PathBuf,
    calls: Mutex<Vec<String>>,
    /// Edited into the checkout while the app builds.
    edit_during_build: Option<PathBuf>,
    fail: bool,
}

impl FakeCargo {
    fn new(target: &Path) -> FakeCargo {
        FakeCargo {
            target: target.to_path_buf(),
            calls: Mutex::new(Vec::new()),
            edit_during_build: None,
            fail: false,
        }
    }

    fn builds(&self) -> usize {
        self.calls.lock().unwrap().iter().filter(|call| call.starts_with("cargo build")).count()
    }
}

impl ProcessRunner for &FakeCargo {
    fn run(&self, request: &ProcessRequest, output: Output<'_>, _cancel: &Cancellation) -> Result<String, Failure> {
        let call = format!("{} {}", request.program, request.argument_text().join(" "));
        self.calls.lock().unwrap().push(call.clone());
        if request.program == "rustc" {
            return Ok("rustc 1.101.0-nightly\nhost: x86_64-pc-windows-msvc".into());
        }
        assert!(request.quiet_output, "cargo's JSON is read, not shown");
        assert!(!call.contains("-p eg-trainer-helper"), "the desktop builds its helper itself: {call}");
        output("   Compiling eg-core v0.1.0");
        let release = self.target.join("release");
        fs::create_dir_all(&release).unwrap();
        if let Some(file) = &self.edit_during_build {
            fs::write(file, "edited while it built").unwrap();
        }
        if self.fail {
            return Err(Failure::Failed("cargo exited with 101.\nerror[E0425]: cannot find value `x` in this scope".into()));
        }
        fs::write(release.join(paths::EXECUTABLE), "desktop").unwrap();
        let artifact = json!({"reason": "compiler-artifact", "target": {"name": "eterras-gift-desktop"}, "executable": release.join(paths::EXECUTABLE)});
        Ok(format!("{{\"reason\":\"build-finished\"}}\n{artifact}"))
    }
}

struct Run {
    stages: Vec<(Stage, String)>,
    lines: Vec<String>,
    result: Result<Prepared, Failure>,
}

fn prepare(cargo: &FakeCargo, root: &Path, runtime: &Path) -> Run {
    let stages = Mutex::new(Vec::new());
    let lines = Mutex::new(Vec::new());
    let result = GenerationBuilder::new(cargo, runtime).prepare(
        root,
        &root.join("install"),
        &|stage, message| stages.lock().unwrap().push((stage, message.to_owned())),
        &|line| lines.lock().unwrap().push(line.to_owned()),
        &Cancellation::default(),
    );
    Run {
        stages: stages.into_inner().unwrap(),
        lines: lines.into_inner().unwrap(),
        result,
    }
}

#[test]
fn the_input_key_covers_the_build_sources_and_the_toolchain_only() {
    let temp = checkout();
    let root = temp.path();
    let cancel = Cancellation::default();
    let key = input_key(root, "tools", &cancel).unwrap();
    assert!(hashes::is_sha256(&key));
    assert_eq!(key, input_key(root, "tools", &cancel).unwrap(), "stable");
    assert_ne!(key, input_key(root, "other tools", &cancel).unwrap(), "another toolchain");
    for excluded in [
        "crates/app/target/debug/out.txt",
        "target/release/app.exe",
        "artifacts/captures/a.png",
        "data/settings.json",
        "design/tokens.json",
        "PORT.md",
    ] {
        fs::write(root.join(excluded), "changed").unwrap();
        assert_eq!(key, input_key(root, "tools", &cancel).unwrap(), "{excluded} is not a build input");
    }
    for included in ["crates/app/src/main.rs", ".cargo/config.toml", "Cargo.lock", "rust-toolchain.toml"] {
        let before = input_key(root, "tools", &cancel).unwrap();
        fs::write(root.join(included), format!("changed {included}")).unwrap();
        assert_ne!(before, input_key(root, "tools", &cancel).unwrap(), "{included} is a build input");
    }
    let before = input_key(root, "tools", &cancel).unwrap();
    fs::write(root.join("crates/app/src/new.rs"), "").unwrap();
    assert_ne!(before, input_key(root, "tools", &cancel).unwrap(), "a new file counts, even empty");
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(input_key(root, "tools", &cancelled), Err(Failure::Cancelled));
}

#[test]
fn a_checkout_builds_once_and_then_reuses_its_generation() {
    let temp = checkout();
    let root = temp.path();
    let runtime = root.join("runtime");
    let cargo = FakeCargo::new(&root.join("fake-target"));
    let first = prepare(&cargo, root, &runtime);
    let prepared = first.result.unwrap();
    assert!(prepared.built);
    assert_eq!(cargo.builds(), 1, "the app, which builds and embeds its helper");
    assert_eq!(
        first.stages,
        [
            (Stage::Checking, "Checking application components".into()),
            (Stage::Building, "Building application and native helper".into()),
            (Stage::Ready, "Application components prepared".into()),
        ]
    );
    assert!(first.lines.iter().any(|line| line.contains("Compiling")), "the build's lines are reported");
    assert!(first.lines.iter().all(|line| !line.starts_with('{')), "cargo's JSON is not");
    assert_eq!(prepared.directory, runtime.join(&prepared.key));
    assert!(store::valid(&prepared.directory, &prepared.key));
    let files = Manifest::read(&prepared.directory).unwrap().files;
    assert_eq!(files.keys().map(String::as_str).collect::<Vec<_>>(), [paths::EXECUTABLE]);
    assert_eq!(GenerationStore::new(&runtime).active_directory(), Some(prepared.directory.clone()));

    let second = prepare(&cargo, root, &runtime);
    assert_eq!(second.result.as_ref().unwrap(), &Prepared { built: false, ..prepared.clone() });
    assert_eq!(cargo.builds(), 1, "nothing built again");
    assert_eq!(second.stages.last().unwrap(), &(Stage::Ready, "Using prepared application components".into()));

    // A changed source is a new generation; the previous stays beside it.
    fs::write(root.join("crates/app/src/main.rs"), "fn main() { /* new */ }").unwrap();
    let third = prepare(&cargo, root, &runtime).result.unwrap();
    assert!(third.built && third.key != prepared.key);
    assert!(prepared.directory.is_dir());

    // A generation tampered with is not used: it is built again beside it.
    fs::write(third.directory.join(paths::EXECUTABLE), "tampered").unwrap();
    let repaired = prepare(&cargo, root, &runtime).result.unwrap();
    assert!(repaired.built && repaired.key == third.key && repaired.directory != third.directory);
    assert_eq!(GenerationStore::new(&runtime).locate(&third.key), repaired.directory);
}

#[test]
fn source_changed_during_the_build_fails_and_keeps_the_previous_generation() {
    let temp = checkout();
    let root = temp.path();
    let runtime = root.join("runtime");
    let previous = prepare(&FakeCargo::new(&root.join("fake-target")), root, &runtime).result.unwrap();
    fs::write(root.join("crates/app/src/main.rs"), "fn main() { /* next */ }").unwrap();
    let mut cargo = FakeCargo::new(&root.join("fake-target"));
    cargo.edit_during_build = Some(root.join("crates/app/src/main.rs"));
    let run = prepare(&cargo, root, &runtime);
    assert_eq!(run.result, Err(Failure::Failed("Source changed during preparation. Retry to build the new source.".into())));
    assert_eq!(
        run.stages.last().unwrap(),
        &(Stage::Blocked, "Preparation failed. Retry after resolving the reported problem.".into())
    );
    let store = GenerationStore::new(&runtime);
    assert_eq!(store.active_directory(), Some(previous.directory));
    assert!(
        fs::read_dir(&runtime)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".stage-")),
        "staging is removed"
    );
}

#[test]
fn a_failed_build_is_blocked_with_the_tool_output() {
    let temp = checkout();
    let root = temp.path();
    let mut cargo = FakeCargo::new(&root.join("fake-target"));
    cargo.fail = true;
    let run = prepare(&cargo, root, &root.join("runtime"));
    let Err(Failure::Failed(message)) = run.result else { panic!("{:?}", run.result) };
    assert!(message.contains("error[E0425]"));
    assert_eq!(run.stages.last().unwrap().0, Stage::Blocked);
    assert_eq!(GenerationStore::new(&root.join("runtime")).active_directory(), None);
}

#[test]
fn an_install_runs_beside_the_launcher_without_a_build() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let install = root.join("install");
    fs::create_dir_all(&install).unwrap();
    let cargo = FakeCargo::new(&root.join("fake-target"));
    let run = prepare(&cargo, root, &root.join("runtime"));
    assert_eq!(run.result, Err(Failure::Failed("The application package is incomplete. Reinstall Eterra's Gift.".into())));
    for file in store::required_files() {
        fs::write(install.join(file), "").unwrap();
    }
    let run = prepare(&cargo, root, &root.join("runtime"));
    assert_eq!(
        run.result,
        Ok(Prepared {
            directory: install,
            key: hosting::PACKAGED.into(),
            built: false
        })
    );
    assert!(cargo.calls.lock().unwrap().is_empty(), "no tool runs for an install");
}

#[test]
fn a_second_preparation_waits_for_the_build_lock() {
    let temp = tempfile::tempdir().unwrap();
    let lock = temp.path().join("build.lock");
    let held = acquire_lock(&lock, &Cancellation::default()).unwrap();
    let cancel = Cancellation::default();
    let waiting = std::thread::spawn({
        let lock = lock.clone();
        let cancel = cancel.clone();
        move || acquire_lock(&lock, &cancel).map(drop)
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(!waiting.is_finished(), "waits while another preparation holds it");
    drop(held);
    assert_eq!(waiting.join().unwrap(), Ok(()));
    let _held = acquire_lock(&lock, &Cancellation::default()).unwrap();
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(acquire_lock(&lock, &cancelled).map(drop), Err(Failure::Cancelled));
    drop(cancel);
}

#[test]
fn cargo_messages_name_the_artifacts() {
    let helper = r#"{"reason":"compiler-artifact","target":{"name":"eg_trainer_helper"},"filenames":["C:/t/eg_trainer_helper.dll"]}"#;
    assert!(desktop_artifact(helper).is_err());
    let desktop = r#"{"reason":"compiler-artifact","target":{"name":"eterras-gift-desktop"},"executable":"C:/t/eterras-gift-desktop.exe"}"#;
    assert_eq!(desktop_artifact(desktop).unwrap(), PathBuf::from("C:/t/eterras-gift-desktop.exe"));
}
