#![windows_subsystem = "windows"]
//! Eterra's Gift: the composition. It finds the checkout or install it belongs to, opens the settings, builds the
//! companion's services and the window, and runs it (`spec/architecture.md` §5).
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
use std::path::PathBuf;
use std::sync::Arc;

/// The app's command line (`spec/shell-and-settings.md` §1).
#[derive(Debug, Default)]
struct Arguments {
    /// The checkout or install the app belongs to.
    root: Option<PathBuf>,
    /// Run by the launcher, so Settings' Restart now can work.
    supervised: bool,
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
                "--supervised" => parsed.supervised = true,
                "--settings" => parsed.settings = true,
                "--no-connect" => parsed.no_connect = true,
                _ => {}
            }
        }
        parsed
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let arguments = Arguments::parse(std::env::args().skip(1));
    let executable = std::env::current_exe().unwrap_or_default();
    let root = arguments.root.clone().unwrap_or_else(|| paths::find_root(executable.parent().unwrap_or(&executable)));

    // A panic anywhere is a fault: reported before the process ends with a non-zero code, so a launcher never
    // restarts after it.
    let early_faults = FaultLog::new(&root, None);
    std::panic::set_hook(Box::new(move |panic| {
        let report = format!("{panic}\n{}", std::backtrace::Backtrace::force_capture());
        early_faults.write("Panic", Some(&report), None, None);
    }));

    let store = SettingsStore::open(&root, &SettingsStore::default_file(), &[trainer_settings::LEGACY]);
    let development = store.paths().is_source_checkout;
    let mut trainer = trainer_settings::descriptor(development);
    trainer.problem = trainer.validate(store.saved()).err();
    // This run has the modules as their choices stood when it started.
    let trainer_on = store.startup_modules().get(trainer_settings::ID).copied().unwrap_or(trainer.default_on) && trainer.problem.is_none();

    // Finds and starts the game from its files and Steam's; the game process itself is never touched. MelonLoader runs
    // from the game folder unless Settings chose another (read once: a new choice waits for a restart).
    let mods = ModWorker::new(ModLibrary::new(store.paths().mod_library_directory.clone()));
    let sources = Arc::new(ModSources::new(Arc::new(SystemProcessRunner)));
    let host = FileGameHost::new(store.active_game_executable(), &folders::player_log(), store.active_melon_loader_directory()).with_mod_worker(mods.clone());
    let launcher = Arc::new(GameLauncher::new(Arc::new(host)));
    // Mods never load online: a proxy a start with mods left in place (the app closed while it waited) goes aside now.
    launcher.settle_mod_loader();

    let found_game = launcher.find_install().map(|install| install.executable);
    let window = MainWindow::new()?;
    let trainer_runtime = if trainer_on {
        let automatic = store
            .module_settings(trainer_settings::ID)
            .and_then(|v| v.get("autoConnect"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let process_name = found_game
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Last Epoch.exe".into(), |name| name.to_string_lossy().into_owned());
        match trainer::Trainer::start(
            executable.with_file_name("eg_trainer_helper.json"),
            folders::player_log(),
            store.paths().module_directory("trainer").join("catalog"),
            process_name,
            automatic,
            arguments.no_connect,
        ) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                FaultLog::new(&root, None).write("Trainer startup", Some(&error), None, None);
                trainer.problem = Some(format!("The trainer could not start: {error}"));
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
    // TODO(launcher): Restart now needs a launcher that starts the app again on exit code 42.
    let settings = SettingsPage::new(service.clone(), vec![trainer], found_game, dotnet::found_directory(), false);

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
    if !trainer_closed || background_fault || mods.faulted() || matches!(settings.outcome, SaveOutcome::Faulted(_)) {
        std::process::exit(1);
    }
    result
}
