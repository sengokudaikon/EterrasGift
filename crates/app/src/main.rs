#![windows_subsystem = "windows"]
#![deny(unsafe_code)]
//! Eterra's Gift: the composition. It finds the checkout or install it belongs to, opens the settings, builds the
//! companion's services and the window, and runs it (`spec/architecture.md` §5).
#[allow(unsafe_code)] // Composition of the explicitly unsafe native trainer adapter.
mod trainer;

use eg_companion::host::FileGameHost;
use eg_companion::launcher::GameLauncher;
use eg_companion::mods::{
    library::ModLibrary,
    sources::{ModSources, SystemProcessRunner},
    worker::ModWorker,
};
use eg_companion::{dotnet, folders};
use eg_core::fault::FaultLog;
use eg_core::paths;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::{SaveOutcome, SettingsService};
use eg_ui::settings::SettingsPage;
use eg_ui::shell::Page;
use eg_ui::trainer_settings;
use eg_ui::{App, MainWindow};
use slint::ComponentHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The app's command line (`spec/shell-and-settings.md` §1).
#[derive(Debug, Default)]
struct Arguments {
    /// The checkout or install the app belongs to.
    root: Option<PathBuf>,
    /// Open on Settings.
    settings: bool,
    /// Do not begin trainer discovery or automatic connection.
    no_connect: bool,
}

impl Arguments {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Arguments {
        let mut parsed = Arguments::default();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--root" => parsed.root = arguments.next().map(PathBuf::from),
                "--settings" => parsed.settings = true,
                "--no-connect" => parsed.no_connect = true,
                _ => {}
            }
        }
        parsed
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let command_line: Vec<String> = std::env::args().skip(1).collect();
    let arguments = Arguments::parse(command_line.iter().cloned());
    let executable = std::env::current_exe().unwrap_or_default();
    let folder = executable.parent().unwrap_or(&executable).to_path_buf();
    let root = arguments.root.clone().unwrap_or_else(|| paths::find_root(&folder));

    // A panic anywhere is a fault: reported before the process ends with a non-zero code, so the app never starts
    // itself again after it.
    let early_faults = FaultLog::new(&root, None);
    std::panic::set_hook(Box::new(move |panic| {
        if trainer::catches_panic() {
            return;
        }
        let report = format!("{panic}\n{}", std::backtrace::Backtrace::force_capture());
        early_faults.write("Panic", Some(&report), None, None);
    }));

    // One app per settings file, taken before anything writes: two would each write their whole settings over the
    // other's and race on the mods. A restart hands over within the wait. A folder that can't hold the lock doesn't keep
    // the app from starting.
    let settings_file = SettingsStore::default_file();
    let instance = match eg_core::instance::acquire(&settings_file, Duration::from_secs(5)) {
        Ok(None) => {
            eg_ui::system::tell("Eterra's Gift", "Eterra's Gift is already running.");
            return Ok(());
        }
        Ok(Some(lock)) => Some(lock),
        Err(_) => None,
    };

    let store = SettingsStore::open(&root, &settings_file, &[trainer_settings::LEGACY]);
    let development = store.paths().is_source_checkout;
    let mut trainer = trainer_settings::descriptor(development);
    // A module that cannot start is reported as the C# DesktopBootstrapper.CreateModules words it, and the app runs
    // without it.
    let start_failed = |title: &str, error: &str| {
        FaultLog::new(&root, Some(&store.paths().log_directory())).write(&format!("Starting {title}"), Some(error), None, Some("Eterra's Gift runs without it."));
        format!("{title} could not start: {error} Details are in the fault log.")
    };
    // This run has the modules as their choices stood when it started.
    let trainer_chosen = store.startup_modules().get(trainer_settings::ID).copied().unwrap_or(trainer.default_on);
    // The trainer reads its settings as it starts (the C# TrainerComposition): a value it cannot read is a start that
    // failed, not a start on defaults. Off, it is only the reason Settings gives.
    if let Err(error) = trainer.validate(store.saved()) {
        trainer.problem = Some(if trainer_chosen { start_failed(trainer.title, &error) } else { error });
    }
    let trainer_on = trainer_chosen && trainer.problem.is_none();

    // Finds and starts the game from its files and Steam's; the game process itself is never touched. MelonLoader runs
    // from the game folder unless Settings chose another (read once: a new choice waits for a restart).
    let mods = ModWorker::new(ModLibrary::new(store.paths().mod_library_directory.clone()));
    let sources = Arc::new(ModSources::new(Arc::new(SystemProcessRunner)));
    let host = FileGameHost::new(store.active_game_executable(), &folders::player_log(), store.active_melon_loader_directory()).with_mod_worker(mods.clone());
    let launcher = Arc::new(GameLauncher::new(Arc::new(host)));
    // Looking for the game (Steam's registry and library files) and for the .NET SDK runs beside the window's
    // construction, and the game is looked for once. Mods never load online: a proxy a start with mods left in place (the
    // app closed while it waited) goes aside with that look. Both are done before the trainer starts and before the
    // window runs, so nothing can start the game first.
    let discovery = std::thread::spawn({
        let launcher = launcher.clone();
        move || {
            let install = launcher.find_install();
            launcher.settle_mod_loader(install.as_ref());
            (install.map(|install| install.executable), dotnet::found_directory())
        }
    });
    let window = MainWindow::new()?;
    let (found_game, found_dotnet) = discovery.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let trainer_runtime = if trainer_on {
        let flag = |key, default| eg_core::settings::module_flag(store.module_settings(trainer_settings::ID), key, default);
        let process_name = found_game
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Last Epoch.exe".into(), |name| name.to_string_lossy().into_owned());
        match trainer::Trainer::start(
            folders::player_log(),
            store.paths().module_directory(trainer_settings::ID).join("game-data"),
            found_game.as_ref().and_then(|exe| exe.parent()).map(Path::to_path_buf),
            process_name,
            eg_trainer::backend::BackendOptions {
                auto_connect: flag(trainer_settings::AUTO_CONNECT, true),
                override_enabled: flag(trainer_settings::OVERRIDE, false),
                ..Default::default()
            },
            arguments.no_connect,
            FaultLog::new(&root, Some(&store.paths().log_directory())),
        ) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                trainer.problem = Some(start_failed(trainer.title, &error));
                None
            }
        }
    } else {
        None
    };
    // Isolate an optional module's startup fault. Companion pages remain usable,
    // and Settings displays the fault without changing the saved module choice.
    let trainer_on = trainer_runtime.is_some();
    let start = if arguments.settings || store.startup_error().is_some() || trainer.problem.is_some() {
        Page::Settings
    } else {
        Page::Home
    };
    let service = SettingsService::new(store);
    // Settings offers Restart now when a choice waits for one: the app then starts itself again as it closes.
    let settings = SettingsPage::new(service.clone(), vec![trainer], found_game, found_dotnet);
    let restart = settings.restart_request();

    eg_ui::install(
        &window,
        App {
            launcher: Some(launcher.clone()),
            settings,
            mods: Some(eg_ui::mods::ModsPage {
                worker: mods.clone(),
                launcher: launcher.clone(),
                settings: service.clone(),
                sources: sources.clone(),
            }),
            trainer: trainer_on,
            trainer_backend: trainer_runtime.as_ref().map(|runtime| runtime.handle.clone()),
            start,
        },
    );
    eg_ui::follow_system_settings(&window);
    eg_ui::system::follow_dark_title_bar(&window);
    let result = window.run();
    let trainer_closed = trainer_runtime.is_none_or(|runtime| runtime.close());
    launcher.cancel();
    eg_ui::mods::cancel_work(&window);
    mods.cancellation().cancel();
    sources.close();
    let background_fault = eg_ui::background::shutdown();
    mods.close();
    let settings = service.close();
    // Stops a start with mods that still waits for MelonLoader, so its proxy is set aside before the app exits.
    launcher.stop();
    // A fault outranks a restart request, which outranks a plain close: the app never starts itself again after a fault.
    if !trainer_closed || background_fault || mods.faulted() || matches!(settings.outcome, SaveOutcome::Faulted(_)) {
        std::process::exit(1);
    }
    if result.is_ok() && restart.requested() {
        // Everything that wrote is closed, so the lock goes first and the new app takes it without waiting. It starts
        // from this same exe with this run's arguments: a checkout's new build is cargo's to make, before a start.
        drop(instance);
        if let Err(error) = std::process::Command::new(&executable).args(&command_line).spawn() {
            FaultLog::new(&root, Some(&settings.store.paths().log_directory())).write("Restarting Eterra's Gift", Some(&error.to_string()), None, None);
            std::process::exit(1);
        }
    }
    result
}
