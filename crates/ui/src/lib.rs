//! The Slint UI: the `.slint` sources under `ui/`, compiled by `build.rs`, and the Rust that puts the app's state
//! behind them: navigation, Home, Settings, and the Windows settings the design reacts to. What is not ported yet is
//! stubbed (`stubs`).

slint::include_modules!();

pub mod background;
pub mod home;
pub mod settings;
pub mod shell;
pub mod stubs;
pub mod system;

use eg_companion::launcher::GameLauncher;
use settings::SettingsPage;
use shell::{Navigation, Page};
use slint::ComponentHandle;
use std::sync::Arc;

/// What a window shows in this run.
pub struct App {
    /// Starts the game from Home; None in a run without one (tests), and then Home shows the companion's own title.
    pub launcher: Option<Arc<GameLauncher>>,
    pub settings: SettingsPage,
    /// This run has the trainer: its pages, its connection block and its Settings groups.
    pub trainer: bool,
    /// Home, or Settings when the settings file or a module has something to say.
    pub start: Page,
}

/// Wires a new window: navigation, the pages and the Windows settings.
pub fn install(window: &MainWindow, app: App) {
    let trainer_pages = if app.trainer { Page::TRAINER.to_vec() } else { Vec::new() };
    // TODO(mods): Mods is listed only when the launcher and the mod library exist.
    let mut navigation = Navigation::new(true, trainer_pages);
    navigation.navigate(app.start);
    let shell = shell::bind(window, navigation);
    home::bind(window, app.launcher, &shell);
    settings::bind(window, app.settings, app.trainer);
    stubs::install(window, app.trainer);
    apply_system_settings(window);
}

/// Pushes the Windows settings the design depends on into the `Tokens` global. Windows sends no event when
/// "Animation effects" changes, so call this again whenever the window is activated (spec/shell-and-settings.md).
pub fn apply_system_settings(window: &MainWindow) {
    let tokens = window.global::<Tokens>();
    tokens.set_high_contrast(system::high_contrast());
    tokens.set_motion(system::animations_enabled());
}
