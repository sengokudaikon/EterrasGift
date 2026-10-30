//! The Slint UI: the `.slint` sources under `ui/`, compiled by `build.rs`, and the Rust that puts the app's state
//! behind them: navigation, Home, Mods, Settings, the trainer's pages and Item Forge, Eterra's decorations, and the Windows
//! settings the design reacts to.

#![deny(unsafe_code)]

slint::include_modules!();

pub mod background;
pub mod decorations;
mod models;
pub mod pages;
pub mod shell;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod system;

/// The translations the build script bundled from `lang/`, as the Rust phrases read them.
mod translations {
    include!(concat!(env!("OUT_DIR"), "/translations.rs"));
}

use eg_companion::launch::launcher::GameLauncher;
use pages::settings::SettingsPage;
use pages::{forge, home, mods, settings, trainer};
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
    /// Where the trainer's pages record what they refuse; None in tests.
    pub faults: Option<eg_core::fault::FaultLog>,
    /// Where the Item Forge keeps the user's item templates; None in tests, which set their own.
    pub templates: Option<eg_trainer::item::template::TemplateStore>,
    /// Home, or Settings when the settings file or a module has something to say.
    pub start: Page,
}

/// What a window was bound to: captures drive the forge's editor through it.
pub struct Installed {
    /// The forge on the trainer's worker, in a run that has one.
    pub forge: Option<pages::forge::ForgeHandle>,
}

/// Shows the app in the user's language when a translation has it, else in English: the views through Slint's bundled
/// catalog and the Rust phrases through the same catalog's table, chosen once for both. Call it as soon as the first
/// window exists, before any page or message is built.
pub fn select_language() {
    let Ok(translations) = eg_core::phrase::Translations::new(translations::CATALOGS) else {
        return;
    };
    let language = sys_locale::get_locale().and_then(|locale| translations.choose(&locale));
    translations.select(language);
    // Fails only when no translation is bundled, and English needs none.
    let _ = slint::select_bundled_translation(language.unwrap_or(""));
}

/// Wires a new window: navigation, the pages and the Windows settings.
pub fn install(window: &MainWindow, app: App) -> Installed {
    let trainer_pages = if app.trainer { Page::TRAINER.to_vec() } else { Vec::new() };
    let mut navigation = Navigation::new(app.mods.is_some(), trainer_pages);
    navigation.navigate(app.start);
    let shell = shell::bind(window, navigation);
    let decorations = decorations::Decorations::install(window);
    home::bind(window, app.launcher, &shell, decorations.clone());
    let before_close = app.mods.map(|page| mods::bind(window, page, &shell));
    let trainer_backend = app.trainer_backend.filter(|_| app.trainer);
    // The trainer's switches reach its session as they flip; Game definitions follows its game data.
    let settings = match &trainer_backend {
        Some(backend) => app.settings.link(settings::trainer::ID, settings::trainer::link(backend.clone())),
        None => app.settings,
    };
    settings::bind_with_close(window, settings, before_close);
    trainer::connection::unbound(window, app.trainer);
    trainer::install(window, trainer_backend.clone());
    let forge = trainer_backend.map(|backend| {
        trainer::connection::bind(window, backend.clone(), &shell, decorations);
        let forge = forge::worker::bind(window, backend, &shell, app.faults);
        forge.edit(|editor| editor.templates = app.templates);
        forge
    });
    apply_system_settings(window);
    Installed { forge }
}

/// Windows says nothing when "Animation effects" or high contrast changes, and Slint surfaces no window activation to
/// read them again on, so they are read again every few seconds: two cheap system calls. Captures
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

/// Pushes the Windows settings the design depends on into the `Tokens` global.
pub fn apply_system_settings<C>(window: &C)
where
    C: ComponentHandle,
    for<'a> Tokens<'a>: slint::Global<'a, C>,
{
    let tokens = window.global::<Tokens>();
    tokens.set_high_contrast(system::high_contrast());
    tokens.set_motion(system::animations_enabled());
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_bundled_translation_reads() {
        eg_core::phrase::Translations::new(super::translations::CATALOGS).unwrap();
    }
}
