//! The Slint UI: the `.slint` sources under `ui/`, compiled by `build.rs`, and the Rust that puts the app's state
//! behind them: navigation, Home, Settings, Eterra's decorations, and the Windows settings the design reacts to. What is not ported yet is
//! stubbed (`stubs`).

#![deny(unsafe_code)]

slint::include_modules!();

pub mod background;
pub mod decorations;
pub mod forge;
pub mod home;
mod models;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod mods;
pub mod settings;
pub mod shell;
pub mod stubs;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod system;
pub mod trainer;
pub mod trainer_backend;
pub mod trainer_settings;

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
    pub mods: Option<mods::ModsPage>,
    /// This run has the trainer: its pages, its connection block and its Settings groups.
    pub trainer: bool,
    pub trainer_backend: Option<eg_trainer::backend::BackendHandle>,
    /// Home, or Settings when the settings file or a module has something to say.
    pub start: Page,
}

/// Wires a new window: navigation, the pages and the Windows settings.
pub fn install(window: &MainWindow, app: App) {
    let trainer_pages = if app.trainer { Page::TRAINER.to_vec() } else { Vec::new() };
    let mut navigation = Navigation::new(app.mods.is_some(), trainer_pages);
    navigation.navigate(app.start);
    let shell = shell::bind(window, navigation);
    let decorations = decorations::Decorations::install(window);
    home::bind(window, app.launcher, &shell, decorations.clone());
    let before_close = app.mods.map(|page| mods::bind(window, page, &shell));
    // The trainer's pages remember the hooks' values in its slice of the settings.
    let trainer_settings = app.trainer.then(|| app.settings.service().module(trainer_settings::ID));
    // The trainer's switches reach its session as they flip; Game definitions follows its game data.
    let settings = match app.trainer_backend.clone().filter(|_| app.trainer) {
        Some(backend) => app.settings.link(trainer_settings::ID, trainer_settings::link(backend)),
        None => app.settings,
    };
    settings::bind_with_close(window, settings, before_close);
    stubs::install(window, app.trainer);
    let trainer_backend = app.trainer_backend.filter(|_| app.trainer);
    trainer::install(window, trainer_backend.clone(), trainer_settings);
    if let Some(backend) = trainer_backend {
        trainer_backend::bind(window, backend, &shell, decorations);
    }
    apply_system_settings(window);
}

/// Windows says nothing when "Animation effects" or high contrast changes, and Slint surfaces no window activation to
/// read them again on (the C# app's moment), so they are read again every few seconds: two cheap system calls. Captures
/// hold their own settings, so the app starts this and they do not.
pub fn follow_system_settings<C>(window: &C)
where
    C: ComponentHandle + 'static,
    for<'a> Tokens<'a>: slint::Global<'a, C>,
{
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(2), {
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                apply_system_settings(&window);
            }
        }
    });
    // Dropping the timer would stop it; it runs for the process's life, idle once its window is gone.
    std::mem::forget(timer);
}

/// Pushes the Windows settings the design depends on into the `Tokens` global. Windows sends no event when
/// "Animation effects" changes, so it is read again while the window lives (spec/shell-and-settings.md §5).
pub fn apply_system_settings<C>(window: &C)
where
    C: ComponentHandle,
    for<'a> Tokens<'a>: slint::Global<'a, C>,
{
    let tokens = window.global::<Tokens>();
    // Set only on a change, so an unchanged reading wakes no binding.
    let (high_contrast, motion) = (system::high_contrast(), system::animations_enabled());
    if tokens.get_high_contrast() != high_contrast {
        tokens.set_high_contrast(high_contrast);
    }
    if tokens.get_motion() != motion {
        tokens.set_motion(motion);
    }
}
