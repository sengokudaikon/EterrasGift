//! Optional trainer startup and its background runtime. The window holds only its queue/watch handle.
use eg_companion::folders;
use eg_core::fault::FaultLog;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::{ModuleSettings, SettingsService};
use eg_trainer::{
    backend::{self, BackendHandle, BackendOptions, Command, SubmitError, SystemClock},
    profiles::{ProfileBook, ProfileSaver},
    sources::FileLogFactory,
};
use eg_trainer_host::desktop::{DesktopConnector, GameProcesses, HelperPackage};
use eg_ui::settings::ModuleChoice;
use eg_ui::trainer_settings;
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

/// The trainer's choice is fixed for this run. Startup faults leave the companion usable and appear in Settings
/// without changing the saved choice.
pub struct Module {
    pub choice: ModuleChoice,
    chosen: bool,
}

impl Module {
    pub fn new(store: &SettingsStore, faults: &FaultLog) -> Self {
        let mut choice = trainer_settings::descriptor(store.paths().is_source_checkout);
        let chosen = store.startup_modules().get(trainer_settings::ID).copied().unwrap_or(choice.default_on);
        // Unreadable trainer settings fail startup rather than falling back to defaults. Off, they are only the
        // reason Settings gives; the module has not attempted to start.
        if let Err(error) = choice.validate(store.saved()) {
            choice.problem = Some(if chosen { start_failed(faults, choice.title, &error) } else { error });
        }
        Self { choice, chosen }
    }

    pub fn start(&mut self, settings: &SettingsService, game: Option<&Path>, no_connect: bool, faults: &FaultLog) -> Option<Trainer> {
        if !self.chosen || self.choice.problem.is_some() {
            return None;
        }
        let module = settings.module(trainer_settings::ID);
        let saved = module.read();
        let flag = |key, default| eg_core::settings::module_flag(saved.as_ref(), key, default);
        let process_name = game
            .and_then(|p| p.file_name())
            .map_or_else(|| "Last Epoch.exe".into(), |name| name.to_string_lossy().into_owned());
        match Trainer::start(
            folders::player_log(),
            settings.snapshot().store.paths().module_directory(trainer_settings::ID).join("game-data"),
            game.and_then(Path::parent).map(Path::to_path_buf),
            process_name,
            BackendOptions {
                auto_connect: flag(trainer_settings::AUTO_CONNECT, true),
                override_enabled: flag(trainer_settings::OVERRIDE, false),
                profiles: ProfileBook::from_settings(saved.as_ref()),
                save_profiles: Some(profile_saver(module)),
                ..Default::default()
            },
            no_connect,
            faults.clone(),
        ) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                self.choice.problem = Some(start_failed(faults, self.choice.title, &error));
                None
            }
        }
    }
}

/// Saves the trainer's profiles in its settings slice. Changes in quick succession are saved once, with the latest
/// profiles: a save already queued takes them. A save the closing service refuses is dropped, as the hooks' values were.
fn profile_saver(settings: ModuleSettings) -> ProfileSaver {
    let latest: Arc<Mutex<Option<ProfileBook>>> = Arc::default();
    ProfileSaver::new(move |book| {
        if latest.lock().unwrap().replace(book.clone()).is_some() {
            return;
        }
        let queued = latest.clone();
        let result = settings.update(move |fields| {
            if let Some(book) = queued.lock().unwrap().take() {
                book.write_settings(fields);
            }
            Ok(())
        });
        if result.is_err() {
            latest.lock().unwrap().take();
        }
    })
}

/// Report a module that cannot start as the C# DesktopBootstrapper.CreateModules words it.
fn start_failed(faults: &FaultLog, title: &str, error: &str) -> String {
    faults.write(&format!("Starting {title}"), Some(error), None, Some("Eterra's Gift runs without it."));
    format!("{title} could not start: {error} Details are in the fault log.")
}

/// The trainer helper DLL, built with the app (an artifact dependency) and written out only when a game connects. Cargo
/// names the variable after the dependency and, as the package and library names differ, the library too.
static HELPER: &[u8] = include_bytes!(env!("CARGO_CDYLIB_FILE_EG_TRAINER_HELPER_eg_trainer_helper"));

/// The embedded helper, known by its content and the source ID compiled into it.
fn helper() -> Result<HelperPackage, String> {
    HelperPackage::new(HELPER)
}

thread_local! {
    static WORKER_CATCHES_PANIC: Cell<bool> = const { Cell::new(false) };
}

pub fn catches_panic() -> bool {
    WORKER_CATCHES_PANIC.get()
}

pub struct Trainer {
    pub handle: BackendHandle,
    worker: JoinHandle<bool>,
}
impl Trainer {
    /// The helper is in the exe; it is written under the app's local folder only when a game connects. The game data
    /// is read from `game` (the installed game, when found) at once, and saved in `game_data`.
    pub fn start(
        player_log: PathBuf,
        game_data: PathBuf,
        game: Option<PathBuf>,
        executable_name: String,
        mut options: BackendOptions,
        no_connect: bool,
        faults: FaultLog,
    ) -> Result<Self, String> {
        let helper = helper()?;
        options.game_data = Some(eg_trainer::game_data::start(game, Some(game_data), Some(faults.clone())).map_err(|e| e.to_string())?);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        let logs = FileLogFactory::new(player_log).map_err(|e| e.to_string())?;
        let (handle, worker) = backend::service(
            GameProcesses::new(executable_name),
            DesktopConnector::new(helper, eg_core::paths::local_directory().join("helpers"), logs.clone()),
            logs.reader(),
            SystemClock,
            options,
        );
        let worker = worker.with_fault_log(faults.clone());
        let worker = thread::Builder::new()
            .name("trainer".into())
            .spawn(move || {
                run_worker(
                    || runtime.block_on(worker.run()),
                    &faults,
                    || {
                        let _ = slint::invoke_from_event_loop(|| {
                            let _ = slint::quit_event_loop();
                        });
                    },
                )
            })
            .map_err(|e| e.to_string())?;
        // No process lookup or preparation when --no-connect was requested.
        let _ = handle.try_submit(Command::Enable);
        if !no_connect {
            let _ = handle.try_submit(Command::StartMonitoring);
        }
        Ok(Self { handle, worker })
    }
    /// Called after the event loop ends. Accepted native work drains before cleanup;
    /// a full queue delays the barrier instead of silently abandoning the worker.
    pub fn close(self) -> bool {
        loop {
            match self.handle.try_submit(Command::Shutdown) {
                Ok(_) | Err(SubmitError::Stopped) => break,
                Err(SubmitError::Full) if self.worker.is_finished() => break,
                Err(SubmitError::Full) => thread::sleep(Duration::from_millis(10)),
            }
        }
        self.worker.join().is_ok_and(|completed| completed)
    }
}

/// Unexpected worker panics have the same fatal policy as Mods/Settings. Expected
/// connection and write failures are typed results and never enter this path.
fn run_worker(work: impl FnOnce(), faults: &FaultLog, shutdown: impl FnOnce()) -> bool {
    let previous = WORKER_CATCHES_PANIC.replace(true);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
    WORKER_CATCHES_PANIC.set(previous);
    match result {
        Ok(()) => true,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("Unknown panic payload");
            faults.write("Trainer worker", Some(message), None, Some("Unexpected worker failure; the app closes with exit code 1."));
            shutdown();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_helper_names_the_source_it_was_built_from() {
        // In every profile the app is built in: the session checks this ID against what the loaded helper reports.
        let helper = helper().unwrap();
        assert_eq!(helper.source_id().len(), 64);
    }

    #[test]
    fn a_worker_panic_records_its_fault_and_requests_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let faults = FaultLog::with_fallbacks(directory.path(), Some(directory.path()), vec![]);
        let mut shutdown = false;
        assert!(!run_worker(
            || {
                assert!(catches_panic());
                panic!("fixture worker fault")
            },
            &faults,
            || shutdown = true
        ));
        assert!(!catches_panic());
        assert!(shutdown);
        let reports = std::fs::read_dir(directory.path()).unwrap().collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        let text = std::fs::read_to_string(reports[0].as_ref().unwrap().path()).unwrap();
        assert!(text.contains("Trainer worker"));
        assert!(text.contains("fixture worker fault"));
        assert!(run_worker(|| {}, &faults, || panic!("normal completion must not close the app")));
    }
}
