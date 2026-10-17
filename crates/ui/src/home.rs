//! Home in the window: `HomeModel` (eg-companion) behind the `HomeState` global. Home follows the game only while it
//! shows, every `GameLauncher::WATCH_INTERVAL`; another page costs nothing.

use crate::background;
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
    visible: Cell<bool>,
    watch_timer: Timer,
}

/// `launcher` is None in a run that has none (tests): Home then shows the companion's own title.
pub fn bind(window: &MainWindow, launcher: Option<Arc<GameLauncher>>, shell: &ShellHandle) {
    let state = window.global::<HomeState>();
    state.set_has_launcher(launcher.is_some());
    let Some(launcher) = launcher else {
        return;
    };
    let home = Rc::new(Home {
        launcher,
        model: RefCell::new(HomeModel::default()),
        window: window.as_weak(),
        visible: Cell::new(shell.current() == Page::Home),
        watch_timer: Timer::default(),
    });
    state.on_start({
        let home = home.clone();
        move |mode| home.play(mode)
    });
    if shell.current() == Page::Home {
        home.refresh();
    }
    shell.on_page_entered({
        let home = home.clone();
        move |page| {
            home.visible.set(page == Page::Home);
            if page == Page::Home {
                home.refresh();
            }
        }
    });
    home.watch_timer.start(TimerMode::Repeated, GameLauncher::WATCH_INTERVAL, {
        // The window's start callback keeps Home; the timer only looks in while it is there.
        let home = Rc::downgrade(&home);
        move || {
            if let Some(home) = home.upgrade()
                && home.visible.get()
            {
                home.watch();
            }
        }
    });
}

impl Home {
    /// Reads whether MelonLoader is beside the game and whether the game runs.
    fn refresh(&self) {
        self.model.borrow_mut().set_mod_loader(self.launcher.read_mod_loader());
        self.watch();
    }

    /// Looks at the game again: cheap enough to repeat while Home shows.
    fn watch(&self) {
        let running = self.launcher.find_running();
        let phase = HomeModel::phase_of(running.as_ref(), |game| self.launcher.read_log(game).readiness);
        self.model.borrow_mut().set_phase(phase);
        self.show();
    }

    fn play(self: &Rc<Self>, mode: ViewMode) {
        let mode = match mode {
            ViewMode::Online => LaunchMode::Online,
            ViewMode::Offline => LaunchMode::Offline,
            ViewMode::WithMods => LaunchMode::OfflineWithMods,
        };
        if !self.model.borrow().can_play() {
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

    fn show(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let model = self.model.borrow();
        let state = window.global::<HomeState>();
        state.set_phase(match model.phase() {
            BloomPhase::Wilted => ViewPhase::Wilted,
            BloomPhase::Budding => ViewPhase::Budding,
            BloomPhase::Opening => ViewPhase::Opening,
            BloomPhase::Bloom => ViewPhase::Bloom,
        });
        state.set_can_start(model.can_play());
        state.set_can_start_with_mods(model.can_play_with_mods());
        state.set_online_tip(model.online_tip().into());
        state.set_offline_tip(model.offline_tip().into());
        state.set_with_mods_tip(model.with_mods_tip().into());
        state.set_message(model.message().unwrap_or_default().into());
        state.set_message_is_error(model.is_error());
    }
}
