//! Home in the window: the `eg_companion::home` feature's impure half. It shows the feature's state in the `HomeState`
//! global, hears the game's watch, follows it while the feature wants, and starts the game and moves Eterra's tree when the
//! feature says to.

use crate::decorations::{Ash, Decorations};
use crate::shell::{Page, ShellHandle};
use crate::{BloomPhase as ViewPhase, HomeState, LaunchMode as ViewMode, MainWindow};
use eg_companion::game::watch::GameWatch;
use eg_companion::home::{Effect, Home, HomeView, Intent, State, Subscription, present};
use eg_companion::launch::host::LaunchMode;
use eg_companion::launch::launcher::GameLauncher;
use eg_core::bloom::BloomPhase;
use slint::ComponentHandle;
use spiderland::{Intents, Slice, Store, Version, callbacks};
use spiderland_slint::{Background, WindowHost, Windowed, on_watch};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

struct Window {
    launcher: Arc<GameLauncher>,
    game: GameWatch,
    background: Background,
    decorations: Rc<Decorations<MainWindow>>,
    view: Slice<Version>,
}

/// `launcher` is None in a run that has none (tests): Home then shows the companion's own title. With a launcher but no
/// watch, it looks only when it asks, on the spot (tests).
pub fn bind(
    window: &MainWindow,
    launcher: Option<Arc<GameLauncher>>,
    game: Option<GameWatch>,
    background: Background,
    shell: &ShellHandle,
    decorations: Rc<Decorations<MainWindow>>,
) {
    let state = window.global::<HomeState>();
    state.set_has_launcher(launcher.is_some());
    let Some(launcher) = launcher else {
        return;
    };
    let store = Store::<Home>::new(
        State::default(),
        Windowed::new(
            window,
            Window {
                game: game.unwrap_or_else(|| GameWatch::inline(launcher.clone())),
                launcher,
                background,
                decorations,
                view: Slice::default(),
            },
        ),
    );
    // The window's callback keeps the store alive, for as long as the window lasts.
    callbacks!(state, store, {
        on_start: |mode| Intent::Start(mode_of(mode)),
    });
    shell.on_shown(Page::Home, move |shown| store.send(if shown { Intent::Shown } else { Intent::Hidden }));
}

fn mode_of(mode: ViewMode) -> LaunchMode {
    match mode {
        ViewMode::Online => LaunchMode::Online,
        ViewMode::Offline => LaunchMode::Offline,
        ViewMode::WithMods => LaunchMode::OfflineWithMods,
    }
}

fn phase_of(phase: BloomPhase) -> ViewPhase {
    match phase {
        BloomPhase::Wilted => ViewPhase::Wilted,
        BloomPhase::Budding => ViewPhase::Budding,
        BloomPhase::Opening => ViewPhase::Opening,
        BloomPhase::Bloom => ViewPhase::Bloom,
    }
}

fn show(state: &HomeState<'_>, view: HomeView) {
    state.set_phase(phase_of(view.phase));
    state.set_game_starting(view.game_starting);
    state.set_can_start(view.can_start);
    state.set_can_start_with_mods(view.can_start_with_mods);
    state.set_online_tip(view.online_tip.into());
    state.set_offline_tip(view.offline_tip.into());
    state.set_with_mods_tip(view.with_mods_tip.into());
    state.set_message(view.message.into());
    state.set_message_is_error(view.message_is_error);
}

impl WindowHost<Home> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
        self.view.update(state.model().version(), || show(&window.global::<HomeState>(), present(state.model())));
    }

    fn subscribe(&mut self, subscription: &Subscription, intents: &Intents<Intent>) -> Box<dyn Any> {
        let game = &self.game;
        match subscription {
            Subscription::Hear => {
                let seen = game.clone();
                Box::new(on_watch(game.subscribe(), intents, move || Intent::Seen(seen.current())))
            }
            Subscription::Follow => Box::new(game.follow()),
        }
    }

    fn perform(&mut self, _: Option<&MainWindow>, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::Refresh => self.game.refresh(),
            Effect::Launch(mode) => {
                let launcher = self.launcher.clone();
                self.background.run(intents, move || launcher.play(mode), Intent::Started);
            }
            Effect::Tree { phase, ash, entering } => {
                let ash = if ash { Ash::Drifting } else { Ash::Hidden };
                if entering {
                    self.decorations.enter(phase, ash);
                } else {
                    self.decorations.follow(phase, ash);
                }
            }
        }
    }
}
