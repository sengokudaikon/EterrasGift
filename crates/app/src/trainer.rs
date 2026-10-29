//! Optional trainer startup and its background runtime. The window holds only its queue/watch handle.
use eg_companion::{game::folders, game::install};
use eg_core::fault::FaultLog;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::{ModuleSettings, SettingsService};
use eg_trainer::connect::host::{DesktopConnector, GameProcesses, HelperPackage};
use eg_trainer::{
    backend::{self, BackendHandle, BackendOptions, Command, SubmitError, SystemClock},
    connect::sources::FileLogFactory,
    features::profiles::{ProfileBook, ProfileSaver},
};
use eg_ui::pages::settings::ModuleChoice;
use eg_ui::pages::settings::trainer as trainer_settings;
use std::{
    cell::Cell,
    error::Error,
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
    /// Where the game data read from the install is saved between runs.
    game_data: PathBuf,
    /// Where the Item Forge keeps the user's item templates.
    templates: PathBuf,
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
        let game_data = store.paths().module_directory(trainer_settings::ID).join("game-data");
        let templates = store.paths().module_directory(trainer_settings::ID).join("forge-templates");
        Self {
            choice,
            chosen,
            game_data,
            templates,
        }
    }

    /// The Item Forge's template store.
    pub fn templates(&self) -> eg_trainer::item::template::TemplateStore {
        eg_trainer::item::template::TemplateStore::new(self.templates.clone())
    }

    pub fn start(&mut self, settings: &SettingsService, game: Option<&Path>, no_connect: bool, faults: &FaultLog) -> Option<Trainer> {
        if !self.chosen || self.choice.problem.is_some() {
            return None;
        }
        match self.launch(settings, game, no_connect, faults) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                self.choice.problem = Some(start_failed(faults, self.choice.title, &error.to_string()));
                None
            }
        }
    }

    /// The helper is in the exe; it is written under the app's local folder only when a game connects. The game data
    /// is read from `game`'s folder (the installed game, when found) at once, and saved in the module's folder.
    fn launch(&self, settings: &SettingsService, game: Option<&Path>, no_connect: bool, faults: &FaultLog) -> Result<Trainer, Box<dyn Error>> {
        let helper = helper()?;
        let module = settings.module(trainer_settings::ID);
        let saved = module.read();
        let flag = |key, default| eg_core::settings::module_flag(saved.as_ref(), key, default);
        let options = BackendOptions {
            auto_connect: flag(trainer_settings::AUTO_CONNECT, true),
            override_enabled: flag(trainer_settings::OVERRIDE, false),
            profiles: ProfileBook::from_settings(saved.as_ref()),
            save_profiles: Some(profile_saver(module)),
            game_data: Some(eg_trainer::definitions::service::start(
                game.and_then(Path::parent).map(Path::to_path_buf),
                Some(self.game_data.clone()),
                Some(faults.clone()),
            )?),
            ..Default::default()
        };
        let process_name = game
            .and_then(Path::file_name)
            .map_or_else(|| install::EXECUTABLE_NAME.into(), |name| name.to_string_lossy().into_owned());
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let logs = FileLogFactory::new(folders::player_log())?;
        let (handle, worker) = backend::service(
            GameProcesses::new(process_name),
            DesktopConnector::new(helper, eg_core::paths::local_directory().join("helpers"), logs.clone()),
            logs.reader(),
            SystemClock,
            options,
        );
        let worker = worker.with_fault_log(faults.clone());
        let worker_faults = faults.clone();
        let worker = thread::Builder::new().name("trainer".into()).spawn(move || {
            run_worker(
                || runtime.block_on(worker.run()),
                &worker_faults,
                || {
                    let _ = slint::invoke_from_event_loop(|| {
                        let _ = slint::quit_event_loop();
                    });
                },
            )
        })?;
        // No process lookup or preparation when --no-connect was requested. A refused command leaves the trainer
        // idle rather than failing the app, so it is only reported.
        let commands = [Some(Command::Enable), (!no_connect).then_some(Command::StartMonitoring)];
        for command in commands.into_iter().flatten() {
            if let Err(error) = handle.try_submit(command.clone()) {
                faults.write("Starting the trainer", Some(&format!("The worker refused {command:?}: {error:?}")), None, None);
            }
        }
        Ok(Trainer { handle, worker })
    }
}

/// Saves the trainer's profiles in its settings slice. Changes in quick succession are saved once, with the latest
/// profiles: a save already queued takes them. A save the closing service refuses is dropped: the app is closing, and
/// the profiles in memory end with it.
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

/// Report a module that cannot start: a fault report, and the reason Settings shows.
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
