//! Home in the window: `HomeModel` (eg-companion) behind the `HomeState` global. Home follows the game's [`GameWatch`] only
//! while it shows; another page costs nothing. Eterra's tree follows the game's phase, and ash drifts while the game is on
//! its way in.

use crate::background;
use crate::decorations::{Ash, Decorations};
use crate::shell::{Page, ShellHandle};
use crate::{BloomPhase as ViewPhase, HomeState, LaunchMode as ViewMode, MainWindow};
use eg_companion::game::watch::{Following, GameSeen, GameWatch};
use eg_companion::home::HomeModel;
use eg_companion::launch::host::LaunchMode;
use eg_companion::launch::launcher::GameLauncher;
use eg_core::bloom::BloomPhase;
use slint::{ComponentHandle, Weak};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::watch;

struct Home {
    launcher: Arc<GameLauncher>,
    model: RefCell<HomeModel>,
    window: Weak<MainWindow>,
    game: GameWatch,
    following: RefCell<Option<Following>>,
    /// The newest look not yet shown.
    latest: RefCell<watch::Receiver<GameSeen>>,
    decorations: Rc<Decorations<MainWindow>>,
    /// Home shows: it follows the game and owns the decorations only then.
    visible: Cell<bool>,
    /// Home is coming into view: the tree is shown where it is, without blooming there.
    entering: Cell<bool>,
}

/// `launcher` is None in a run that has none (tests): Home then shows the companion's own title.
pub fn bind(window: &MainWindow, launcher: Option<Arc<GameLauncher>>, game: Option<GameWatch>, shell: &ShellHandle, decorations: Rc<Decorations<MainWindow>>) {
    let state = window.global::<HomeState>();
    state.set_has_launcher(launcher.is_some());
    let Some(launcher) = launcher else {
        return;
    };
    let game = game.unwrap_or_else(|| GameWatch::inline(launcher.clone()));
    let home = Rc::new(Home {
        launcher,
        model: RefCell::new(HomeModel::default()),
        window: window.as_weak(),
        latest: RefCell::new(game.subscribe()),
        game,
        following: RefCell::new(None),
        decorations,
        visible: Cell::new(false),
        entering: Cell::new(true),
    });
    home.listen();
    state.on_start({
        let home = home.clone();
        move |mode| home.play(mode)
    });
    home.follow(shell.current() == Page::Home);
    if shell.current() == Page::Home {
        home.refresh();
    }
    shell.on_page_entered({
        let home = home.clone();
        move |page| {
            home.follow(page == Page::Home);
            if page == Page::Home {
                home.entering.set(true);
                home.refresh();
            }
        }
    });
}

impl Home {
    /// Shows each look as it comes. The window's start callback keeps Home; this only holds it weakly.
    fn listen(self: &Rc<Self>) {
        let mut updates = self.game.subscribe();
        let home = Rc::downgrade(self);
        // Fails only where no event loop can ever run, and then nothing could show a look anyway.
        let _ = slint::spawn_local(async move {
            while updates.changed().await.is_ok() {
                let Some(home) = home.upgrade() else {
                    return;
                };
                home.take_latest();
            }
        });
    }

    /// Follows the game while Home shows; on another page the watch stops looking, so it costs nothing.
    fn follow(&self, visible: bool) {
        self.visible.set(visible);
        *self.following.borrow_mut() = visible.then(|| self.game.follow());
    }

    /// Reads whether MelonLoader is beside the game and whether the game runs.
    fn refresh(&self) {
        self.game.refresh();
        self.take_latest();
    }

    fn take_latest(&self) {
        let mut latest = self.latest.borrow_mut();
        if !latest.has_changed().unwrap_or(false) {
            return;
        }
        let seen = *latest.borrow_and_update();
        drop(latest);
        {
            let mut model = self.model.borrow_mut();
            if let Some(loader) = seen.loader {
                model.set_mod_loader(loader);
            }
            model.set_phase(seen.phase());
        }
        self.show();
    }

    fn play(self: &Rc<Self>, mode: ViewMode) {
        let mode = match mode {
            ViewMode::Online => LaunchMode::Online,
            ViewMode::Offline => LaunchMode::Offline,
            ViewMode::WithMods => LaunchMode::OfflineWithMods,
        };
        if !self.can_play() {
            return;
        }
        self.model.borrow_mut().begin(mode == LaunchMode::OfflineWithMods);
        self.show();
        let launcher = self.launcher.clone();
        let home = self.clone();
        background::run(
            move || launcher.play(mode),
            move |report| {
                home.model.borrow_mut().finish(report);
                home.refresh();
            },
        );
    }

    fn can_play(&self) -> bool {
        self.model.borrow().can_play() && !self.launcher.is_starting()
    }

    fn show(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let model = self.model.borrow();
        let ash = if model.is_game_starting() { Ash::Drifting } else { Ash::Hidden };
        // A look or a start that ends while another page shows leaves that page's decorations alone.
        if self.visible.get() {
            if self.entering.replace(false) {
                self.decorations.enter(model.phase(), ash);
            } else {
                self.decorations.follow(model.phase(), ash);
            }
        }
        let state = window.global::<HomeState>();
        state.set_game_starting(model.is_game_starting());
        state.set_phase(match model.phase() {
            BloomPhase::Wilted => ViewPhase::Wilted,
            BloomPhase::Budding => ViewPhase::Budding,
            BloomPhase::Opening => ViewPhase::Opening,
            BloomPhase::Bloom => ViewPhase::Bloom,
        });
        // One launcher starts the game for every page, so a start begun on Mods holds Home's buttons too.
        let elsewhere = self.launcher.is_starting();
        state.set_can_start(model.can_play() && !elsewhere);
        state.set_can_start_with_mods(model.can_play_with_mods() && !elsewhere);
        state.set_online_tip(model.online_tip().as_ref().into());
        state.set_offline_tip(model.offline_tip().as_ref().into());
        state.set_with_mods_tip(model.with_mods_tip().as_ref().into());
        // What a start or a conflict said comes first; otherwise why a button waits, said in words as well as in its tip.
        match model.message() {
            Some(message) => {
                state.set_message(message.into());
                state.set_message_is_error(model.is_error());
            }
            None => {
                state.set_message(model.wait_reason().unwrap_or_default().into());
                state.set_message_is_error(false);
            }
        }
    }
}
