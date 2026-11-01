//! Local mod projects, evaluated by MSBuild rather than guessed from XML. Builds never deploy to the game:
//! their output is offered to the library, which places it on the next modded start.

use melon_loader::assembly::{LoaderVersion, MelonAssembly};
use melon_loader::project::{Evaluation, Stamp, changed_since, compiler_errors, dotnet, evaluation_arguments, path_key, project_files, read_evaluation};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};
use task_runtime::CancellationToken;
use task_runtime::{Closed, Tasks, process};

use spiderland_phrases::phrases;

phrases! {
    Cancelled: "The operation was cancelled."
    CannotStart: "Cannot start {program}: {error}"
    OutputFailed: "Reading build output failed."
    Exited: "{program} exited with {code}.\n{output}"
    ScanPanicked: "Reading mod projects panicked."
    ScanNotStarted: "Reading mod projects could not start: {error}"
    NotBuilt: "{project} did not build. {message}"
    NotBuiltErrors: "{project} did not build: {errors}"
    NoOutput: "{project} built, but {target} is not there."
}

const PROJECT_DEPTH: usize = 3;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const POLL: Duration = Duration::from_millis(20);

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
    /// Case-insensitive DLL identities are stored in lower case.
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

#[derive(Default)]
struct Flight {
    result: Mutex<Option<Result<Vec<ModProject>, SourceError>>>,
    ready: Condvar,
}

pub struct ModSources {
    runner: Arc<dyn ProcessRunner>,
    projects: Mutex<BTreeMap<String, CachedProject>>,
    finding: Mutex<BTreeMap<String, Arc<Flight>>>,
    lifetime: CancellationToken,
    sdk_missing: AtomicBool,
    evaluations: usize,
}

impl ModSources {
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            projects: Mutex::default(),
            finding: Mutex::default(),
            lifetime: CancellationToken::default(),
            sdk_missing: AtomicBool::new(false),
            evaluations: (std::thread::available_parallelism().map_or(1, usize::from) / 2).clamp(1, 4),
        }
    }

    pub fn sdk_missing(&self) -> bool {
        self.sdk_missing.load(Ordering::Acquire)
    }

    /// Call during application shutdown: ordinary callers may stop waiting without cancelling a shared evaluation.
    pub fn close(&self) {
        let flights: Vec<_> = {
            let finding = self.finding.lock().unwrap();
            self.lifetime.cancel();
            finding.values().cloned().collect()
        };
        for flight in flights {
            let mut result = flight.result.lock().unwrap();
            while result.is_none() {
                result = flight.ready.wait(result).unwrap();
            }
        }
    }

    /// Folder and SDK are the current settings, not the settings from when this service was constructed.
    pub fn find(self: &Arc<Self>, folder: Option<&Path>, sdk: Option<&Path>, cancel: &CancellationToken) -> Result<Vec<ModProject>, SourceError> {
        cancelled(cancel)?;
        let Some(folder) = folder.filter(|folder| folder.is_dir()) else {
            self.sdk_missing.store(false, Ordering::Release);
            return Ok(Vec::new());
        };
        let folder = file_ops::paths::normalized(folder);
        let executable = dotnet::executable(sdk);
        let key = format!("{}|{}", path_key(&folder), path_key(&executable));
        let flight = {
            let mut finding = self.finding.lock().unwrap();
            cancelled(&self.lifetime)?;
            // Completed looks are replaced, but overlapping callers share even uncached project evaluations.
            finding.retain(|_, flight| flight.result.lock().unwrap().is_none());
            if let Some(flight) = finding.get(&key) {
                flight.clone()
            } else {
                let flight = Arc::new(Flight::default());
                finding.insert(key, flight.clone());
                let owner = self.clone();
                let answer = flight.clone();
                if let Err(error) = std::thread::Builder::new().name("mod-source-scan".into()).spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.find_in(&folder, &executable, &owner.lifetime)))
                        .unwrap_or_else(|_| Err(SourceError::Failed(ScanPanicked.into())));
                    *answer.result.lock().unwrap() = Some(result);
                    answer.ready.notify_all();
                }) {
                    *flight.result.lock().unwrap() = Some(Err(SourceError::Failed(ScanNotStarted.with(&[("error", &error)]))));
                }
                flight
            }
        };
        let mut result = flight.result.lock().unwrap();
        loop {
            cancelled(cancel)?;
            if let Some(result) = result.as_ref() {
                return result.clone();
            }
            result = flight.ready.wait_timeout(result, POLL).unwrap().0;
        }
    }

    fn find_in(&self, folder: &Path, executable: &Path, cancel: &CancellationToken) -> Result<Vec<ModProject>, SourceError> {
        let files = project_files(folder, PROJECT_DEPTH)?;
        let next = AtomicUsize::new(0);
        let missing = AtomicBool::new(false);
        let found = Mutex::new(Vec::new());
        let failure = Mutex::new(None);
        std::thread::scope(|scope| {
            for _ in 0..self.evaluations.min(files.len()) {
                scope.spawn(|| {
                    loop {
                        if cancel.is_cancelled() {
                            *failure.lock().unwrap() = Some(SourceError::Cancelled);
                            break;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = files.get(index) else { break };
                        match self.evaluate(file, folder, executable, cancel) {
                            Ok((project, started)) => {
                                if !started {
                                    missing.store(true, Ordering::Relaxed);
                                }
                                if let Some(project) = project {
                                    found.lock().unwrap().push((index, project));
                                }
                            }
                            Err(error) => *failure.lock().unwrap() = Some(error),
                        }
                    }
                });
            }
        });
        self.sdk_missing.store(missing.load(Ordering::Relaxed), Ordering::Release);
        if let Some(error) = failure.into_inner().unwrap() {
            return Err(error);
        }
        let mut found = found.into_inner().unwrap();
        found.sort_by_key(|(index, _)| *index);
        Ok(found.into_iter().map(|(_, project)| project).collect())
    }

    fn evaluate(&self, file: &Path, folder: &Path, executable: &Path, cancel: &CancellationToken) -> Result<(Option<ModProject>, bool), SourceError> {
        let key = format!("{}|{}|{}", path_key(file), path_key(folder), path_key(executable));
        let known = self.projects.lock().unwrap().get(&key).cloned();
        let imports = known.as_ref().map_or(&[][..], |known| known.imports.as_slice());
        if let Some(known) = &known
            && known.stamp == Stamp::of(file, folder, imports)?
        {
            return Ok((known.project.clone(), true));
        }
        let request = ProcessRequest {
            executable: executable.to_path_buf(),
            arguments: evaluation_arguments(file),
            directory: file.parent().unwrap_or(folder).to_path_buf(),
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
        let have: BTreeSet<_> = installed.iter().map(|file| file.to_lowercase()).collect();
        let projects = self.find(folder, sdk, cancel)?;
        let mut snapshot = SourceSnapshot::default();
        for project in latest(projects.iter().filter(|project| have.contains(&project.file().to_lowercase()))) {
            cancelled(cancel)?;
            snapshot.builds.insert(project.file().to_lowercase(), self.built(project, folder)?);
        }
        snapshot.not_installed = latest(projects.iter().filter(|project| project.is_mod && !have.contains(&project.file().to_lowercase())))
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
        let key = project.file().to_lowercase();
        if by_file.get(&key).is_none_or(|previous| modified(&project.target_path) > modified(&previous.target_path)) {
            by_file.insert(key, project);
        }
    }
    by_file.into_values().collect()
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
