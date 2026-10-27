//! Wire the window to its services, then close them in dependency order before considering a restart.

use crate::{companion::Companion, startup::Startup, trainer};
use eg_core::fault::FaultLog;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::{SaveOutcome, SettingsService, Snapshot};
use eg_ui::settings::SettingsPage;
use eg_ui::shell::Page;
use eg_ui::trainer_settings;
use eg_ui::{App, MainWindow};
use slint::ComponentHandle;

pub fn run(startup: Startup) -> Result<(), slint::PlatformError> {
    let store = SettingsStore::open(&startup.root, &startup.settings_file, &[trainer_settings::LEGACY]);
    let faults = FaultLog::new(&startup.root, Some(&store.paths().log_directory()));
    let mut trainer = trainer::Module::new(&store, &faults);
    let companion = Companion::new(&store);
    let discovery = companion.discover();
    let window = MainWindow::new()?;
    let (found_game, found_dotnet) = discovery.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let service = SettingsService::new(store);
    // The trainer saves its profiles through the service, so it starts once the service runs.
    let trainer_runtime = trainer.start(&service, found_game.as_deref(), startup.arguments.no_connect, &faults);
    let start = if startup.arguments.settings || service.snapshot().store.startup_error().is_some() || trainer.choice.problem.is_some() {
        Page::Settings
    } else {
        Page::Home
    };
    let settings = SettingsPage::new(service.clone(), vec![trainer.choice], found_game, found_dotnet);
    let restart = settings.restart_request();

    eg_ui::install(
        &window,
        App {
            launcher: Some(companion.launcher.clone()),
            settings,
            mods: Some(eg_ui::mods::ModsPage {
                worker: companion.mods.clone(),
                launcher: companion.launcher.clone(),
                settings: service.clone(),
                sources: companion.sources.clone(),
            }),
            trainer: trainer_runtime.is_some(),
            trainer_backend: trainer_runtime.as_ref().map(|runtime| runtime.handle.clone()),
            faults: Some(faults.clone()),
            start,
        },
    );
    eg_ui::follow_system_settings(&window);
    eg_ui::system::follow_dark_title_bar(&window);
    let result = window.run();
    let settings = shutdown(&window, trainer_runtime, companion, service);
    // Shutdown faults exit first. A window error also prevents a requested restart.
    if result.is_ok() && restart.requested() {
        let faults = FaultLog::new(&startup.root, Some(&settings.store.paths().log_directory()));
        startup.restart(&faults);
    }
    result
}

/// Drain accepted trainer work, cancel producers, then close writers and settle the game's mod-loader proxy.
/// A worker fault outranks every restart request and ordinary close.
fn shutdown(window: &MainWindow, trainer: Option<trainer::Trainer>, companion: Companion, settings: SettingsService) -> Snapshot {
    let trainer_closed = trainer.is_none_or(|runtime| runtime.close());
    companion.launcher.cancel();
    eg_ui::mods::cancel_work(window);
    companion.mods.cancellation().cancel();
    companion.sources.close();
    let background_fault = eg_ui::background::shutdown();
    companion.mods.close();
    let settings = settings.close();
    // A start with mods may still be waiting for MelonLoader; set its proxy aside before exiting.
    companion.launcher.stop();
    if !trainer_closed || background_fault || companion.mods.faulted() || matches!(settings.outcome, SaveOutcome::Faulted(_)) {
        std::process::exit(1);
    }
    settings
}
