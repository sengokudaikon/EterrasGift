//! Home's state: it starts the game online, offline, or offline with mods, with Eterra's tree saying where the game is.
//! Plain values; the UI reads them after each change.

use crate::game::process::RunningGame;
use crate::launch::launcher::{LaunchReport, StartingWithMods};
use crate::launch::loader::ModLoaderState;
use eg_core::bloom::BloomPhase;
use eg_core::game_log::GameReadiness;
use eg_i18n::{Phrase, phrases};
use std::borrow::Cow;

phrases! {
    Online: "Start through Steam, without mods"
    Offline: "Start offline, without mods"
    WithMods: "Start offline with MelonLoader and the mods that are on"
    Running: "Last Epoch is already running."
    NoModLoader: "MelonLoader isn't in the game folder."
    Conflict: "The game folder has both version.dll and version.dll.disabled. Delete the older one, then start the game."
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeModel {
    has_mod_loader: bool,
    /// Both MelonLoader proxies are in the game folder: no start can tell which is current, so none is offered.
    conflicted: bool,
    phase: BloomPhase,
    starting: bool,
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
        !self.starting && self.phase == BloomPhase::Wilted && !self.conflicted
    }

    pub fn can_play_with_mods(&self) -> bool {
        self.can_play() && self.has_mod_loader
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
        } else if !self.has_mod_loader {
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
