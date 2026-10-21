//! The trainer's contribution to Settings. Kept outside the shared page and persistence infrastructure so a
//! composition can omit it. A run that has the trainer links its backend (`link`): the switches reach the session the
//! moment they flip, and Game definitions follows the session's catalog.

use crate::settings::{LiveStatus, ModuleChoice, ModuleLink, ModuleSection};
use eg_core::settings::LegacyModuleSettings;
use eg_trainer::backend::{Activity, BackendHandle, BackendSnapshot, CaptureProgress, CatalogState, Command, GameFiles, Phase};
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
        description: "Edits items in the running game for offline characters. It loads a helper into Last Epoch, which stays there until the game restarts. Online characters are always refused.",
        name: "the trainer",
        default_on: development,
        problem: None,
        sections: vec![
            ModuleSection::Toggle {
                key: AUTO_CONNECT,
                title: "Connection",
                label: "Connect automatically",
                description: "Find Last Epoch and connect once an offline character is loaded. Online characters are always refused.",
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
            ModuleSection::Status {
                title: "Game definitions",
                description: "Affix and unique definitions read from the game's files, or captured from the game on first connection when the files cannot be read; keyed by game build and language.",
                text: NO_CACHE,
                action: "Refresh from game",
                tip: "Capture the definitions again from the connected game",
                unavailable: "Available once an offline character is connected.",
            },
        ],
    }
}

const NO_CACHE: &str = "No game-data cache loaded";

/// The running trainer, as its Settings groups reach it. `cache` is the game-data cache's folder: a saved capture's
/// time is when its file was written.
pub fn link(backend: BackendHandle, cache: Option<std::path::PathBuf>) -> Rc<dyn ModuleLink> {
    Rc::new(Link(backend, cache.map(eg_trainer::catalog_cache::CatalogCache::new)))
}

struct Link(BackendHandle, Option<eg_trainer::catalog_cache::CatalogCache>);

impl ModuleLink for Link {
    /// As the C# `TrainerSettingsViewModel`: the session takes the choice at once, whatever its save does.
    fn switched(&self, key: &str, on: bool) {
        let command = match key {
            AUTO_CONNECT => Command::SetAutoConnect(on),
            OVERRIDE => Command::SetOverride(on),
            _ => return,
        };
        let _ = self.0.try_submit(command);
    }

    fn status(&self, _section: usize) -> Option<LiveStatus> {
        let snapshot = self.0.snapshot();
        let mut status = game_data(&snapshot, self.0.subscribe().has_changed().is_ok());
        if let (CatalogState::Saved(data), Some(cache), None | Some(CaptureProgress::Complete { .. })) = (&snapshot.catalog, &self.1, snapshot.capture_progress)
            && let Some(captured) = captured_at(cache, &data.identity)
        {
            status.text = format!("{} · {}", status.text, captured.strftime("%-d %B %Y %H:%M"));
        }
        Some(status)
    }

    fn act(&self, section: usize) {
        if self.status(section).is_some_and(|status| status.can_act && !status.busy) {
            let _ = self.0.try_submit(Command::RefreshCatalog);
        }
    }
}

/// Game definitions as the C# `TrainerConnectionViewModel.GameDataDescription` and `CatalogStore.CacheDescription`
/// word them: what loading them is doing while a capture runs, else the definitions in use. The saved-data line's
/// capture time comes from the cache file (`Link::status`), as the Rust catalog identity does not keep it.
pub fn game_data(snapshot: &BackendSnapshot, running: bool) -> LiveStatus {
    let capturing =
        snapshot.activity == Some(Activity::CapturingCatalog) || snapshot.capture_progress.is_some_and(|progress| !matches!(progress, CaptureProgress::Complete { .. }));
    // The C# command runs while the session can write (connected to an offline character); a capture under way only
    // greys the button, as `IsRefreshingGameData` does.
    LiveStatus {
        text: describe(snapshot.capture_progress, &snapshot.catalog, &snapshot.game_files),
        can_act: running && snapshot.phase == Phase::Ready,
        busy: capturing,
    }
}

/// When the saved capture was taken: its file is written once, whole, by each capture (`CatalogCache::save`).
fn captured_at(cache: &eg_trainer::catalog_cache::CatalogCache, identity: &eg_trainer::catalog::CatalogIdentity) -> Option<jiff::Zoned> {
    let modified = std::fs::metadata(cache.path_for(identity).ok()?).ok()?.modified().ok()?;
    let timestamp = jiff::Timestamp::try_from(modified).ok()?;
    Some(timestamp.to_zoned(jiff::tz::TimeZone::system()))
}

fn describe(progress: Option<CaptureProgress>, catalog: &CatalogState, files: &GameFiles) -> String {
    match (progress, catalog, files) {
        // As C#'s `GameDataDescription`: the count while pages load, else what the cache holds.
        (Some(CaptureProgress::Page { cursor, total, .. }), _, _) => format!("Loading game definitions: {cursor}/{total}"),
        (_, CatalogState::Current { data, .. } | CatalogState::Stale(data), _) => format!("Current game data · {} affixes", data.affixes.len()),
        // Before a session, the definitions read from the game's files (not in the C# app, which only captured).
        (_, _, GameFiles::Ready { affixes, .. }) => format!("Game files · {affixes} affixes"),
        (_, _, GameFiles::Reading) => "Reading game files".into(),
        (_, CatalogState::Missing, _) => NO_CACHE.into(),
        (_, CatalogState::Saved(data), _) => {
            let hash: String = data.identity.binary_hash.chars().take(12).collect();
            format!("Saved game data · {hash} · {}", data.identity.locale.language)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_trainer::catalog::CatalogKind;

    #[test]
    fn game_definitions_say_what_the_capture_is_doing_else_what_is_loaded() {
        let none = GameFiles::NotConfigured;
        assert_eq!(describe(None, &CatalogState::Missing, &none), "No game-data cache loaded");
        let page = CaptureProgress::Page {
            kind: CatalogKind::Affixes,
            cursor: 120,
            total: 960,
            count: 40,
            pages: 3,
            bytes: 4096,
        };
        assert_eq!(describe(Some(page), &CatalogState::Missing, &none), "Loading game definitions: 120/960");
        assert_eq!(describe(Some(CaptureProgress::LocaleRead), &CatalogState::Missing, &none), "No game-data cache loaded");
        assert_eq!(
            describe(Some(CaptureProgress::Complete { pages: 3, bytes: 4096 }), &CatalogState::Missing, &none),
            "No game-data cache loaded"
        );
    }

    #[test]
    fn before_a_session_the_game_files_say_what_they_hold() {
        let ready = GameFiles::Ready {
            affixes: 1156,
            build: "8dfd6929".into(),
        };
        assert_eq!(describe(None, &CatalogState::Missing, &GameFiles::Reading), "Reading game files");
        assert_eq!(describe(None, &CatalogState::Missing, &ready), "Game files · 1156 affixes");
        let unreadable = GameFiles::Unavailable("no Last Epoch_Data".into());
        assert_eq!(
            describe(None, &CatalogState::Missing, &unreadable),
            "No game-data cache loaded",
            "a failed read leaves the old wording"
        );
    }

    #[test]
    fn the_connection_switch_is_named_for_what_it_does() {
        let sections = descriptor(true).sections;
        assert!(matches!(
            sections[0],
            ModuleSection::Toggle {
                label: "Connect automatically",
                ..
            }
        ));
    }
}
