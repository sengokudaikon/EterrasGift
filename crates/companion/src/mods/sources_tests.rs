use super::*;
use serde_json::json;
use std::sync::mpsc;
use std::time::Instant;

#[derive(Default)]
struct Runner {
    calls: Mutex<Vec<ProcessRequest>>,
    failure: Mutex<Option<ProcessError>>,
    answer: Mutex<Option<String>>,
    imports: Mutex<Vec<PathBuf>>,
    held: AtomicBool,
    started: AtomicUsize,
    active: AtomicUsize,
    maximum: AtomicUsize,
    omit_output: AtomicBool,
}

impl ProcessRunner for Runner {
    fn run(&self, request: &ProcessRequest, cancel: &Cancellation) -> Result<String, ProcessError> {
        self.calls.lock().unwrap().push(request.clone());
        self.started.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        while self.held.load(Ordering::Acquire) {
            if cancel.is_cancelled() {
                self.active.fetch_sub(1, Ordering::SeqCst);
                return Err(ProcessError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        let project = Path::new(&request.arguments[1]);
        let target = target(project);
        if request.arguments[0] == "msbuild" {
            if let Some(answer) = self.answer.lock().unwrap().clone() {
                return Ok(answer);
            }
            let mod_project = !project.file_stem().unwrap().to_string_lossy().starts_with("Tool");
            let imports = self.imports.lock().unwrap().iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(";");
            Ok(json!({
                "Properties": {
                    "TargetPath": target,
                    "OutputType": if mod_project { "Library" } else { "Exe" },
                    "AssemblyName": project.file_stem().unwrap().to_string_lossy(),
                    "Version": "1.0.0",
                    "MSBuildAllProjects": imports,
                },
                "Items": { "Reference": [{ "Filename": "MelonLoader" }] },
            })
            .to_string())
        } else {
            if !self.omit_output.load(Ordering::Relaxed) {
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(target, "a fake build; never loaded").unwrap();
            }
            Ok(String::new())
        }
    }
}

fn target(project: &Path) -> PathBuf {
    project.parent().unwrap().join("bin/Release").join(project.file_stem().unwrap()).with_extension("dll")
}

fn project(root: &Path, relative: &str) -> PathBuf {
    let file = root.join(relative);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "<Project />").unwrap();
    file
}

fn modified_at(file: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)))
        .unwrap();
}

fn service(runner: &Arc<Runner>) -> Arc<ModSources> {
    Arc::new(ModSources::new(runner.clone()))
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(5), "condition did not become true");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn discovery_uses_evaluated_targets_limits_depth_and_ignores_outputs_and_tools() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "loom/src/Loom.csproj");
    project(temp.path(), "tithe/src/Tithe.csproj");
    project(temp.path(), "tools/Tool.csproj");
    project(temp.path(), "a/b/c/DepthThree.csproj");
    project(temp.path(), "a/b/c/d/TooDeep.csproj");
    project(temp.path(), "loom/bin/Copied.csproj");
    project(temp.path(), "loom/OBJ/Copied.csproj");
    project(temp.path(), ".git/Hidden.csproj");
    let runner = Arc::new(Runner::default());
    let sources = service(&runner);
    let cancel = Cancellation::default();
    let found = sources.find(Some(temp.path()), None, &cancel).unwrap();
    assert_eq!(found.len(), 4);
    assert_eq!(runner.started.load(Ordering::Relaxed), 4);
    let snapshot = sources.read(Some(temp.path()), None, &["tItHe.DLL".into()], &cancel).unwrap();
    assert!(!snapshot.builds["tithe.dll"].is_built());
    assert_eq!(
        snapshot.not_installed.iter().map(|project| project.name.as_str()).collect::<Vec<_>>(),
        ["DepthThree", "Loom"]
    );
    assert_eq!(runner.started.load(Ordering::Relaxed), 4, "unchanged projects use cached evaluations");
    assert!(sources.find(None, None, &cancel).unwrap().is_empty());
    assert!(sources.find(Some(&temp.path().join("missing")), None, &cancel).unwrap().is_empty());
    sources.close();
}

#[test]
fn evaluation_identifies_melon_packages_but_never_guesses_relative_output_paths() {
    let temp = tempfile::tempdir().unwrap();
    let file = project(temp.path(), "Loom.csproj");
    let mut answer = json!({
        "Properties": { "TargetPath": target(&file), "OutputType": "Library", "AssemblyName": "CustomName", "Version": " 0.7.0 " },
        "Items": { "PackageReference": [{ "Identity": "LavaGang.MelonLoader.net6" }] },
    });
    let (found, _) = evaluated(&file, &answer.to_string()).unwrap();
    let found = found.unwrap();
    assert!(found.is_mod);
    assert_eq!(found.name, "CustomName");
    assert_eq!(found.version.as_deref(), Some("0.7.0"));
    answer["Properties"]["TargetPath"] = "bin/Loom.dll".into();
    assert!(evaluated(&file, &answer.to_string()).unwrap().0.is_none());
    assert!(evaluated(&file, "Welcome to .NET").is_none());
}

#[test]
fn failed_evaluations_are_cached_but_adding_old_dated_package_props_invalidates_them() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "loom/src/Loom.csproj");
    let runner = Arc::new(Runner::default());
    *runner.failure.lock().unwrap() = Some(ProcessError::Failed("bad project".into()));
    let sources = service(&runner);
    let cancel = Cancellation::default();
    assert!(sources.find(Some(temp.path()), None, &cancel).unwrap().is_empty());
    assert!(sources.find(Some(temp.path()), None, &cancel).unwrap().is_empty());
    assert_eq!(runner.started.load(Ordering::Relaxed), 1);
    let packages = project(temp.path(), "Directory.Packages.props");
    modified_at(&packages, 1);
    *runner.failure.lock().unwrap() = None;
    assert_eq!(sources.find(Some(temp.path()), None, &cancel).unwrap().len(), 1);
    assert_eq!(runner.started.load(Ordering::Relaxed), 2);
    assert!(!sources.sdk_missing());
    sources.close();
}

#[test]
fn imported_files_build_props_and_global_json_invalidate_evaluations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("mods");
    let file = project(&root, "loom/src/Loom.csproj");
    let imported = project(temp.path(), "sdk/custom.targets");
    modified_at(&imported, 1);
    let runner = Arc::new(Runner::default());
    runner.imports.lock().unwrap().push(imported.clone());
    let sources = service(&runner);
    let cancel = Cancellation::default();
    sources.find(Some(&root), None, &cancel).unwrap();
    modified_at(&imported, 2);
    sources.find(Some(&root), None, &cancel).unwrap();
    project(&root, "Directory.Build.props");
    sources.find(Some(&root), None, &cancel).unwrap();
    project(temp.path(), "global.json");
    sources.find(Some(&root), None, &cancel).unwrap();
    modified_at(&file, 3);
    sources.find(Some(&root), None, &cancel).unwrap();
    assert_eq!(runner.started.load(Ordering::Relaxed), 5);
    sources.close();
}

#[test]
fn malformed_json_and_missing_sdk_are_retried_without_a_file_change() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "Loom.csproj");
    let runner = Arc::new(Runner::default());
    *runner.answer.lock().unwrap() = Some("Welcome to .NET".into());
    let sources = service(&runner);
    let cancel = Cancellation::default();
    assert!(sources.find(Some(temp.path()), None, &cancel).unwrap().is_empty());
    *runner.answer.lock().unwrap() = None;
    *runner.failure.lock().unwrap() = Some(ProcessError::Start("dotnet missing".into()));
    assert!(sources.find(Some(temp.path()), None, &cancel).unwrap().is_empty());
    assert!(sources.sdk_missing());
    *runner.failure.lock().unwrap() = None;
    assert_eq!(sources.find(Some(temp.path()), None, &cancel).unwrap().len(), 1);
    assert!(!sources.sdk_missing());
    assert_eq!(runner.started.load(Ordering::Relaxed), 3);
    sources.close();
}

#[test]
fn a_new_sdk_choice_is_used_immediately_and_re_evaluates_cached_projects() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "Loom.csproj");
    let runner = Arc::new(Runner::default());
    let sources = service(&runner);
    let cancel = Cancellation::default();
    let first = temp.path().join("sdk-one");
    let second = temp.path().join("sdk-two");
    sources.find(Some(temp.path()), Some(&first), &cancel).unwrap();
    sources.find(Some(temp.path()), Some(&second), &cancel).unwrap();
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].executable, first.join("dotnet.exe"));
    assert_eq!(calls[1].executable, second.join("dotnet.exe"));
    sources.close();
}

#[test]
fn overlapping_looks_share_evaluation_and_one_callers_cancellation_does_not_kill_the_other() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "Loom.csproj");
    let runner = Arc::new(Runner::default());
    runner.held.store(true, Ordering::Release);
    let sources = service(&runner);
    let first_cancel = Cancellation::default();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| sources.find(Some(temp.path()), None, &first_cancel));
        wait_until(|| runner.started.load(Ordering::Acquire) == 1);
        let second = scope.spawn(|| sources.find(Some(temp.path()), None, &Cancellation::default()));
        first_cancel.cancel();
        assert_eq!(first.join().unwrap(), Err(SourceError::Cancelled));
        runner.held.store(false, Ordering::Release);
        assert_eq!(second.join().unwrap().unwrap().len(), 1);
    });
    assert_eq!(runner.started.load(Ordering::Relaxed), 1);
    sources.close();
}

#[test]
fn parallel_evaluations_are_bounded_and_shutdown_cancels_and_drains_them() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..12 {
        project(temp.path(), &format!("Mod{index}.csproj"));
    }
    let runner = Arc::new(Runner::default());
    runner.held.store(true, Ordering::Release);
    let sources = service(&runner);
    std::thread::scope(|scope| {
        let result = scope.spawn(|| sources.find(Some(temp.path()), None, &Cancellation::default()));
        wait_until(|| runner.active.load(Ordering::Acquire) == sources.evaluations);
        sources.close();
        assert_eq!(result.join().unwrap(), Err(SourceError::Cancelled));
    });
    assert_eq!(runner.active.load(Ordering::Acquire), 0);
    assert_eq!(runner.maximum.load(Ordering::Relaxed), sources.evaluations);
    assert!(runner.started.load(Ordering::Relaxed) <= 4);
    assert_eq!(sources.find(Some(temp.path()), None, &Cancellation::default()), Err(SourceError::Cancelled));
}

#[test]
fn latest_output_wins_for_a_duplicate_dll_identity() {
    let temp = tempfile::tempdir().unwrap();
    let first = project(temp.path(), "first/Loom.csproj");
    let second = project(temp.path(), "second/Loom.csproj");
    for (project, time) in [(&first, 1), (&second, 2)] {
        fs::create_dir_all(target(project).parent().unwrap()).unwrap();
        fs::write(target(project), "not an assembly").unwrap();
        modified_at(&target(project), time);
    }
    let runner = Arc::new(Runner::default());
    let sources = service(&runner);
    let snapshot = sources.read(Some(temp.path()), None, &[], &Cancellation::default()).unwrap();
    assert_eq!(snapshot.not_installed.len(), 1);
    assert_eq!(snapshot.not_installed[0].project_file, second);
    sources.close();
}

#[test]
fn built_output_is_hashed_and_source_changes_exclude_generated_output() {
    let temp = tempfile::tempdir().unwrap();
    let file = project(temp.path(), "loom/src/Loom.csproj");
    let source = project(temp.path(), "loom/src/Entry.cs");
    let props = project(temp.path(), "loom/Directory.Build.props");
    for file in [&file, &source, &props] {
        modified_at(file, 1);
    }
    let runner = Arc::new(Runner::default());
    let sources = service(&runner);
    let project = sources.find(Some(temp.path()), None, &Cancellation::default()).unwrap().remove(0);
    assert!(!sources.built(&project, Some(temp.path())).unwrap().is_built());
    fs::create_dir_all(project.target_path.parent().unwrap()).unwrap();
    fs::write(&project.target_path, "not an assembly").unwrap();
    modified_at(&project.target_path, 2);
    project_file_with_time(temp.path(), "loom/src/obj/generated.cs", 100);
    project_file_with_time(temp.path(), "loom/src/.git/index", 100);
    let built = sources.built(&project, Some(temp.path())).unwrap();
    assert_eq!(built.hash, Some(eg_core::hashes::of_bytes(b"not an assembly")));
    assert!(!built.changed_since_build);
    modified_at(&source, 3);
    assert!(sources.built(&project, Some(temp.path())).unwrap().changed_since_build);
    modified_at(&source, 1);
    modified_at(&props, 3);
    assert!(sources.built(&project, Some(temp.path())).unwrap().changed_since_build);
    sources.close();
}

fn project_file_with_time(root: &Path, relative: &str, time: u64) {
    modified_at(&project(root, relative), time);
}

#[test]
fn build_uses_release_selected_sdk_and_loader_root_and_never_deploys() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "loom/src/Loom.csproj");
    let runner = Arc::new(Runner::default());
    let sources = service(&runner);
    let cancel = Cancellation::default();
    let project = sources.find(Some(temp.path()), None, &cancel).unwrap().remove(0);
    let loader = temp.path().join("loader stage");
    let sdk = temp.path().join("selected sdk");
    assert_eq!(sources.build(&project, &loader, Some(&sdk), &cancel).unwrap(), project.target_path);
    let calls = runner.calls.lock().unwrap();
    let build = calls.last().unwrap();
    assert_eq!(build.executable, sdk.join("dotnet.exe"));
    assert_eq!(build.directory, project.project_file.parent().unwrap());
    assert!(build.arguments.contains(&"-p:DeployToGame=false".into()));
    assert!(build.arguments.contains(&"--disable-build-servers".into()));
    assert!(build.arguments.contains(&"Release".into()));
    assert!(build.arguments.contains(&format!("-p:GamePath={}", loader.display()).into()));
    assert!(!loader.exists(), "build service never creates or places game files");
    sources.close();
}

#[test]
fn build_reports_distinct_compiler_errors_and_missing_output() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "Loom.csproj");
    let runner = Arc::new(Runner::default());
    let sources = service(&runner);
    let cancel = Cancellation::default();
    let project = sources.find(Some(temp.path()), None, &cancel).unwrap().remove(0);
    *runner.failure.lock().unwrap() = Some(ProcessError::Failed(format!(
        "{}: error CS1002: ; expected [{}]\n{}: error CS1002: ; expected [{}]\nBuild FAILED.",
        temp.path().join("Entry.cs(12,5)").display(),
        project.project_file.display(),
        temp.path().join("Entry.cs(12,5)").display(),
        project.project_file.display()
    )));
    let error = sources.build(&project, temp.path(), None, &cancel).unwrap_err();
    assert_eq!(error.to_string(), "Loom.csproj did not build: Entry.cs(12,5): error CS1002: ; expected");
    *runner.failure.lock().unwrap() = None;
    runner.omit_output.store(true, Ordering::Relaxed);
    assert!(sources.build(&project, temp.path(), None, &cancel).unwrap_err().to_string().contains("built, but"));
    cancel.cancel();
    assert_eq!(sources.build(&project, temp.path(), None, &cancel), Err(SourceError::Cancelled));
    sources.close();
}

#[test]
fn updates_compare_project_versions_and_installability_compares_hashes() {
    let project = ModProject {
        project_file: PathBuf::from("mods/loom/src/Loom.csproj"),
        target_path: PathBuf::from("mods/loom/src/bin/Loom.dll"),
        name: "Loom".into(),
        version: Some("1.2.0".into()),
        is_mod: true,
    };
    assert!(project.update_available(Some("1.1.0")));
    assert!(!project.update_available(None));
    assert!(!project.update_available(Some("1.2.0")));
    assert!(project.source_folder(Path::new("mods")).starts_with("mods"));
    let build = ModBuild {
        project,
        version: Some("99.0.0".into()),
        hash: Some("ABC123".into()),
        changed_since_build: false,
    };
    assert!(build.is_installed("abc123"));
    assert!(!build.can_install("other", Some("1.1.0")), "Update takes priority over installing an older output");
    assert!(build.can_install("other", Some("1.2.0")));
    assert!(!build.can_install("abc123", Some("1.2.0")));
}

#[test]
fn compiler_errors_are_limited() {
    let output = (0..9).map(|index| format!("file.cs: error E{index}: broken\n")).collect::<String>();
    assert_eq!(compiler_errors(&output, Path::new("unused")).lines().count(), 5);
    assert_eq!(bounded_text("abé", 3), "ab");
}

#[test]
fn process_runner_distinguishes_pre_cancelled_work_from_a_missing_executable() {
    let temp = tempfile::tempdir().unwrap();
    let request = ProcessRequest {
        executable: temp.path().join("does-not-exist.exe"),
        arguments: Vec::new(),
        directory: temp.path().to_path_buf(),
    };
    let runtime = eg_runtime::Runtime::start().unwrap();
    let runner = SystemProcessRunner::new(runtime.tasks());
    let cancel = Cancellation::default();
    assert!(matches!(runner.run(&request, &cancel), Err(ProcessError::Start(_))));
    cancel.cancel();
    assert_eq!(runner.run(&request, &cancel), Err(ProcessError::Cancelled));
    assert!(!runtime.shutdown());
}

#[test]
fn process_runner_says_cancelled_once_the_runtime_has_closed() {
    let temp = tempfile::tempdir().unwrap();
    let request = ProcessRequest {
        executable: temp.path().join("does-not-exist.exe"),
        arguments: Vec::new(),
        directory: temp.path().to_path_buf(),
    };
    let runtime = eg_runtime::Runtime::start().unwrap();
    let runner = SystemProcessRunner::new(runtime.tasks());
    assert!(!runtime.shutdown());
    assert_eq!(runner.run(&request, &Cancellation::default()), Err(ProcessError::Cancelled));
}

#[test]
fn an_evaluator_panic_is_reported_and_releases_all_waiters() {
    struct Panicking;
    impl ProcessRunner for Panicking {
        fn run(&self, _: &ProcessRequest, _: &Cancellation) -> Result<String, ProcessError> {
            panic!("fixture panic")
        }
    }
    let temp = tempfile::tempdir().unwrap();
    project(temp.path(), "Loom.csproj");
    let sources = Arc::new(ModSources::new(Arc::new(Panicking)));
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| tx.send(sources.find(Some(temp.path()), None, &Cancellation::default())).unwrap());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err().to_string().contains("panicked"));
    });
    sources.close();
}
