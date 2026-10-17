//! Home's state (`spec/companion.md` §5): it starts the game online, offline, or offline with mods, with Eterra's tree
//! saying where the game is. Plain values; the UI reads them after each change.

use crate::launcher::{LaunchReport, STARTING_WITH_MODS};
use crate::loader::{CONFLICT, ModLoaderState};
use crate::process::RunningGame;
use eg_core::bloom::BloomPhase;
use eg_core::game_log::GameReadiness;

const ONLINE: &str = "Start the game through Steam. MelonLoader is set aside first, so no mod loads online";
const OFFLINE: &str = "Start the game in offline mode. MelonLoader is set aside first, so no mod loads";
const WITH_MODS: &str = "Start the game in offline mode with MelonLoader and the mods that are on; needs MelonLoader in the game folder";
const RUNNING: &str = "Last Epoch is running. Start it again once it has closed.";

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
            self.message = conflicted.then(conflict);
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

    pub fn online_tip(&self) -> String {
        self.tip(ONLINE)
    }

    pub fn offline_tip(&self) -> String {
        self.tip(OFFLINE)
    }

    pub fn with_mods_tip(&self) -> String {
        self.tip(WITH_MODS)
    }

    /// What each button does, or, while the game runs and they wait, why they wait.
    fn tip(&self, action: &str) -> String {
        if self.conflicted {
            conflict()
        } else if self.phase == BloomPhase::Wilted {
            action.to_owned()
        } else {
            RUNNING.to_owned()
        }
    }

    /// A start is under way: the old message goes, and a start with mods says why it takes a while.
    pub fn begin(&mut self, with_mods: bool) {
        self.starting = true;
        self.is_error = false;
        self.message = with_mods.then(|| STARTING_WITH_MODS.to_owned());
    }

    pub fn finish(&mut self, report: LaunchReport) {
        self.starting = false;
        self.is_error = !report.started;
        self.message = Some(report.message);
    }
}

fn conflict() -> String {
    format!("{CONFLICT} Delete the older one, then start the game.")
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
        assert_eq!(home.offline_tip(), RUNNING);
        home.set_phase(BloomPhase::Wilted);
        assert_eq!(home.online_tip(), ONLINE);
        home.set_mod_loader(ModLoaderState::NotInstalled);
        assert!(home.can_play() && !home.can_play_with_mods());
    }

    #[test]
    fn a_conflict_stops_every_start_until_it_is_resolved() {
        let mut home = HomeModel::default();
        home.set_mod_loader(ModLoaderState::Conflict);
        assert!(!home.can_play());
        assert!(home.is_error());
        assert_eq!(home.message(), Some(conflict().as_str()));
        assert_eq!(home.with_mods_tip(), conflict());
        home.set_mod_loader(ModLoaderState::SetAside);
        assert_eq!(home.message(), None);
        assert!(home.can_play());
    }

    #[test]
    fn a_starts_message_lasts_until_that_game_closes() {
        let mut home = HomeModel::default();
        home.begin(true);
        assert!(!home.can_play());
        assert_eq!(home.message(), Some(STARTING_WITH_MODS));
        home.finish(LaunchReport {
            started: true,
            message: "Started Last Epoch offline, without mods.".into(),
        });
        assert!(!home.is_error());
        home.set_phase(BloomPhase::Bloom);
        assert!(home.message().is_some());
        home.set_phase(BloomPhase::Wilted);
        assert_eq!(home.message(), None);
        home.finish(LaunchReport {
            started: false,
            message: "Last Epoch is starting already.".into(),
        });
        assert!(home.is_error());
    }
}
