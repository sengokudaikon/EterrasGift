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
use eg_core::settings::SettingsStore;
use eg_core::settings_service::{SaveOutcome, SettingsService};
use eg_core::{hosting, paths};
use eg_ui::settings::SettingsPage;
use eg_ui::shell::Page;
use eg_ui::trainer_settings;
use eg_ui::{App, MainWindow};
use slint::ComponentHandle;
use std::path::PathBuf;
use std::sync::Arc;

/// The trainer helper's manifest beside the executable: written by `mise run trainer:helper:build` for a cargo build,
/// and by the launcher into each generation (`eterras_gift_launcher::HELPER_MANIFEST`), which checks it is there.
const HELPER_MANIFEST: &str = "eg_trainer_helper.json";

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
    /// The generation the launcher prepared this from; an unsupervised start checks it is whole.
    generation: Option<String>,
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
                "--generation" => parsed.generation = arguments.next(),
                _ => {}
            }
        }
        parsed
    }
}

/// Run by cargo (`mise run run`, `mise run live`): the C# app's "a debugger is attached", so it runs itself rather than
/// handing the start to a launcher that cargo also put in the target folder.
fn run_by_cargo() -> bool {
    std::env::var_os("CARGO").is_some() || std::env::var_os("CARGO_PKG_NAME").is_some()
}

fn main() -> Result<(), slint::PlatformError> {
    let command_line: Vec<String> = std::env::args().skip(1).collect();
    let arguments = Arguments::parse(command_line.iter().cloned());
    let executable = std::env::current_exe().unwrap_or_default();
    let folder = executable.parent().unwrap_or(&executable).to_path_buf();
    let root = arguments.root.clone().unwrap_or_else(|| paths::find_root(&folder));

    // Every normal start goes through the launcher (`spec/architecture.md` §1.2): started directly, the app starts the
    // launcher beside it with its own arguments and ends.
    let launcher_executable = folder.join(hosting::LAUNCHER_EXECUTABLE);
    if !arguments.supervised && !run_by_cargo() && launcher_executable.is_file() {
        if let Err(error) = std::process::Command::new(&launcher_executable).args(&command_line).spawn() {
            FaultLog::new(&root, None).write("Application startup or lifetime", Some(&error.to_string()), None, None);
            std::process::exit(1);
        }
        return Ok(());
    }
    // The supervising launcher hashed this generation just before starting it; only an unsupervised start checks it
    // here, and an incomplete one asks for a restart, which prepares it again.
    if !arguments.supervised
        && let Some(key) = &arguments.generation
        && !hosting::valid(&folder, key, &[paths::EXECUTABLE, HELPER_MANIFEST])
    {
        std::process::exit(hosting::REQUESTED_EXIT_CODE);
    }

    // A panic anywhere is a fault: reported before the process ends with a non-zero code, so a launcher never
    // restarts after it.
    let early_faults = FaultLog::new(&root, None);
    std::panic::set_hook(Box::new(move |panic| {
        if trainer::catches_panic() {
            return;
        }
        let report = format!("{panic}\n{}", std::backtrace::Backtrace::force_capture());
        early_faults.write("Panic", Some(&report), None, None);
    }));

    let store = SettingsStore::open(&root, &SettingsStore::default_file(), &[trainer_settings::LEGACY]);
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
            executable.with_file_name(HELPER_MANIFEST),
            folders::player_log(),
            store.paths().module_directory("trainer").join("catalog"),
            process_name,
            eg_trainer::backend::BackendOptions {
                auto_connect: automatic,
                override_enabled: store
                    .module_settings(trainer_settings::ID)
                    .and_then(|v| v.get("iKnowWhatImDoing"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
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
    // Under the launcher, Settings offers Restart now: the app then exits with 42 and is prepared and started again.
    let settings = SettingsPage::new(service.clone(), vec![trainer], found_game, found_dotnet, arguments.supervised);
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
    // A fault outranks a restart request, which outranks a plain close: the launcher never restarts after a fault.
    if !trainer_closed || background_fault || mods.faulted() || matches!(settings.outcome, SaveOutcome::Faulted(_)) {
        std::process::exit(1);
    }
    if result.is_ok() && restart.requested() {
        std::process::exit(hosting::REQUESTED_EXIT_CODE);
    }
    result
}
