//! The trainer's contribution to Settings. Kept outside the shared page and persistence infrastructure so a
//! composition can omit it. A run that has the trainer links its backend (`link`): the switches reach the session the
//! moment they flip, and Game definitions follows the session's catalog.

use crate::settings::{LiveStatus, ModuleChoice, ModuleLink, ModuleSection};
use eg_core::settings::LegacyModuleSettings;
use eg_trainer::backend::{Activity, BackendHandle, BackendSnapshot, CatalogState, Command, GameFiles};
use std::rc::Rc;

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
        title: "Trainer",
        description: "Edits items in the running game for offline characters. It loads a helper into Last Epoch, which leaves when the offline character leaves play or you disconnect. Online characters are always refused.",
        name: "the trainer",
        default_on: development,
        problem: None,
        sections: vec![
            ModuleSection::Status {
                title: "Game definitions",
                description: "Affix and unique definitions read from the game's files and kept until the game updates. The trainer opens only for the game build they were read from.",
                text: NO_DATA,
                action: "Read again",
                tip: "Read the definitions from the game's files again",
                unavailable: "Available once Last Epoch is found.",
            },
            ModuleSection::Toggle {
                key: AUTO_CONNECT,
                title: "Trainer connection",
                label: "Connect automatically",
                description: "Connect the trainer once an offline character is loaded. Mods work independently of this connection.",
                note: "",
                default: true,
                on_text: "Automatic",
                off_text: "Manual",
            },
            ModuleSection::Toggle {
                key: OVERRIDE,
                title: "I know what I'm doing",
                label: "I know what I'm doing",
                description: "Writes edits the game would not make itself: Weaver, enchantment and corrupted affixes wherever the slot fits, incompatible affixes and bases, another idol size, Weaver's Will on any unique and Weaver's Touch on any idol. With this off, incompatible affixes can still be listed but not written.",
                note: "Use expendable items. The game may drop or rearrange what it does not support.",
                default: false,
                on_text: "On",
                off_text: "Off",
            },
        ],
    }
}

const NO_DATA: &str = "No game data: Last Epoch was not found";

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
        (_, GameFiles::Reading) => "Reading game files".into(),
        (_, GameFiles::Unavailable(reason)) => format!("The game files could not be read: {reason}"),
        (CatalogState::Current { data, .. } | CatalogState::Stale(data), _) => format!("Current game data · {} affixes", data.affixes.len()),
        (_, GameFiles::Ready { affixes, .. }) => format!("Game files · {affixes} affixes"),
        (_, GameFiles::NotConfigured) => NO_DATA.into(),
    };
    if let GameFiles::Ready { checked_at, updated_at, .. } = files {
        let time = |value: Option<jiff::Timestamp>| {
            value.map_or_else(
                || "Not recorded".into(),
                |time| time.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M:%S %Z").to_string(),
            )
        };
        text.push_str(&format!("\nLast checked: {}\nCache updated: {}", time(*checked_at), time(*updated_at)));
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
        assert_eq!(describe(&CatalogState::Missing, &GameFiles::NotConfigured), "No game data: Last Epoch was not found");
        assert_eq!(describe(&CatalogState::Missing, &GameFiles::Reading), "Reading game files");
        assert_eq!(
            describe(&CatalogState::Missing, &ready),
            "Game files · 1156 affixes\nLast checked: Not recorded\nCache updated: Not recorded"
        );
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
        assert!(shown.contains("Last checked: 2026-10-22"), "{shown}");
        assert!(shown.contains("Cache updated: 2026-10-21"), "{shown}");
    }
}
