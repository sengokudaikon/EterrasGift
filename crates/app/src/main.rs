#![windows_subsystem = "windows"]
//! Eterra's Gift: the composition. It finds the checkout or install it belongs to, opens the settings, builds the
//! companion's services and the window, and runs it (`spec/architecture.md` §5).

use eg_companion::host::FileGameHost;
use eg_companion::launcher::GameLauncher;
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
    /// Modules start no background work. TODO(trainer): the trainer reads it once it starts looking for the game.
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
    let host = FileGameHost::new(store.active_game_executable(), &folders::player_log(), store.active_melon_loader_directory());
    let launcher = Arc::new(GameLauncher::new(Arc::new(host)));
    // Mods never load online: a proxy a start with mods left in place (the app closed while it waited) goes aside now.
    launcher.settle_mod_loader();

    // A startup error opens Settings, which says why.
    let start = if arguments.settings || store.startup_error().is_some() || trainer.problem.is_some() {
        Page::Settings
    } else {
        Page::Home
    };
    let found_game = launcher.find_install().map(|install| install.executable);
    let service = SettingsService::new(store);
    // TODO(launcher): Restart now needs a launcher that starts the app again on exit code 42.
    let settings = SettingsPage::new(service.clone(), vec![trainer], found_game, dotnet::found_directory(), false);

    let window = MainWindow::new()?;
    eg_ui::install(
        &window,
        App {
            launcher: Some(launcher.clone()),
            settings,
            trainer: trainer_on,
            start,
        },
    );
    eg_ui::follow_system_settings(&window);
    let result = window.run();
    launcher.cancel();
    let background_fault = eg_ui::background::shutdown();
    let settings = service.close();
    // Stops a start with mods that still waits for MelonLoader, so its proxy is set aside before the app exits.
    launcher.stop();
    if background_fault || matches!(settings.outcome, SaveOutcome::Faulted(_)) {
        std::process::exit(1);
    }
    result
}
