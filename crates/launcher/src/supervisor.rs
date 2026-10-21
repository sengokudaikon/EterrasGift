//! The launcher's state machine (the C# `PreparationWindow`'s logic): prepare a generation, run the app from it
//! supervised, and start it again only when it asks (`spec/architecture.md` §1.2). The window it drives is a [`Surface`];
//! the preparation and the app are a [`Preparer`] and an [`AppRunner`], so tests and captures stand in fakes and never
//! build or start anything.

use crate::arguments;
use crate::builder::{Prepared, Preparer};
use crate::process::Failure;
use crate::store::GenerationStore;
use eg_core::fault::FaultLog;
use eg_core::hosting::{self, REQUESTED_EXIT_CODE, RestartPolicy};
use eg_core::operations::Cancellation;
use eg_core::paths;
use eg_ui::preparation::Stage;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// What the preparation window does for the supervisor (the C# `PreparationView` and window calls). Called from the
/// supervisor's thread.
pub trait Surface: Send + Sync {
    /// Working again from the start.
    fn start(&self, message: &str);
    fn report(&self, stage: Stage, message: &str);
    /// The latest line of output.
    fn detail(&self, line: &str);
    /// A note that needs no stop.
    fn say(&self, message: &str);
    /// Stopped: the reason in full, and Retry and Open Settings.
    fn stop(&self, reason: &str, message: Option<&str>);
    fn show(&self);
    fn hide(&self);
    /// Ends the launcher with this code.
    fn exit(&self, code: i32);
}

/// How the app ended, and how long it had run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: i32,
    pub ran: Duration,
}

/// Starts the app and waits for it.
pub trait AppRunner: Send + Sync {
    /// Runs `executable` with `arguments` in `root`, calls `started` once it runs, and waits for it to exit. A
    /// cancelled wait leaves the app running.
    fn run(&self, executable: &Path, arguments: &[String], root: &Path, started: &dyn Fn(), cancel: &Cancellation) -> Result<Exit, Failure>;
}

pub struct SystemAppRunner;

impl AppRunner for SystemAppRunner {
    fn run(&self, executable: &Path, arguments: &[String], root: &Path, started: &dyn Fn(), cancel: &Cancellation) -> Result<Exit, Failure> {
        let ran = Instant::now();
        let mut child = Command::new(executable)
            .args(arguments)
            .current_dir(root)
            .spawn()
            .map_err(|error| Failure::Failed(format!("Cannot launch Eterra's Gift. {error}")))?;
        started();
        loop {
            if cancel.is_cancelled() {
                return Err(Failure::Cancelled);
            }
            if let Some(status) = child.try_wait()? {
                // A process ended without a code (killed) is a fault, never a request.
                return Ok(Exit {
                    code: status.code().unwrap_or(1),
                    ran: ran.elapsed(),
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub struct Supervisor<P, A, S> {
    root: PathBuf,
    /// The launcher's own folder: an install's app, or a checkout build to open Settings from.
    packaged: PathBuf,
    /// The launcher's command line, forwarded to the app.
    arguments: Vec<String>,
    runtime: PathBuf,
    preparer: P,
    apps: A,
    surface: S,
    restarts: Mutex<RestartPolicy>,
    running: AtomicBool,
    exiting: AtomicBool,
    lifetime: Cancellation,
    log: Mutex<()>,
}

impl<P: Preparer + Sync, A: AppRunner, S: Surface> Supervisor<P, A, S> {
    pub fn new(root: &Path, packaged: &Path, arguments: Vec<String>, runtime: &Path, preparer: P, apps: A, surface: S) -> Self {
        Supervisor {
            root: root.to_path_buf(),
            packaged: packaged.to_path_buf(),
            arguments,
            runtime: runtime.to_path_buf(),
            preparer,
            apps,
            surface,
            restarts: Mutex::new(RestartPolicy::default()),
            running: AtomicBool::new(false),
            exiting: AtomicBool::new(false),
            lifetime: Cancellation::default(),
            log: Mutex::new(()),
        }
    }

    pub fn surface(&self) -> &S {
        &self.surface
    }

    pub fn apps(&self) -> &A {
        &self.apps
    }

    /// The window closes: whatever runs is cancelled and the launcher ends.
    pub fn close(&self) {
        self.exit(0);
    }

    /// Ends the launcher with `code`, once.
    fn exit(&self, code: i32) {
        if self.exiting.swap(true, Ordering::SeqCst) {
            return;
        }
        self.lifetime.cancel();
        self.surface.exit(code);
    }

    /// Prepares a generation and runs the app from it until it exits for good (Opened, and Retry).
    pub fn prepare_and_run(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.surface.start("Checking application components…");
        let log = self.runtime.join("preparation.log");
        start_log(&log);
        let watch = Instant::now();
        let report = |line: &str| {
            self.append(&log, watch, line);
            self.surface.detail(line);
        };
        let progress = |stage: Stage, message: &str| {
            self.surface.report(stage, message);
            report(&format!("{stage:?}: {message}"));
        };
        match self.supervise(&progress, &report) {
            Ok(()) => {}
            Err(Failure::Cancelled) if self.lifetime.is_cancelled() => self.exit(0),
            Err(failure) => {
                let message = match failure {
                    Failure::Cancelled => "The operation was canceled.".to_owned(),
                    Failure::Failed(message) => message,
                };
                report(&message);
                FaultLog::new(&self.root, Some(&self.runtime)).write("Preparation", Some(&message), None, None);
                self.surface.stop(&message, Some("Preparation stopped. Resolve the problem below, then Retry."));
                self.surface.show();
            }
        }
        self.running.store(false, Ordering::SeqCst);
    }

    fn supervise(&self, progress: &dyn Fn(Stage, &str), report: &(dyn Fn(&str) + Sync)) -> Result<(), Failure> {
        fs::create_dir_all(&self.runtime)?;
        let mut generation = self.prepare(progress, report)?;
        while !self.lifetime.is_cancelled() {
            let exit = self.run_app(&generation.directory, false, Some(&generation.key))?;
            if exit.code == 0 {
                self.exit(0);
                return Ok(());
            }
            if exit.code != REQUESTED_EXIT_CODE {
                // The window has been hidden since the app started: an app that stopped with an error is said, not a
                // silent exit.
                report(&format!("Eterra's Gift stopped (exit code {}).", exit.code));
                self.surface.stop(
                    &format!(
                        "Eterra's Gift stopped unexpectedly (exit code {}). Its fault report is in the logs folder of its data folder.",
                        exit.code
                    ),
                    Some("Eterra's Gift stopped. Retry starts it again."),
                );
                self.surface.show();
                return Ok(());
            }
            let restart = self
                .restarts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .should_restart(exit.code, &generation.key, exit.ran);
            if !restart {
                return Err(Failure::Failed(
                    "The application requested another restart for the same generation. Retry after checking the preparation log.".into(),
                ));
            }
            self.surface.show();
            progress(Stage::Restarting, "Restarting Eterra's Gift…");
            generation = self.prepare(progress, report)?;
        }
        Ok(())
    }

    fn prepare(&self, progress: &dyn Fn(Stage, &str), report: &(dyn Fn(&str) + Sync)) -> Result<Prepared, Failure> {
        self.preparer.prepare(&self.root, &self.packaged, progress, report, &self.lifetime)
    }

    /// Opens the app's Settings from the newest build there is, without connecting; a Restart now there prepares and
    /// starts again, as Retry does.
    pub fn open_settings(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut restart = false;
        let active = GenerationStore::new(&self.runtime).active_directory();
        let candidates = active
            .into_iter()
            .chain([self.packaged.clone(), self.root.join("target").join("release"), self.root.join("target").join("debug")]);
        let directory = candidates.into_iter().find(|directory| directory.join(paths::EXECUTABLE).is_file());
        match directory {
            None => self
                .surface
                .say("Settings will be available after the application builds. Check the preparation details, then Retry."),
            // Settings runs supervised, so it offers Restart now: that asks for the start a Retry makes.
            Some(directory) => match self.run_app(&directory, true, None) {
                Ok(exit) => restart = exit.code == REQUESTED_EXIT_CODE,
                Err(Failure::Cancelled) => {}
                Err(Failure::Failed(message)) => {
                    self.surface.stop(&message, None);
                    self.surface.show();
                }
            },
        }
        self.running.store(false, Ordering::SeqCst);
        if restart {
            self.prepare_and_run();
        }
    }

    fn run_app(&self, directory: &Path, settings: bool, generation: Option<&str>) -> Result<Exit, Failure> {
        let mut arguments = arguments::for_app(&self.arguments, &self.root);
        if let Some(generation) = generation.filter(|generation| *generation != hosting::PACKAGED) {
            arguments.extend(["--generation".to_owned(), generation.to_owned()]);
        }
        if settings {
            arguments.extend(["--settings".to_owned(), "--no-connect".to_owned()]);
        }
        let exit = self
            .apps
            .run(&directory.join(paths::EXECUTABLE), &arguments, &self.root, &|| self.surface.hide(), &self.lifetime)?;
        if settings {
            self.surface.show();
        }
        Ok(exit)
    }

    /// Logging cannot prevent preparation feedback or shutdown.
    fn append(&self, log: &Path, watch: Instant, line: &str) {
        let _guard = self.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = jiff::Timestamp::now();
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(file, "{now} {}ms {line}", watch.elapsed().as_millis());
        }
    }
}

/// The preparation log holds one preparation, build output included; the one before it stays beside it as
/// `preparation.previous.log`, so the log never grows past two runs. Logging cannot prevent preparation: a log that
/// can't be set aside is appended to.
pub fn start_log(log: &Path) {
    if log.is_file() {
        let _ = fs::rename(log, log.with_extension("previous.log"));
    }
}

#[cfg(test)]
mod tests;
