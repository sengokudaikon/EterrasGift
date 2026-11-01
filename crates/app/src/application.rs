//! Wire the window to its services, then close them in dependency order before considering a restart.

use crate::{
    companion::Companion,
    startup::{self, Startup},
    trainer,
};
use eg_core::fault::FaultLog;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::{SaveOutcome, SettingsService, Snapshot};
use eg_runtime::Runtime;
use eg_ui::pages::settings::SettingsPage;
use eg_ui::pages::settings::trainer as trainer_settings;
use eg_ui::shell::Page;
use eg_ui::{App, MainWindow};
use slint::ComponentHandle;
use std::process::ExitCode;

pub fn run(startup: Startup) -> Result<ExitCode, slint::PlatformError> {
    let store = SettingsStore::open(&startup.root, &startup.settings_file, &[trainer_settings::LEGACY]);
    let faults = FaultLog::new(&startup.root, Some(&store.paths().log_directory()));
    startup::install_panic_hook(faults.clone());
    let runtime = Runtime::start().expect("the runtime for background work starts");
    eg_ui::background::install(runtime.tasks());
    let companion = Companion::new(&store);
    let discovery = companion.discover();
    let window = MainWindow::new();
    // Everything the window and the trainer's checks say from here on is in the user's language.
    eg_ui::select_language();
    let mut trainer = trainer::Module::new(&store, &faults);
    // Discovery may be setting a mod-loader proxy aside: it finishes even when the window cannot open. A panic in it
    // has already been reported by the hook.
    let found = discovery.join();
    let window = window.inspect_err(|error| {
        faults.write("Opening the window", Some(&error.to_string()), None, None);
    })?;
    let (found_game, found_dotnet) = found.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let settings_unreadable = store.startup_error().is_some();
    let service = SettingsService::new(store);
    // The trainer saves its profiles through the service, so it starts once the service runs.
    let trainer_runtime = trainer.start(&service, found_game.as_deref(), startup.arguments.no_connect, &faults);
    let start = if startup.arguments.settings || settings_unreadable || trainer.choice.problem.is_some() {
        Page::Settings
    } else {
        Page::Home
    };
    let templates = trainer_runtime.as_ref().map(|_| trainer.templates());
    let settings = SettingsPage::new(service.clone(), vec![trainer.choice], found_game, found_dotnet);
    let restart = settings.restart_request();

    eg_ui::install(
        &window,
        App {
            launcher: Some(companion.launcher.clone()),
            settings,
            mods: Some(eg_ui::pages::mods::ModsPage {
                worker: companion.mods.clone(),
                launcher: companion.launcher.clone(),
                settings: service.clone(),
                sources: companion.sources.clone(),
            }),
            trainer: trainer_runtime.is_some(),
            trainer_backend: trainer_runtime.as_ref().map(|runtime| runtime.handle.clone()),
            faults: Some(faults.clone()),
            logs: Some(eg_ui::pages::logs::LogFiles {
                game: eg_companion::game::folders::player_log(),
                reports: faults.directories().collect(),
            }),
            templates,
            start,
        },
    );
    eg_ui::follow_system_settings(&window);
    eg_ui::system::follow_dark_title_bar(&window);
    let result = window.run();
    let Some(settings) = shutdown(&window, trainer_runtime, companion, runtime, service) else {
        // A shutdown fault outranks every restart request and the window's own result.
        return Ok(ExitCode::FAILURE);
    };
    // A window error also prevents a requested restart.
    if result.is_ok() && restart.requested() {
        let faults = FaultLog::new(&startup.root, Some(&settings.store.paths().log_directory()));
        if !startup.restart(&faults) {
            return Ok(ExitCode::FAILURE);
        }
    }
    result.map(|()| ExitCode::SUCCESS)
}

/// Drain accepted trainer work, cancel producers, then close writers and settle the game's mod-loader proxy.
/// The settings as closed, or None when a worker or a writer faulted: the app then exits with a non-zero code.
fn shutdown(window: &MainWindow, trainer: Option<trainer::Trainer>, companion: Companion, runtime: Runtime, settings: SettingsService) -> Option<Snapshot> {
    let trainer_closed = trainer.is_none_or(|runtime| runtime.close());
    companion.launcher.cancel();
    eg_ui::pages::mods::cancel_work(window);
    companion.mods.cancellation().cancel();
    companion.sources.close();
    // Before the mods worker closes: background work may be waiting on it.
    let background_fault = runtime.shutdown();
    companion.mods.close();
    let settings = settings.close();
    // A start with mods may still be waiting for MelonLoader; set its proxy aside before exiting.
    companion.launcher.stop();
    let faulted = !trainer_closed || background_fault || companion.mods.faulted() || matches!(settings.outcome, SaveOutcome::Faulted(_));
    (!faulted).then_some(settings)
}
