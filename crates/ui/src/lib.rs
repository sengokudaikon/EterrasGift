//! The Slint UI: the `.slint` sources under `ui/`, compiled by `build.rs`, and the Rust that puts the app's state behind
//! them: navigation, the pages rewritten on `spiderland` (`pages`), Eterra's decorations, and the Windows settings the design
//! reacts to.

// Deny, not forbid: the code Slint generates allows unsafe for its vtables, and forbid refuses that.
#![deny(unsafe_code)]

slint::include_modules!();

pub mod decorations;
pub mod pages;
pub mod shell;
pub mod system;

/// The translations the build script bundled from `lang/`, as the Rust phrases read them.
mod translations {
    include!(concat!(env!("OUT_DIR"), "/translations.rs"));
}

use eg_companion::game::watch::GameWatch;
use eg_companion::launch::launcher::GameLauncher;
use eg_core::settings_page::Intent as SettingsIntent;
use pages::{forge, home, logs, mods, settings, trainer};
use shell::{Navigation, Page};
use slint::ComponentHandle;
use spiderland_slint::Background;
use std::rc::Rc;
use std::sync::Arc;

/// What a window shows in this run.
pub struct App {
    /// Starts the game from Home; None in a run without one (tests), and then Home shows the companion's own title.
    pub launcher: Option<Arc<GameLauncher>>,
    /// Home's look at the game. With a launcher but no watch, Home looks only when it asks (tests).
    pub game: Option<GameWatch>,
    /// Where pages run work that must not hold the UI thread.
    pub background: Background,
    /// Explicit file sources; tests supply their own or leave the viewer unconfigured.
    pub logs: Option<eg_core::logs::LogFiles>,
    pub mods: Option<mods::ModsPage>,
    /// This run has the trainer: its pages, its connection block and its Settings groups.
    pub trainer: bool,
    pub trainer_backend: Option<eg_trainer::backend::BackendHandle>,
    /// A source checkout: the trainer's pages also list features with no row yet.
    pub planned: bool,
    /// Where the Item Forge records an item it can't show; None in a run without a fault log.
    pub faults: Option<eg_core::fault::FaultLog>,
    /// Where the Item Forge keeps the user's item templates; None in tests, which set their own.
    pub templates: Option<eg_trainer::item::template::TemplateStore>,
    pub settings: settings::Settings,
    /// Home, or Settings when the settings file or a module has something to say.
    pub start: Page,
}

/// Shows the app in the user's language when a translation has it, else in English. Call it as soon as the first window
/// exists, before any page or message is built.
pub fn select_language() {
    spiderland_slint::select_language(translations::CATALOGS);
}

/// What a window was bound to: captures set up the forge's states through it.
pub struct Installed {
    /// The forge on the trainer's worker, in a run that has one.
    pub forge: Option<forge::ForgeHandle>,
}

/// Wires a new window: navigation, the pages and the Windows settings.
pub fn install(window: &MainWindow, app: App) -> Installed {
    let trainer_pages = if app.trainer { Page::TRAINER.to_vec() } else { Vec::new() };
    let mut navigation = Navigation::new(app.mods.is_some(), trainer_pages);
    navigation.navigate(app.start);
    let shell = shell::bind(window, navigation);
    let decorations = decorations::Decorations::install(window);
    home::bind(window, app.launcher, app.game, app.background.clone(), &shell, decorations.clone());
    let mods = app.mods.map(|page| mods::bind(window, page, app.background.clone(), &shell));
    let trainer_backend = app.trainer_backend.filter(|_| app.trainer);
    let background = app.background;
    logs::bind(window, app.logs, trainer_backend.clone(), background.clone(), &shell);
    // The trainer's switches reach its session as they flip; Game definitions follows its game data.
    let settings_page = match &trainer_backend {
        Some(backend) => app.settings.link(settings::trainer::ID, settings::trainer::link(backend.clone())),
        None => app.settings,
    };
    // The window's close goes through Mods first, which keeps its drafts and finishes its work, and then through Settings,
    // which waits for saves. Either one can keep the window open to show what failed.
    let close_first = mods.clone().map(|mods| Rc::new(move || mods.close()) as Rc<dyn Fn()>);
    let settings = settings::bind(window, settings_page, close_first);
    if let Some(mods) = mods {
        mods.on_closed(move |closed| settings.send(if closed { SettingsIntent::CloseRequested } else { SettingsIntent::CloseAbandoned }));
    }
    trainer::connection::unbound(window, app.trainer);
    trainer::install(window, trainer_backend.clone(), app.planned, background.clone());
    let forge = trainer_backend.map(|backend| {
        trainer::connection::bind(window, backend.clone(), &shell, decorations);
        forge::bind(window, backend, background, &shell, app.faults, app.templates)
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
        spiderland_phrases::Translations::new(super::translations::CATALOGS).unwrap();
    }
}
