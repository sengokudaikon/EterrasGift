//! The Slint UI: the Rust that puts the app's state behind the views of `eg_ui_view`, re-exported here, which compile on their
//! own: navigation, the pages rewritten on `spiderland` (`pages`), Eterra's decorations, and the Windows settings the design
//! reacts to.

#![forbid(unsafe_code)]

pub use eg_ui_view::*;

pub mod decorations;
pub mod pages;
pub mod shell;
pub mod system;
mod trail;

/// The translations the build script bundled from `lang/`, as the Rust phrases read them.
mod translations {
    include!(concat!(env!("OUT_DIR"), "/translations.rs"));
}

use eg_companion::game::watch::GameWatch;
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::mods::feature::{Intent as ModsIntent, Mods, Output as ModsOutput};
use eg_companion::settings::{Intent as SettingsIntent, Output as SettingsOutput};
use eg_trainer_screens::connection::WorkerChanged;
use eg_trainer_screens::settings as trainer_settings;
use pages::{forge, home, logs, mods, settings, trainer};
use shell::{Navigation, Page};
use slint::ComponentHandle;
use spiderland::{Lifecycle, Store};
use spiderland_slint::Background;
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
    pub logs: Option<eg_companion::logs::LogFiles>,
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

/// Shows the app in the user's language when a translation has it, else in English: the views through Slint's bundled
/// translation, the phrases through the bundled catalogs, and numbers in that language's symbols, chosen once for all. Call it
/// as soon as the first window exists, before any page or message is built.
pub fn select_language() {
    let translations = phrases::Translations::new(translations::CATALOGS).expect("a bundled catalog's plural rule can't be read");
    let language = sys_locale::get_locale().and_then(|locale| translations.choose(&locale));
    translations.select(language);
    phrases::locale::select(language);
    // Fails only when no translation is bundled, and English needs none.
    let _ = slint::select_bundled_translation(language.unwrap_or(""));
}

/// Mods' index among the pages that go first in the window's close: the only one.
const MODS_CLOSE: usize = 0;

/// What a window was bound to: captures set up the forge's states through it, and the app's shutdown stops the Mods page's
/// work.
pub struct Installed {
    /// The forge on the trainer's worker, in a run that has one.
    pub forge: Option<forge::ForgeHandle>,
    mods: Option<Store<Mods>>,
}

impl Installed {
    /// Cancels the Mods page's compiler work, even when the event loop ended on a fault rather than a close.
    pub fn cancel_work(&self) {
        if let Some(mods) = &self.mods {
            mods.send(ModsIntent::StopWork);
        }
    }
}

/// Wires a new window: navigation, the pages and the Windows settings.
pub fn install(window: &MainWindow, app: App) -> Installed {
    let trainer_pages = if app.trainer { Page::TRAINER.to_vec() } else { Vec::new() };
    let mut navigation = Navigation::new(app.mods.is_some(), trainer_pages);
    navigation.navigate(app.start);
    let shell = shell::bind(window, navigation);
    let decorations = decorations::Decorations::install(window);
    home::bind(window, app.launcher, app.game, app.background.clone(), &shell, decorations.clone());
    let mods = app.mods.map(|page| mods::build(window, page, app.background.clone(), &shell));
    let trainer_backend = app.trainer_backend.filter(|_| app.trainer);
    let background = app.background;
    // The trainer's switches reach its session as they flip; Game definitions follows its game data.
    let settings_page = match &trainer_backend {
        Some(backend) => app.settings.link(trainer_settings::ID, trainer_settings::link(backend.clone())),
        None => app.settings,
    };
    // Settings takes the window's close. Mods goes first, keeping its drafts and finishing its work, and then Settings waits
    // for saves. Either one can keep the window open to show what failed. The two are wired to each other before either
    // starts, so neither misses what the other does as it starts.
    let settings = settings::build(window, settings_page, usize::from(mods.is_some()), &shell);
    if let Some(mods) = &mods {
        let told = |output: &SettingsOutput| match output {
            SettingsOutput::Lifecycle(Lifecycle::CloseRequested) => Some(ModsIntent::CloseRequested),
            SettingsOutput::Lifecycle(Lifecycle::CloseCalledOff) => Some(ModsIntent::CloseCalledOff),
            _ => None,
        };
        settings.forward(&mods.intents(), told);
        mods.forward(&settings.intents(), |output| match output {
            ModsOutput::Closed(answer) => Some(SettingsIntent::CloseAnswered(MODS_CLOSE, *answer)),
            ModsOutput::ShowPage => None,
        });
    }
    // In the order the pages have always started: Mods, Logs, then Settings.
    let mods = mods.map(|mods| mods::bind(window, mods.start()));
    logs::bind(window, app.logs, trainer_backend.clone(), background.clone(), &shell);
    let settings = settings::bind(window, settings.start());
    trainer::connection::unbound(window, app.trainer);
    trainer::install(window, trainer_backend.clone(), app.planned, background.clone());
    let forge = trainer_backend.map(|backend| {
        // A linked module's state moves on with the worker's, so Settings reads its status sections again.
        let connection = trainer::connection::build(window, backend.clone(), &shell, decorations);
        connection.forward(&settings.intents(), |_: &WorkerChanged| SettingsIntent::LinkChanged);
        trainer::connection::bind(window, connection.start());
        forge::bind(window, backend, background, &shell, app.faults, app.templates)
    });
    apply_system_settings(window);
    Installed { forge, mods }
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
        phrases::Translations::new(super::translations::CATALOGS).unwrap();
    }
}
