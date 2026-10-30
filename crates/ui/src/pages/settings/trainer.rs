//! The trainer's contribution to Settings. Kept outside the shared page and persistence infrastructure so a
//! composition can omit it. A run that has the trainer links its backend (`link`): the switches reach the session the
//! moment they flip, and Game definitions follows the session's catalog.

use crate::pages::settings::{LiveStatus, ModuleChoice, ModuleLink, ModuleSection};
use eg_core::phrase::{Phrase, Plural};
use eg_core::settings::LegacyModuleSettings;
use eg_trainer::backend::{Activity, BackendHandle, BackendSnapshot, CatalogState, Command, GameFiles};
use std::rc::Rc;

const TRAINER: Phrase = Phrase::new("Trainer");
const TRAINER_DESCRIPTION: Phrase =
    Phrase::new("Edit offline characters and their items in the running game. A helper loads into Last Epoch and leaves when you disconnect or the character leaves play.");
const THE_TRAINER: Phrase = Phrase::new("the trainer");
const GAME_DEFINITIONS: Phrase = Phrase::new("Game definitions");
const GAME_DEFINITIONS_DESCRIPTION: Phrase = Phrase::new("Affixes and uniques read from the game's files, read again when the game updates.");
const READ_AGAIN: Phrase = Phrase::new("Read again");
const READ_AGAIN_TIP: Phrase = Phrase::new("Read the game's files again");
const NOT_FOUND: Phrase = Phrase::new("Last Epoch wasn't found.");
const CONNECTION: Phrase = Phrase::new("Trainer connection");
const CONNECT_AUTOMATICALLY: Phrase = Phrase::new("Connect automatically");
const CONNECTION_DESCRIPTION: Phrase = Phrase::new("Connect when an offline character is loaded.");
const AUTOMATIC: Phrase = Phrase::new("Automatic");
const MANUAL: Phrase = Phrase::new("Manual");
const OVERRIDE_TITLE: Phrase = Phrase::new("I know what I'm doing");
const OVERRIDE_DESCRIPTION: Phrase = Phrase::new(
    "Allows edits the game wouldn't make: incompatible affixes and bases, Weaver, enchantment and corrupted affixes in any fitting slot, other idol sizes, Weaver's Will on any unique and Weaver's Touch on any idol.",
);
const OVERRIDE_NOTE: Phrase = Phrase::new("Use expendable items. The game may drop or rearrange what it doesn't support.");
const ON: Phrase = Phrase::new("On");
const OFF: Phrase = Phrase::new("Off");
const NO_DATA: Phrase = Phrase::new("Last Epoch wasn't found");
const READING: Phrase = Phrase::new("Reading the game files…");
const UNREADABLE: Phrase = Phrase::new("The game files could not be read: {reason}");
const AFFIXES: Plural = Plural::new("{n} affix", "{n} affixes");
const NOT_RECORDED: Phrase = Phrase::new("not recorded");
const CHECKED: Phrase = Phrase::new("Checked: {time}");
const UPDATED: Phrase = Phrase::new("Updated: {time}");

pub const ID: &str = "trainer";
pub const AUTO_CONNECT: &str = "autoConnect";
pub const OVERRIDE: &str = "iKnowWhatImDoing";
pub const LEGACY: LegacyModuleSettings = LegacyModuleSettings {
    module_id: ID,
    enabled_key: Some("trainerEnabled"),
    keys: &[AUTO_CONNECT, OVERRIDE, "hookValues"],
};

pub fn descriptor(development: bool) -> ModuleChoice {
    ModuleChoice {
        id: ID,
        title: TRAINER,
        description: TRAINER_DESCRIPTION,
        name: THE_TRAINER,
        default_on: development,
        problem: None,
        sections: vec![
            ModuleSection::Status {
                title: GAME_DEFINITIONS,
                description: GAME_DEFINITIONS_DESCRIPTION,
                text: NO_DATA,
                action: READ_AGAIN,
                tip: READ_AGAIN_TIP,
                unavailable: NOT_FOUND,
            },
            ModuleSection::Toggle {
                key: AUTO_CONNECT,
                title: CONNECTION,
                label: CONNECT_AUTOMATICALLY,
                description: CONNECTION_DESCRIPTION,
                note: None,
                default: true,
                on_text: AUTOMATIC,
                off_text: MANUAL,
            },
            ModuleSection::Toggle {
                key: OVERRIDE,
                title: OVERRIDE_TITLE,
                label: OVERRIDE_TITLE,
                description: OVERRIDE_DESCRIPTION,
                note: Some(OVERRIDE_NOTE),
                default: false,
                on_text: ON,
                off_text: OFF,
            },
        ],
    }
}

/// The running trainer, as its Settings groups reach it.
pub fn link(backend: BackendHandle) -> Rc<dyn ModuleLink> {
    Rc::new(Link(backend))
}

struct Link(BackendHandle);

impl ModuleLink for Link {
    /// The session takes the choice at once, whatever its save does.
    fn switched(&self, key: &str, on: bool) {
        let command = match key {
            AUTO_CONNECT => Command::SetAutoConnect(on),
            OVERRIDE => Command::SetOverride(on),
            _ => return,
        };
        let _ = self.0.try_submit(command);
    }

    fn status(&self, _section: usize) -> Option<LiveStatus> {
        let running = self.0.is_running();
        Some(self.0.peek(|snapshot| game_data(snapshot, running)))
    }

    fn act(&self, section: usize) {
        if self.status(section).is_some_and(|status| status.can_act && !status.busy) {
            let _ = self.0.try_submit(Command::RefreshCatalog);
        }
    }
}

/// Game definitions: the definitions in use, else what the game data service holds. Game definitions come from the
/// game's files, so Read again needs only the game's folder, not a running game.
pub fn game_data(snapshot: &BackendSnapshot, running: bool) -> LiveStatus {
    LiveStatus {
        text: describe(&snapshot.catalog, &snapshot.game_files),
        can_act: running && snapshot.game_files != GameFiles::NotConfigured,
        busy: snapshot.activity == Some(Activity::ReadingGameFiles) || snapshot.game_files == GameFiles::Reading,
    }
}

fn describe(catalog: &CatalogState, files: &GameFiles) -> String {
    let mut text = match (catalog, files) {
        (_, GameFiles::Reading) => READING.into(),
        (_, GameFiles::Unavailable(reason)) => UNREADABLE.with(&[("reason", reason)]),
        (CatalogState::Current { data, .. } | CatalogState::Stale(data), _) => AFFIXES.count(data.affixes.len()),
        (_, GameFiles::Ready { affixes, .. }) => AFFIXES.count(*affixes),
        (_, GameFiles::NotConfigured) => NO_DATA.into(),
    };
    if let GameFiles::Ready { checked_at, updated_at, .. } = files {
        let time = |value: Option<jiff::Timestamp>| {
            value.map_or_else(
                || NOT_RECORDED.into(),
                |time| time.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M:%S %Z").to_string(),
            )
        };
        for line in [CHECKED.with(&[("time", &time(*checked_at))]), UPDATED.with(&[("time", &time(*updated_at))])] {
            text.push('\n');
            text.push_str(&line);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_definitions_say_what_the_game_files_hold() {
        let ready = GameFiles::Ready {
            affixes: 1156,
            build: "8dfd6929".into(),
            checked_at: None,
            updated_at: None,
        };
        assert_eq!(describe(&CatalogState::Missing, &GameFiles::NotConfigured), "Last Epoch wasn't found");
        assert_eq!(describe(&CatalogState::Missing, &GameFiles::Reading), "Reading the game files…");
        assert_eq!(describe(&CatalogState::Missing, &ready), "1156 affixes\nChecked: not recorded\nUpdated: not recorded");
        assert_eq!(
            describe(&CatalogState::Missing, &GameFiles::Unavailable("no Last Epoch_Data".into())),
            "The game files could not be read: no Last Epoch_Data",
            "the trainer cannot open without them, so the reason is shown"
        );
    }

    #[test]
    fn checked_and_updated_times_are_distinct_and_hidden_on_failed_reads() {
        let checked: jiff::Timestamp = "2026-10-22T11:40:00Z".parse().unwrap();
        let updated: jiff::Timestamp = "2026-10-21T17:15:00Z".parse().unwrap();
        let files = GameFiles::Ready {
            affixes: 42,
            build: "test".into(),
            checked_at: Some(checked),
            updated_at: Some(updated),
        };
        let shown = describe(&CatalogState::Missing, &files);
        assert!(shown.contains("Checked: 2026-10-22"), "{shown}");
        assert!(shown.contains("Updated: 2026-10-21"), "{shown}");
    }
}
