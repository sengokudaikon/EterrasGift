//! Local mod projects, evaluated by MSBuild rather than guessed from XML. Builds never deploy to the game:
//! their output is offered to the library, which places it on the next modded start.

use file_ops::case;
use melon_loader::assembly::{LoaderVersion, MelonAssembly};
use melon_loader::project::{Evaluation, Stamp, changed_since, compiler_errors, dotnet, evaluation_arguments, path_key, project_files, read_evaluation};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use task_runtime::CancellationToken;
use task_runtime::{Closed, Tasks, process};
use tokio::sync::{Semaphore, watch};

use phrases::phrases;

phrases! {
    Cancelled: "The operation was cancelled."
    CannotStart: "Cannot start {program}: {error}"
    OutputFailed: "Reading build output failed."
    Exited: "{program} exited with {code}.\n{output}"
    ScanPanicked: "Reading mod projects panicked."
    NotBuilt: "{project} did not build. {message}"
    NotBuiltErrors: "{project} did not build: {errors}"
    NoOutput: "{project} built, but {target} is not there."
}

const PROJECT_DEPTH: usize = 3;
const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModProject {
    pub project_file: PathBuf,
    pub target_path: PathBuf,
    pub name: String,
    pub version: Option<String>,
    pub is_mod: bool,
}

impl ModProject {
    pub fn file(&self) -> String {
        self.target_path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }

    /// A newer project version is compared with the installed assembly's informational version, never MelonInfo.
    pub fn update_available(&self, installed_project_version: Option<&str>) -> bool {
        self.version
            .as_deref()
            .and_then(LoaderVersion::parse)
            .zip(installed_project_version.and_then(LoaderVersion::parse))
            .is_some_and(|(source, installed)| source > installed)
    }

    pub fn source_folder(&self, sources: &Path) -> String {
        let folder = self.project_file.parent().unwrap_or(&self.project_file);
        // Paths come from the same directory walk, so their spelling agrees; falling back to the full path is safe.
        folder
            .strip_prefix(sources)
            .map(|under| Path::new(sources.file_name().unwrap_or_default()).join(under))
            .unwrap_or_else(|_| folder.to_path_buf())
            .display()
            .to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModBuild {
    pub project: ModProject,
    pub version: Option<String>,
    pub hash: Option<String>,
    pub changed_since_build: bool,
}

impl ModBuild {
    pub fn is_built(&self) -> bool {
        self.hash.is_some()
    }

    pub fn is_installed(&self, installed_hash: &str) -> bool {
        self.hash.as_deref().is_some_and(|hash| hash.eq_ignore_ascii_case(installed_hash))
    }

    pub fn can_install(&self, installed_hash: &str, installed_project_version: Option<&str>) -> bool {
        self.is_built() && !self.is_installed(installed_hash) && !self.project.update_available(installed_project_version)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceSnapshot {
    /// By the DLL's name, as a [`file_ops::case::key`].
    pub builds: BTreeMap<String, ModBuild>,
    pub not_installed: Vec<ModProject>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    Cancelled,
    SdkMissing(String),
    Failed(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str(Cancelled.get()),
            Self::SdkMissing(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<io::Error> for SourceError {
    fn from(error: io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct ProcessRequest {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessError {
    /// The executable could not be started, distinct from a project that fails to evaluate or build.
    Start(String),
    Failed(String),
    Cancelled,
}

pub trait ProcessRunner: Send + Sync {
    fn run(&self, request: &ProcessRequest, cancel: &CancellationToken) -> Result<String, ProcessError>;
}

/// Runs real programs on the runtime. It waits on the calling thread, so call it off the UI thread.
pub struct SystemProcessRunner {
    tasks: Tasks,
}

impl SystemProcessRunner {
    pub fn new(tasks: Tasks) -> Self {
        Self { tasks }
    }
}

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, request: &ProcessRequest, cancel: &CancellationToken) -> Result<String, ProcessError> {
        let mut command = process::Command::new(&request.executable);
        command
            .args(&request.arguments)
            .current_dir(&request.directory)
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1")
            .env("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1");
        let cancel = cancel.clone();
        // Refused only once the runtime has closed: the app is on its way out.
        let task = self
            .tasks
            .spawn(async move { process::run(command, OUTPUT_LIMIT, &cancel).await })
            .map_err(|Closed| ProcessError::Cancelled)?;
        let output = self
            .tasks
            .block_on(task)
            .map_err(|_| ProcessError::Failed(OutputFailed.into()))?
            .map_err(|error| match error {
                process::Error::Start(error) => ProcessError::Start(CannotStart.with(&[("program", &request.executable.display()), ("error", &error)])),
                process::Error::Failed(error) => ProcessError::Failed(error.to_string()),
                process::Error::Cancelled => ProcessError::Cancelled,
            })?;
        if !output.status.success() {
            let name = request.executable.file_name().unwrap_or_default().to_string_lossy();
            let captured = format!(
                "{}
{}",
                output.stdout, output.stderr
            );
            return Err(ProcessError::Failed(Exited.with(&[
                ("program", &name),
                ("code", &output.status.code().unwrap_or(-1)),
                ("output", &bounded_text(&captured, OUTPUT_LIMIT)),
            ])));
        }
        Ok(output.stdout.trim().to_owned())
    }
}

fn bounded_text(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Clone)]
struct CachedProject {
    stamp: Stamp,
    imports: Vec<PathBuf>,
    project: Option<ModProject>,
}

/// One look at a folder, shared by everyone who asks for it while it runs: None until it has found the projects. A sender
/// gone without a value is a look that panicked.
type Flight = watch::Receiver<Option<Result<Vec<ModProject>, SourceError>>>;

/// Local mod projects, found by evaluating each with the .NET SDK. Its work is a scope of its own on the runtime:
/// [`ModSources::close`] cancels it and waits for it, and a panic in it counts as the runtime's.
pub struct ModSources {
    runner: Arc<dyn ProcessRunner>,
    projects: Mutex<BTreeMap<String, CachedProject>>,
    finding: Mutex<BTreeMap<String, Flight>>,
    tasks: Tasks,
    /// Cancelled by the close: every look and evaluation stops.
    lifetime: CancellationToken,
    sdk_missing: AtomicBool,
    /// Held by each evaluation: at most [`evaluations`] run at once, across every look.
    running: Arc<Semaphore>,
}

/// How many projects are evaluated at once: half the processors, at most four.
fn evaluations() -> usize {
    (std::thread::available_parallelism().map_or(1, usize::from) / 2).clamp(1, 4)
}

impl ModSources {
    pub fn new(runner: Arc<dyn ProcessRunner>, tasks: &Tasks) -> Self {
        let tasks = tasks.scope();
        Self {
            runner,
            projects: Mutex::default(),
            finding: Mutex::default(),
            lifetime: tasks.token(),
            tasks,
            sdk_missing: AtomicBool::new(false),
            running: Arc::new(Semaphore::new(evaluations())),
        }
    }

    pub fn sdk_missing(&self) -> bool {
        self.sdk_missing.load(Ordering::Acquire)
    }

    /// Call during application shutdown, off the reactor: ordinary callers may stop waiting without cancelling a shared
    /// evaluation. It cancels every look and waits for them.
    pub fn close(&self) {
        self.tasks.block_on(self.tasks.close());
    }

    /// Folder and SDK are the current settings, not the settings from when this service was constructed. It waits on this
    /// thread, so call it off the UI thread and the reactor.
    pub fn find(self: &Arc<Self>, folder: Option<&Path>, sdk: Option<&Path>, cancel: &CancellationToken) -> Result<Vec<ModProject>, SourceError> {
        cancelled(cancel)?;
        let Some(folder) = folder.filter(|folder| folder.is_dir()) else {
            self.sdk_missing.store(false, Ordering::Release);
            return Ok(Vec::new());
        };
        let folder = file_ops::paths::normalized(folder);
        let executable = dotnet::executable(sdk);
        let key = format!("{}|{}", path_key(&folder), path_key(&executable));
        let mut flight = {
            let mut finding = self.finding.lock().unwrap();
            cancelled(&self.lifetime)?;
            // Completed looks are replaced, but overlapping callers share even uncached project evaluations.
            finding.retain(|_, flight| flight.borrow().is_none());
            if let Some(flight) = finding.get(&key) {
                flight.clone()
            } else {
                let (found, flight) = watch::channel(None);
                let owner = self.clone();
                // Refused only once the close has begun.
                self.tasks
                    .spawn(async move {
                        let result = owner.find_in(folder, executable).await;
                        found.send_replace(Some(result));
                    })
                    .map_err(|Closed| SourceError::Cancelled)?;
                finding.insert(key, flight.clone());
                flight
            }
        };
        self.tasks.block_on(async {
            tokio::select! {
                biased;
                () = cancel.cancelled() => Err(SourceError::Cancelled),
                found = flight.wait_for(Option::is_some) => match found {
                    Ok(found) => found.clone().expect("waited for a value"),
                    Err(_) => Err(SourceError::Failed(ScanPanicked.into())),
                },
            }
        })
    }

    /// Evaluates every project under `folder`, at most [`evaluations`] at once, each on the blocking pool.
    async fn find_in(self: Arc<Self>, folder: PathBuf, executable: PathBuf) -> Result<Vec<ModProject>, SourceError> {
        let files = {
            let folder = folder.clone();
            match self.tasks.spawn_blocking(move || project_files(&folder, PROJECT_DEPTH)) {
                Ok(files) => files.await.map_err(|_| SourceError::Failed(ScanPanicked.into()))??,
                Err(Closed) => return Err(SourceError::Cancelled),
            }
        };
        let (folder, executable) = (Arc::new(folder), Arc::new(executable));
        let mut evaluations = Vec::with_capacity(files.len());
        let mut failure = None;
        for file in files {
            let permit = tokio::select! {
                biased;
                () = self.lifetime.cancelled() => None,
                permit = self.running.clone().acquire_owned() => permit.ok(),
            };
            let Some(permit) = permit else {
                failure = Some(SourceError::Cancelled);
                break;
            };
            let (owner, folder, executable) = (self.clone(), folder.clone(), executable.clone());
            match self.tasks.spawn_blocking(move || {
                let _permit = permit;
                owner.evaluate(&file, &folder, &executable, &owner.lifetime)
            }) {
                Ok(evaluation) => evaluations.push(evaluation),
                Err(Closed) => {
                    failure = Some(SourceError::Cancelled);
                    break;
                }
            }
        }
        let mut found = Vec::new();
        let mut missing = false;
        // Every evaluation started is awaited, even after a failure, so none outlives the look.
        for evaluation in evaluations {
            match evaluation.await {
                Ok(Ok((project, started))) => {
                    missing |= !started;
                    found.extend(project);
                }
                Ok(Err(error)) => failure = Some(error),
                Err(_) => failure = Some(SourceError::Failed(ScanPanicked.into())),
            }
        }
        self.sdk_missing.store(missing, Ordering::Release);
        failure.map_or(Ok(found), Err)
    }

    fn evaluate(&self, file: &Path, folder: &Path, executable: &Path, cancel: &CancellationToken) -> Result<(Option<ModProject>, bool), SourceError> {
        cancelled(cancel)?;
        let key = format!("{}|{}|{}", path_key(file), path_key(folder), path_key(executable));
        let known = self.projects.lock().unwrap().get(&key).cloned();
        let imports = known.as_ref().map_or(&[][..], |known| known.imports.as_slice());
        if let Some(known) = &known
            && known.stamp == Stamp::of(file, folder, imports)?
        {
            return Ok((known.project.clone(), true));
        }
        // From the SDK's own folder rather than the project's: `dotnet` takes the `global.json` it finds from where it
        // runs, and one beside a downloaded project could choose which SDK, and so which code, it loads.
        let request = ProcessRequest {
            executable: executable.to_path_buf(),
            arguments: evaluation_arguments(&std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf())),
            directory: executable.parent().filter(|parent| parent.is_absolute()).unwrap_or(folder).to_path_buf(),
        };
        let output = match self.runner.run(&request, cancel) {
            Ok(output) => output,
            Err(ProcessError::Start(_)) => return Ok((None, false)),
            Err(ProcessError::Cancelled) => return Err(SourceError::Cancelled),
            Err(ProcessError::Failed(_)) => {
                let cached = CachedProject {
                    stamp: Stamp::of(file, folder, imports)?,
                    imports: imports.to_vec(),
                    project: None,
                };
                self.projects.lock().unwrap().insert(key, cached);
                return Ok((None, true));
            }
        };
        // Non-JSON first-run banners aren't cached, so the next look retries without a file change.
        let Some(Evaluation { project, imports }) = read_evaluation(file, &output) else {
            return Ok((None, true));
        };
        let project = project.map(|project| ModProject {
            project_file: file.to_path_buf(),
            target_path: project.target_path,
            name: project.name,
            version: project.version,
            is_mod: project.is_mod,
        });
        let cached = CachedProject {
            stamp: Stamp::of(file, folder, &imports)?,
            imports,
            project: project.clone(),
        };
        self.projects.lock().unwrap().insert(key, cached);
        Ok((project, true))
    }

    pub fn read(self: &Arc<Self>, folder: Option<&Path>, sdk: Option<&Path>, installed: &[String], cancel: &CancellationToken) -> Result<SourceSnapshot, SourceError> {
        let have: BTreeSet<_> = installed.iter().map(|file| case::key(file)).collect();
        let projects = self.find(folder, sdk, cancel)?;
        let mut snapshot = SourceSnapshot::default();
        for project in latest(projects.iter().filter(|project| have.contains(&case::key(&project.file())))) {
            cancelled(cancel)?;
            snapshot.builds.insert(case::key(&project.file()), self.built(project, folder)?);
        }
        snapshot.not_installed = latest(projects.iter().filter(|project| project.is_mod && !have.contains(&case::key(&project.file()))))
            .into_iter()
            .cloned()
            .collect();
        snapshot.not_installed.sort_by_key(|project| project.name.to_lowercase());
        Ok(snapshot)
    }

    pub fn built(&self, project: &ModProject, sources: Option<&Path>) -> Result<ModBuild, SourceError> {
        if !project.target_path.is_file() {
            return Ok(ModBuild {
                project: project.clone(),
                version: None,
                hash: None,
                changed_since_build: false,
            });
        }
        let version = match MelonAssembly::read(&project.target_path) {
            Ok(assembly) => assembly.map(|assembly| assembly.version),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => None,
            Err(error) => return Err(error.into()),
        };
        let hash = file_ops::hashes::of_file(&project.target_path)?;
        let built = fs::metadata(&project.target_path)?.modified()?;
        let changed_since_build = changed_since(&project.project_file, sources, built)?;
        Ok(ModBuild {
            project: project.clone(),
            version,
            hash: Some(hash),
            changed_since_build,
        })
    }

    pub fn build(&self, project: &ModProject, loader_root: &Path, sdk: Option<&Path>, cancel: &CancellationToken) -> Result<PathBuf, SourceError> {
        cancelled(cancel)?;
        let directory = project.project_file.parent().unwrap_or(&project.project_file);
        let mut arguments = vec![OsString::from("build"), project.project_file.as_os_str().to_owned()];
        arguments.extend(["-c", "Release", "-nologo", "-v", "quiet", "--disable-build-servers", "-p:DeployToGame=false"].map(OsString::from));
        let mut game_path = OsString::from("-p:GamePath=");
        game_path.push(file_ops::paths::normalized(loader_root));
        arguments.push(game_path);
        let request = ProcessRequest {
            executable: dotnet::executable(sdk),
            arguments,
            directory: directory.to_path_buf(),
        };
        match self.runner.run(&request, cancel) {
            Ok(_) => {}
            Err(ProcessError::Cancelled) => return Err(SourceError::Cancelled),
            Err(ProcessError::Start(message)) => return Err(SourceError::SdkMissing(message)),
            Err(ProcessError::Failed(message)) => {
                let name = project.project_file.file_name().unwrap_or_default().to_string_lossy();
                let errors = compiler_errors(&message, directory);
                return Err(SourceError::Failed(if errors.is_empty() {
                    NotBuilt.with(&[("project", &name), ("message", &message)])
                } else {
                    NotBuiltErrors.with(&[("project", &name), ("errors", &errors)])
                }));
            }
        }
        cancelled(cancel)?;
        if !project.target_path.is_file() {
            return Err(SourceError::Failed(NoOutput.with(&[
                ("project", &project.project_file.file_name().unwrap_or_default().to_string_lossy()),
                ("target", &project.target_path.display()),
            ])));
        }
        Ok(project.target_path.clone())
    }

    /// A token for work stopped from elsewhere, such as a [`build`](Self::build) the page lets go of: cancelled as soon as
    /// `stop` completes, or when the sources close. Nothing polls: a task waits on `stop`, and cancelling the token ends it.
    pub fn cancel_on(&self, stop: impl Future<Output = ()> + Send + 'static) -> CancellationToken {
        let cancel = self.lifetime.child_token();
        let watched = cancel.clone();
        let watching = self.tasks.spawn(async move {
            tokio::select! {
                () = stop => watched.cancel(),
                () = watched.cancelled() => {}
            }
        });
        // Refused only once the runtime has closed: the app is on its way out, and nothing more should run.
        if watching.is_err() {
            cancel.cancel();
        }
        cancel
    }
}

fn cancelled(cancel: &CancellationToken) -> Result<(), SourceError> {
    if cancel.is_cancelled() { Err(SourceError::Cancelled) } else { Ok(()) }
}

fn modified(path: &Path) -> Option<SystemTime> {
    file_ops::stamp::written(path).ok()
}

fn latest<'a>(projects: impl Iterator<Item = &'a ModProject>) -> Vec<&'a ModProject> {
    let mut by_file: BTreeMap<String, &'a ModProject> = BTreeMap::new();
    for project in projects {
        let key = case::key(&project.file());
        if by_file.get(&key).is_none_or(|previous| modified(&project.target_path) > modified(&previous.target_path)) {
            by_file.insert(key, project);
        }
    }
    by_file.into_values().collect()
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
