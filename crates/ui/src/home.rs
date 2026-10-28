//! Home in the window: `HomeModel` (eg-companion) behind the `HomeState` global. Home follows the game only while it
//! shows, every `GameLauncher::WATCH_INTERVAL`; another page costs nothing. Eterra's tree follows the game's phase, and
//! ash drifts while the game is on its way in.

use crate::background;
use crate::decorations::{Ash, Decorations};
use crate::shell::{Page, ShellHandle};
use crate::{BloomPhase as ViewPhase, HomeState, LaunchMode as ViewMode, MainWindow};
use eg_companion::home::HomeModel;
use eg_companion::host::LaunchMode;
use eg_companion::launcher::GameLauncher;
use eg_core::bloom::BloomPhase;
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

struct Home {
    launcher: Arc<GameLauncher>,
    model: RefCell<HomeModel>,
    window: Weak<MainWindow>,
    watch_timer: Timer,
    decorations: Rc<Decorations<MainWindow>>,
    /// Home shows: it follows the game and owns the decorations only then.
    visible: Cell<bool>,
    /// Home is coming into view: the tree is shown where it is, without blooming there.
    entering: Cell<bool>,
    /// A look at the game is on its way back from its thread; another waits for it rather than overlapping it.
    looking: Cell<bool>,
    /// A look asked for while one was out, and whether it reads MelonLoader's state too.
    again: Cell<Option<bool>>,
}

/// `launcher` is None in a run that has none (tests): Home then shows the companion's own title.
pub fn bind(window: &MainWindow, launcher: Option<Arc<GameLauncher>>, shell: &ShellHandle, decorations: Rc<Decorations<MainWindow>>) {
    let state = window.global::<HomeState>();
    state.set_has_launcher(launcher.is_some());
    let Some(launcher) = launcher else {
        return;
    };
    let home = Rc::new(Home {
        launcher,
        model: RefCell::new(HomeModel::default()),
        window: window.as_weak(),
        watch_timer: Timer::default(),
        decorations,
        visible: Cell::new(false),
        entering: Cell::new(true),
        looking: Cell::new(false),
        again: Cell::new(None),
    });
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
    /// Looks at the game every `WATCH_INTERVAL` while Home shows; on another page the timer stops, so it costs nothing.
    fn follow(self: &Rc<Self>, visible: bool) {
        self.visible.set(visible);
        if !visible {
            self.watch_timer.stop();
            return;
        }
        // The window's start callback keeps Home; the timer only holds it weakly.
        let home = Rc::downgrade(self);
        self.watch_timer.start(TimerMode::Repeated, GameLauncher::WATCH_INTERVAL, move || {
            if let Some(home) = home.upgrade() {
                home.watch();
            }
        });
    }

    /// Reads whether MelonLoader is beside the game and whether the game runs.
    fn refresh(self: &Rc<Self>) {
        self.look(true);
    }

    /// Looks at the game again: cheap enough to repeat while Home shows.
    fn watch(self: &Rc<Self>) {
        self.look(false);
    }

    /// Finding the install reads Steam's registry and library files, and the watch takes a process snapshot and reads
    /// Player.log: all of it on a thread of its own, so a slow disk never holds the window.
    fn look(self: &Rc<Self>, loader: bool) {
        if self.looking.replace(true) {
            self.again.set(Some(loader || self.again.get().unwrap_or(false)));
            return;
        }
        let launcher = self.launcher.clone();
        let home = self.clone();
        background::run(
            move || {
                let loader = loader.then(|| launcher.read_mod_loader());
                let running = launcher.find_running();
                let phase = HomeModel::phase_of(running.as_ref(), |game| launcher.read_log(game).readiness);
                (loader, phase)
            },
            move |(loader, phase)| {
                home.looking.set(false);
                {
                    let mut model = home.model.borrow_mut();
                    if let Some(loader) = loader {
                        model.set_mod_loader(loader);
                    }
                    model.set_phase(phase);
                }
                home.show();
                if let Some(loader) = home.again.take() {
                    home.look(loader);
                }
            },
        );
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
