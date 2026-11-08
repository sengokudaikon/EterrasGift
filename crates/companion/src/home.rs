//! Home: it starts the game online, offline, or offline with mods, with Eterra's tree saying where the game is.
//!
//! [`Home`] is the feature. A page shown or left, what the game's watch sees and a start begun or finished are intents;
//! asking the watch to look, starting and moving the tree are effects. The UI crate performs those and shows [`present`]'s
//! output.

use crate::game::process::RunningGame;
use crate::game::watch::GameSeen;
use crate::launch::host::LaunchMode;
use crate::launch::launcher::{LaunchReport, StartingWithMods};
use crate::launch::loader::ModLoaderState;
use eg_core::bloom::BloomPhase;
use eg_core::game_log::GameReadiness;
use phrases::{Phrase, phrases};
use spiderland::{Effects, Feature, Panicked, Tracked};
use std::borrow::Cow;

phrases! {
    Online: "Start through Steam, without mods"
    Offline: "Start offline, without mods"
    WithMods: "Start offline with MelonLoader and the mods that are on"
    Running: "Last Epoch is already running."
    NoModLoader: "MelonLoader isn't in the game folder."
    Conflict: "The game folder has both version.dll and version.dll.disabled. Delete the older one, then start the game."
    LookFailed: "Home could not check on the game. Details are in the fault log."
    StartFailed: "The start stopped unexpectedly. Details are in the fault log."
}

/// Home as a feature: see the module's documentation.
pub struct Home;

pub enum Intent {
    /// Home came into view.
    Shown,
    /// Another page came into view.
    Hidden,
    /// What the game's watch sees now.
    Seen(GameSeen),
    Start(LaunchMode),
    Started(Result<LaunchReport, Panicked>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Ask the game's watch to look now, reading MelonLoader's state too.
    Refresh,
    Launch(LaunchMode),
    /// Move Eterra's tree to `phase`, and drift ash if `ash` is set. `entering` puts it there at once rather than blooming.
    Tree {
        phase: BloomPhase,
        ash: bool,
        entering: bool,
    },
}

#[derive(Debug, Default)]
pub struct State {
    model: Tracked<HomeModel>,
    visible: bool,
    /// Home came into view and hasn't moved the tree yet: the first move only places it.
    entering: bool,
    /// Where the tree was last told to go while Home has been showing.
    tree: Option<(BloomPhase, bool)>,
}

impl State {
    /// What `present` reads, so its version says when the view needs doing again.
    pub fn model(&self) -> &Tracked<HomeModel> {
        &self.model
    }
}

/// What runs while the state wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscription {
    /// Hearing what the game's watch sees, as [`Intent::Seen`]. It costs nothing, so it never stops: a look a start asked
    /// for lands while another page shows.
    Hear,
    /// Keeping the watch looking, which is what costs. Home follows the game only while it shows, so another page costs
    /// nothing.
    Follow,
}

impl Feature for Home {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = spiderland::Never;
    type Subscription = Subscription;

    fn subscriptions(state: &State) -> impl IntoIterator<Item = Subscription> {
        [Some(Subscription::Hear), state.visible.then_some(Subscription::Follow)].into_iter().flatten()
    }

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        match intent {
            Intent::Shown => {
                state.visible = true;
                state.entering = true;
                state.tree = None;
                effects.push(Effect::Refresh);
            }
            Intent::Hidden => {
                // Another page's decorations are its own from here on.
                state.visible = false;
                state.tree = None;
            }
            Intent::Seen(seen) => {
                // Most looks find what the last one did, and those leave the view's inputs untouched.
                let mut model = (*state.model).clone();
                if let Some(loader) = seen.loader {
                    model.set_mod_loader(loader);
                }
                model.set_phase(seen.phase());
                model.set_elsewhere(seen.starting);
                model.look_failed = seen.failed;
                state.model.set(model);
                move_tree(state, effects);
            }
            Intent::Start(mode) => {
                if state.model.can_play() {
                    state.model.begin(mode == LaunchMode::OfflineWithMods);
                    effects.push(Effect::Launch(mode));
                    move_tree(state, effects);
                }
            }
            Intent::Started(result) => {
                state.model.finish(result.unwrap_or_else(|Panicked| LaunchReport {
                    started: false,
                    message: StartFailed.into(),
                }));
                effects.push(Effect::Refresh);
            }
        }
    }
}

/// Says where the tree goes, when Home shows and that has changed. A look or start that ends while another page shows
/// leaves that page's decorations alone.
fn move_tree(state: &mut State, effects: &mut Effects<Effect>) {
    if !state.visible {
        return;
    }
    let target = (state.model.phase(), state.model.is_game_starting());
    let entering = std::mem::take(&mut state.entering);
    if entering || state.tree != Some(target) {
        state.tree = Some(target);
        effects.push(Effect::Tree {
            phase: target.0,
            ash: target.1,
            entering,
        });
    }
}

/// What the view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeView {
    pub phase: BloomPhase,
    pub game_starting: bool,
    pub can_start: bool,
    pub can_start_with_mods: bool,
    pub online_tip: String,
    pub offline_tip: String,
    pub with_mods_tip: String,
    pub message: String,
    pub message_is_error: bool,
}

pub fn present(model: &HomeModel) -> HomeView {
    // What a start or a conflict said comes first; otherwise why a button waits, said in words as well as in its tip.
    let (message, message_is_error) = match model.message() {
        Some(message) => (message.to_owned(), model.is_error()),
        None if model.look_failed => (LookFailed.into(), true),
        None => (model.wait_reason().unwrap_or_default().to_owned(), false),
    };
    HomeView {
        phase: model.phase(),
        game_starting: model.is_game_starting(),
        can_start: model.can_play(),
        can_start_with_mods: model.can_play_with_mods(),
        online_tip: model.online_tip().into_owned(),
        offline_tip: model.offline_tip().into_owned(),
        with_mods_tip: model.with_mods_tip().into_owned(),
        message,
        message_is_error,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeModel {
    /// A look has read MelonLoader's state. Until then Home doesn't claim it's missing.
    loader_known: bool,
    has_mod_loader: bool,
    /// Both MelonLoader proxies are in the game folder: no start can tell which is current, so none is offered.
    conflicted: bool,
    phase: BloomPhase,
    starting: bool,
    /// The launcher is busy with a start this page didn't begin.
    elsewhere: bool,
    /// The last look at the game failed, so what Home shows may be out of date.
    look_failed: bool,
    message: Option<String>,
    is_error: bool,
}

impl HomeModel {
    /// Eterra's tree follows the game from its own files: wilted with none running, budding once its process is there,
    /// opening while Player.log has no character yet, in bloom with one loaded (online or offline). Nothing is faked.
    pub fn phase_of(running: Option<&RunningGame>, readiness: impl FnOnce(&RunningGame) -> GameReadiness) -> BloomPhase {
        match running.map(readiness) {
            None => BloomPhase::Wilted,
            Some(GameReadiness::NoCharacter) => BloomPhase::Opening,
            Some(GameReadiness::Online | GameReadiness::Offline) => BloomPhase::Bloom,
            Some(GameReadiness::Starting) => BloomPhase::Budding,
        }
    }

    pub fn phase(&self) -> BloomPhase {
        self.phase
    }

    /// The game is on its way in: the one wait Home has, so the only time ash drifts here.
    pub fn is_game_starting(&self) -> bool {
        matches!(self.phase, BloomPhase::Budding | BloomPhase::Opening)
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn is_error(&self) -> bool {
        self.is_error
    }

    pub fn set_mod_loader(&mut self, state: ModLoaderState) {
        self.loader_known = true;
        self.has_mod_loader = matches!(state, ModLoaderState::Active | ModLoaderState::SetAside);
        let conflicted = state == ModLoaderState::Conflict;
        if conflicted != self.conflicted {
            self.conflicted = conflicted;
            // Every start stops on it, so Home says so where the starts report, and takes it back once it is resolved.
            self.is_error = conflicted;
            self.message = conflicted.then(|| Conflict.into());
        }
    }

    pub fn set_phase(&mut self, phase: BloomPhase) {
        // A start's message is about that game: once it has closed, what it said is no longer so.
        if phase == BloomPhase::Wilted && self.phase != BloomPhase::Wilted {
            self.is_error = false;
            self.message = None;
        }
        self.phase = phase;
    }

    /// Only with no game running (a second start would only exit by itself, so the buttons wait for the game to close)
    /// and MelonLoader's proxy in one known state.
    pub fn can_play(&self) -> bool {
        !self.starting && !self.elsewhere && self.phase == BloomPhase::Wilted && !self.conflicted
    }

    fn set_elsewhere(&mut self, starting: bool) {
        // Home's own start also keeps the launcher busy; that one is `starting`.
        self.elsewhere = starting && !self.starting;
    }

    pub fn can_play_with_mods(&self) -> bool {
        self.can_play() && (self.has_mod_loader || !self.loader_known)
    }

    pub fn online_tip(&self) -> Cow<'static, str> {
        self.tip(Online)
    }

    pub fn offline_tip(&self) -> Cow<'static, str> {
        self.tip(Offline)
    }

    pub fn with_mods_tip(&self) -> Cow<'static, str> {
        self.tip(WithMods)
    }

    /// Why a Play button waits, for the line under the buttons when nothing else is said there: a disabled button takes no
    /// keyboard focus, so its tip alone would leave a keyboard user guessing.
    pub fn wait_reason(&self) -> Option<&'static str> {
        if self.starting || self.conflicted {
            None
        } else if self.phase != BloomPhase::Wilted {
            Some(Running.get())
        } else if self.loader_known && !self.has_mod_loader {
            Some(NoModLoader.get())
        } else {
            None
        }
    }

    /// What each button does, or, while the game runs and they wait, why they wait.
    fn tip(&self, action: Phrase) -> Cow<'static, str> {
        if self.conflicted {
            Cow::Borrowed(Conflict.get())
        } else if self.phase == BloomPhase::Wilted {
            Cow::Borrowed(action.get())
        } else {
            Cow::Borrowed(Running.get())
        }
    }

    /// A start is under way: the old message goes, and a start with mods says why it takes a while.
    pub fn begin(&mut self, with_mods: bool) {
        self.starting = true;
        self.is_error = false;
        self.message = with_mods.then(|| StartingWithMods.into());
    }

    pub fn finish(&mut self, report: LaunchReport) {
        self.starting = false;
        self.is_error = !report.started;
        self.message = Some(report.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiderland::step;

    fn send(state: &mut State, intent: Intent) -> Vec<Effect> {
        step::<Home>(state, intent).effects
    }

    fn found(phase: BloomPhase) -> Intent {
        let readiness = match phase {
            BloomPhase::Wilted => None,
            BloomPhase::Budding => Some(GameReadiness::Starting),
            BloomPhase::Opening => Some(GameReadiness::NoCharacter),
            BloomPhase::Bloom => Some(GameReadiness::Offline),
        };
        Intent::Seen(GameSeen {
            running: readiness.map(|_| RunningGame {
                process_id: 1,
                start_time: jiff::Timestamp::UNIX_EPOCH,
            }),
            readiness,
            loader: Some(ModLoaderState::SetAside),
            starting: false,
            failed: false,
        })
    }

    fn starting(starting: bool) -> Intent {
        Intent::Seen(GameSeen { starting, ..GameSeen::default() })
    }

    fn report(started: bool) -> LaunchReport {
        LaunchReport { started, message: "said".into() }
    }

    #[test]
    fn showing_looks_and_the_first_result_places_the_tree_without_blooming() {
        let mut state = State::default();
        assert_eq!(spiderland::subscriptions::<Home>(&state), [Subscription::Hear]);
        assert_eq!(send(&mut state, Intent::Shown), [Effect::Refresh]);
        assert_eq!(spiderland::subscriptions::<Home>(&state), [Subscription::Hear, Subscription::Follow]);
        assert_eq!(
            send(&mut state, found(BloomPhase::Bloom)),
            [Effect::Tree {
                phase: BloomPhase::Bloom,
                ash: false,
                entering: true
            }]
        );
    }

    #[test]
    fn the_tree_moves_only_when_its_target_changes() {
        let mut state = State::default();
        send(&mut state, Intent::Shown);
        send(&mut state, found(BloomPhase::Wilted));
        assert!(send(&mut state, found(BloomPhase::Wilted)).is_empty(), "the same phase again moves nothing");
        assert_eq!(
            send(&mut state, found(BloomPhase::Budding)),
            [Effect::Tree {
                phase: BloomPhase::Budding,
                ash: true,
                entering: false
            }]
        );
    }

    #[test]
    fn while_another_page_shows_nothing_watches_or_moves_the_tree() {
        let mut state = State::default();
        send(&mut state, Intent::Shown);
        send(&mut state, Intent::Hidden);
        assert_eq!(spiderland::subscriptions::<Home>(&state), [Subscription::Hear], "it still hears, and stops following");
        assert!(send(&mut state, found(BloomPhase::Bloom)).is_empty(), "the result is kept, and the tree left alone");
        assert_eq!(state.model().phase(), BloomPhase::Bloom);
    }

    #[test]
    fn a_start_is_refused_unless_every_play_is_available() {
        let mut state = State::default();
        send(&mut state, Intent::Shown);
        send(&mut state, found(BloomPhase::Bloom));
        assert!(send(&mut state, Intent::Start(LaunchMode::Online)).is_empty());
        send(&mut state, found(BloomPhase::Wilted));
        assert!(send(&mut state, Intent::Start(LaunchMode::Online)).contains(&Effect::Launch(LaunchMode::Online)));
        assert!(send(&mut state, Intent::Start(LaunchMode::Online)).is_empty(), "a start under way holds the rest");
    }

    #[test]
    fn a_start_begun_elsewhere_holds_plays_until_a_look_finds_it_finished() {
        let mut state = State::default();
        send(&mut state, starting(true));
        assert!(!present(state.model()).can_start);
        assert!(send(&mut state, Intent::Start(LaunchMode::Offline)).is_empty());
        send(&mut state, starting(false));
        assert!(present(state.model()).can_start);
    }

    #[test]
    fn a_look_during_homes_own_start_does_not_outlast_it() {
        let mut state = State::default();
        send(&mut state, found(BloomPhase::Wilted));
        send(&mut state, Intent::Start(LaunchMode::Offline));
        send(&mut state, starting(true));
        send(&mut state, Intent::Started(Ok(report(false))));
        assert!(present(state.model()).can_start, "the launcher being busy with Home's own start is not another start");
    }

    #[test]
    fn a_failed_look_says_so_until_a_look_succeeds() {
        let mut state = State::default();
        send(&mut state, Intent::Shown);
        send(
            &mut state,
            Intent::Seen(GameSeen {
                failed: true,
                ..GameSeen::default()
            }),
        );
        let view = present(state.model());
        assert_eq!(view.message, LookFailed.get());
        assert!(view.message_is_error);
        send(&mut state, found(BloomPhase::Wilted));
        assert_eq!(present(state.model()).message, "");
    }

    #[test]
    fn a_start_that_panics_ends_the_start_and_looks_again() {
        let mut state = State::default();
        send(&mut state, found(BloomPhase::Wilted));
        send(&mut state, Intent::Start(LaunchMode::Offline));
        assert_eq!(send(&mut state, Intent::Started(Err(Panicked))), [Effect::Refresh]);
        let view = present(state.model());
        assert_eq!(view.message, StartFailed.get());
        assert!(view.message_is_error);
        assert!(view.can_start);
    }

    #[test]
    fn only_what_present_reads_moves_the_views_version() {
        let mut state = State::default();
        send(&mut state, found(BloomPhase::Wilted));
        let shown = state.model().version();
        send(&mut state, found(BloomPhase::Wilted));
        assert_eq!(state.model().version(), shown, "a look that finds the same leaves the view alone");
        send(&mut state, Intent::Hidden);
        assert_eq!(state.model().version(), shown, "page changes leave the view's inputs alone");
        send(&mut state, found(BloomPhase::Budding));
        assert_ne!(state.model().version(), shown);
    }

    #[test]
    fn before_the_first_look_home_claims_nothing_about_melonloader() {
        let view = present(&HomeModel::default());
        assert_eq!(view.message, "");
        assert!(view.can_start && view.can_start_with_mods);
    }

    #[test]
    fn the_tree_follows_the_running_game() {
        let game = RunningGame {
            process_id: 1,
            start_time: jiff::Timestamp::UNIX_EPOCH,
        };
        assert_eq!(HomeModel::phase_of(None, |_| GameReadiness::Offline), BloomPhase::Wilted);
        assert_eq!(HomeModel::phase_of(Some(&game), |_| GameReadiness::Starting), BloomPhase::Budding);
        assert_eq!(HomeModel::phase_of(Some(&game), |_| GameReadiness::NoCharacter), BloomPhase::Opening);
        assert_eq!(HomeModel::phase_of(Some(&game), |_| GameReadiness::Online), BloomPhase::Bloom);
    }

    #[test]
    fn the_buttons_wait_while_the_game_runs_and_say_why() {
        let mut home = HomeModel::default();
        home.set_mod_loader(ModLoaderState::SetAside);
        assert!(home.can_play_with_mods());
        home.set_phase(BloomPhase::Budding);
        assert!(!home.can_play());
        assert_eq!(home.offline_tip(), Running.get());
        home.set_phase(BloomPhase::Wilted);
        assert_eq!(home.online_tip(), Online.get());
        assert_eq!(home.wait_reason(), None);
        home.set_mod_loader(ModLoaderState::NotInstalled);
        assert!(home.can_play() && !home.can_play_with_mods());
        assert_eq!(home.wait_reason(), Some(NoModLoader.get()));
        home.set_phase(BloomPhase::Budding);
        assert_eq!(home.wait_reason(), Some(Running.get()));
    }

    #[test]
    fn a_conflict_stops_every_start_until_it_is_resolved() {
        let mut home = HomeModel::default();
        home.set_mod_loader(ModLoaderState::Conflict);
        assert!(!home.can_play());
        assert!(home.is_error());
        assert_eq!(home.message(), Some(Conflict.get()));
        assert_eq!(home.with_mods_tip(), Conflict.get());
        home.set_mod_loader(ModLoaderState::SetAside);
        assert_eq!(home.message(), None);
        assert!(home.can_play());
    }

    #[test]
    fn a_starts_message_lasts_until_that_game_closes() {
        let mut home = HomeModel::default();
        home.begin(true);
        assert!(!home.can_play());
        assert_eq!(home.message(), Some(StartingWithMods.get()));
        home.finish(LaunchReport {
            started: true,
            message: "Started offline, without mods.".into(),
        });
        assert!(!home.is_error());
        home.set_phase(BloomPhase::Bloom);
        assert!(home.message().is_some());
        home.set_phase(BloomPhase::Wilted);
        assert_eq!(home.message(), None);
        home.finish(LaunchReport {
            started: false,
            message: "Last Epoch is already starting.".into(),
        });
        assert!(home.is_error());
    }
}
