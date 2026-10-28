//! Local mod projects, evaluated by MSBuild rather than guessed from XML. Builds never deploy to the game:
//! their output is offered to the library, which places it on the next modded start.

use super::assembly::{LoaderVersion, MelonAssembly};
use eg_core::operations::Cancellation;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

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

    pub fn caption(&self, sources: &Path) -> String {
        let folder = self.source_folder(sources);
        self.version.as_ref().map_or_else(|| folder.clone(), |version| format!("{version} · {folder}"))
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
            Self::Cancelled => f.write_str("The operation was cancelled."),
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
    fn run(&self, request: &ProcessRequest, cancel: &Cancellation) -> Result<String, ProcessError>;
}

#[derive(Default)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, request: &ProcessRequest, cancel: &Cancellation) -> Result<String, ProcessError> {
        if cancel.is_cancelled() {
            return Err(ProcessError::Cancelled);
        }
        let mut command = Command::new(&request.executable);
        command
            .args(&request.arguments)
            .current_dir(&request.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1")
            .env("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1");
        no_window(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| ProcessError::Start(format!("Cannot start {}: {error}", request.executable.display())))?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        // Drain both pipes concurrently, even after reaching the capture bound, or a noisy compiler can block forever.
        let output = std::thread::spawn(move || drain(stdout));
        let errors = std::thread::spawn(move || drain(stderr));
        let status = loop {
            if cancel.is_cancelled() {
                kill_tree(&mut child);
                let _ = child.wait();
                break Err(ProcessError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(POLL),
                Err(error) => {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    break Err(ProcessError::Failed(error.to_string()));
                }
            }
        };
        let output = output.join().map_err(|_| ProcessError::Failed("Reading build output failed.".into()));
        let errors = errors.join().map_err(|_| ProcessError::Failed("Reading build errors failed.".into()));
        let status = status?;
        let output = output?.map_err(|error| ProcessError::Failed(error.to_string()))?;
        let errors = errors?.map_err(|error| ProcessError::Failed(error.to_string()))?;
        if cancel.is_cancelled() {
            return Err(ProcessError::Cancelled);
        }
        if !status.success() {
            let name = request.executable.file_name().unwrap_or_default().to_string_lossy();
            let captured = format!("{output}\n{errors}");
            return Err(ProcessError::Failed(format!(
                "{name} exited with {}.\n{}",
                status.code().unwrap_or(-1),
                bounded_text(&captured, OUTPUT_LIMIT)
            )));
        }
        Ok(output.trim().to_owned())
    }
}

fn drain(mut reader: impl Read) -> io::Result<String> {
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(String::from_utf8_lossy(&captured).into_owned());
        }
        let keep = count.min(OUTPUT_LIMIT - captured.len());
        captured.extend_from_slice(&buffer[..keep]);
    }
}

fn bounded_text(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        // Only our captured compiler's PID is used; descendants can otherwise keep the output pipes open.
        let mut kill = Command::new("taskkill.exe");
        kill.args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut kill);
        let _ = kill.status();
    }
    let _ = child.kill();
}

type Stamp = Vec<(String, Option<SystemTime>)>;

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
    lifetime: Cancellation,
    sdk_missing: AtomicBool,
    evaluations: usize,
}

impl ModSources {
    pub fn new(runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            projects: Mutex::default(),
            finding: Mutex::default(),
            lifetime: Cancellation::default(),
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
    pub fn find(self: &Arc<Self>, folder: Option<&Path>, sdk: Option<&Path>, cancel: &Cancellation) -> Result<Vec<ModProject>, SourceError> {
        cancelled(cancel)?;
        let Some(folder) = folder.filter(|folder| folder.is_dir()) else {
            self.sdk_missing.store(false, Ordering::Release);
            return Ok(Vec::new());
        };
        let folder = eg_core::paths::normalized(folder);
        let executable = executable(sdk);
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
                        .unwrap_or_else(|_| Err(SourceError::Failed("Reading mod projects panicked.".into())));
                    *answer.result.lock().unwrap() = Some(result);
                    answer.ready.notify_all();
                }) {
                    *flight.result.lock().unwrap() = Some(Err(SourceError::Failed(format!("Reading mod projects could not start: {error}"))));
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

    fn find_in(&self, folder: &Path, executable: &Path, cancel: &Cancellation) -> Result<Vec<ModProject>, SourceError> {
        let files = files(folder, Some(PROJECT_DEPTH))?;
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

    fn evaluate(&self, file: &Path, folder: &Path, executable: &Path, cancel: &Cancellation) -> Result<(Option<ModProject>, bool), SourceError> {
        let key = format!("{}|{}|{}", path_key(file), path_key(folder), path_key(executable));
        let known = self.projects.lock().unwrap().get(&key).cloned();
        let imports = known.as_ref().map_or(&[][..], |known| known.imports.as_slice());
        if let Some(known) = &known
            && known.stamp == stamp(file, folder, imports)?
        {
            return Ok((known.project.clone(), true));
        }
        let mut arguments = vec![OsString::from("msbuild"), file.as_os_str().to_owned()];
        arguments.extend(
            [
                "-nologo",
                "-getProperty:TargetPath",
                "-getProperty:OutputType",
                "-getProperty:AssemblyName",
                "-getProperty:Version",
                "-getProperty:MSBuildAllProjects",
                "-getItem:Reference",
                "-getItem:PackageReference",
                "-p:Configuration=Release",
            ]
            .map(OsString::from),
        );
        let request = ProcessRequest {
            executable: executable.to_path_buf(),
            arguments,
            directory: file.parent().unwrap_or(folder).to_path_buf(),
        };
        let output = match self.runner.run(&request, cancel) {
            Ok(output) => output,
            Err(ProcessError::Start(_)) => return Ok((None, false)),
            Err(ProcessError::Cancelled) => return Err(SourceError::Cancelled),
            Err(ProcessError::Failed(_)) => {
                let cached = CachedProject {
                    stamp: stamp(file, folder, imports)?,
                    imports: imports.to_vec(),
                    project: None,
                };
                self.projects.lock().unwrap().insert(key, cached);
                return Ok((None, true));
            }
        };
        // Non-JSON first-run banners aren't cached, so the next look retries without a file change.
        let Some((project, imports)) = evaluated(file, &output) else { return Ok((None, true)) };
        let cached = CachedProject {
            stamp: stamp(file, folder, &imports)?,
            imports,
            project: project.clone(),
        };
        self.projects.lock().unwrap().insert(key, cached);
        Ok((project, true))
    }

    pub fn read(self: &Arc<Self>, folder: Option<&Path>, sdk: Option<&Path>, installed: &[String], cancel: &Cancellation) -> Result<SourceSnapshot, SourceError> {
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
        let hash = eg_core::hashes::of_file(&project.target_path)?;
        let built = fs::metadata(&project.target_path)?.modified()?;
        let mut source_files = files(project.project_file.parent().unwrap_or(&project.project_file), None)?;
        source_files.extend(build_props(&project.project_file, sources));
        let changed_since_build = source_files.iter().any(|file| modified(file).is_some_and(|time| time > built));
        Ok(ModBuild {
            project: project.clone(),
            version,
            hash: Some(hash),
            changed_since_build,
        })
    }

    pub fn build(&self, project: &ModProject, loader_root: &Path, sdk: Option<&Path>, cancel: &Cancellation) -> Result<PathBuf, SourceError> {
        cancelled(cancel)?;
        let directory = project.project_file.parent().unwrap_or(&project.project_file);
        let mut arguments = vec![OsString::from("build"), project.project_file.as_os_str().to_owned()];
        arguments.extend(["-c", "Release", "-nologo", "-v", "quiet", "--disable-build-servers", "-p:DeployToGame=false"].map(OsString::from));
        let mut game_path = OsString::from("-p:GamePath=");
        game_path.push(eg_core::paths::normalized(loader_root));
        arguments.push(game_path);
        let request = ProcessRequest {
            executable: executable(sdk),
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
                    format!("{name} did not build. {message}")
                } else {
                    format!("{name} did not build: {errors}")
                }));
            }
        }
        cancelled(cancel)?;
        if !project.target_path.is_file() {
            return Err(SourceError::Failed(format!(
                "{} built, but {} is not there.",
                project.project_file.file_name().unwrap_or_default().to_string_lossy(),
                project.target_path.display()
            )));
        }
        Ok(project.target_path.clone())
    }
}

fn executable(sdk: Option<&Path>) -> PathBuf {
    sdk.map(|folder| folder.join("dotnet.exe"))
        .or_else(crate::mods::dotnet::found)
        .unwrap_or_else(|| "dotnet".into())
}

fn cancelled(cancel: &Cancellation) -> Result<(), SourceError> {
    if cancel.is_cancelled() { Err(SourceError::Cancelled) } else { Ok(()) }
}

fn evaluated(file: &Path, output: &str) -> Option<(Option<ModProject>, Vec<PathBuf>)> {
    let root: Value = serde_json::from_str(output).ok()?;
    let Some(properties) = root.get("Properties").filter(|value| value.is_object()) else {
        return Some((None, Vec::new()));
    };
    let imports = text(properties, "MSBuildAllProjects")
        .into_iter()
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect();
    let target = PathBuf::from(text(properties, "TargetPath").unwrap_or_default());
    if !target.is_absolute() || !target.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll")) {
        return Some((None, imports));
    }
    let library = text(properties, "OutputType").is_some_and(|kind| kind.eq_ignore_ascii_case("Library"));
    let items = |kind| root.get("Items").and_then(|items| items.get(kind)).and_then(Value::as_array).into_iter().flatten();
    let melon = items("Reference").any(|item| text(item, "Filename").is_some_and(|name| name.eq_ignore_ascii_case("MelonLoader")))
        || items("PackageReference").any(|item| text(item, "Identity").is_some_and(|name| name.to_ascii_lowercase().starts_with("lavagang.melonloader")));
    Some((
        Some(ModProject {
            project_file: file.to_path_buf(),
            target_path: target,
            name: text(properties, "AssemblyName")
                .map(str::to_owned)
                .unwrap_or_else(|| file.file_stem().unwrap_or_default().to_string_lossy().into_owned()),
            version: text(properties, "Version").map(str::to_owned),
            is_mod: library && melon,
        }),
        imports,
    ))
}

fn text<'a>(owner: &'a Value, name: &str) -> Option<&'a str> {
    owner.get(name)?.as_str().map(str::trim).filter(|text| !text.is_empty())
}

fn path_key(path: &Path) -> String {
    eg_core::paths::normalized(path).to_string_lossy().to_lowercase()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|metadata| metadata.modified()).ok()
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

fn stamp(file: &Path, folder: &Path, imports: &[PathBuf]) -> io::Result<Stamp> {
    let mut paths = vec![file.to_path_buf()];
    paths.extend(build_props(file, Some(folder)));
    paths.extend(above(file, "Directory.Packages.props", Some(folder)));
    paths.extend(above(file, "global.json", None));
    paths.extend_from_slice(imports);
    let mut stamp = BTreeMap::new();
    for path in paths {
        let time = match fs::metadata(&path) {
            Ok(metadata) => Some(metadata.modified()?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        stamp.insert(path_key(&path), time);
    }
    Ok(stamp.into_iter().collect())
}

fn above(project: &Path, name: &str, top: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let top = top.map(path_key);
    for folder in project.parent().into_iter().flat_map(Path::ancestors) {
        let path = folder.join(name);
        if path.is_file() {
            found.push(path);
        }
        if top.as_ref().is_some_and(|top| *top == path_key(folder)) {
            break;
        }
    }
    found
}

fn build_props(project: &Path, sources: Option<&Path>) -> Vec<PathBuf> {
    let mut props = above(project, "Directory.Build.props", sources);
    props.extend(above(project, "Directory.Build.targets", sources));
    props
}

/// With a depth, collect projects; without one, collect all source files. Reparse folders are excluded so a linked
/// output or directory cycle cannot escape the user's sources tree.
fn files(folder: &Path, depth: Option<usize>) -> io::Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(folder)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_ascii_lowercase());
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_file() && (depth.is_none() || path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csproj"))) {
            found.push(path);
        } else if metadata.is_dir() && depth != Some(0) && searched(&entry) {
            found.extend(files(&path, depth.map(|depth| depth - 1))?);
        }
    }
    Ok(found)
}

fn searched(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy().to_lowercase();
    !(name.starts_with('.') || name == "bin" || name == "obj") && entry.metadata().is_ok_and(|metadata| eg_core::paths::is_plain_directory(&metadata))
}

fn compiler_errors(output: &str, folder: &Path) -> String {
    let prefix = format!("{}{}", folder.display(), std::path::MAIN_SEPARATOR);
    let mut errors = Vec::new();
    for line in output.lines().map(str::trim).filter(|line| line.contains(": error ")) {
        let line = if line.ends_with(']') {
            line.rsplit_once(" [").map_or(line, |(error, _)| error)
        } else {
            line
        };
        let line = if line.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(&prefix)) {
            &line[prefix.len()..]
        } else {
            line
        };
        if !errors.contains(&line) {
            errors.push(line);
            if errors.len() == 5 {
                break;
            }
        }
    }
    errors.join("\n")
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
