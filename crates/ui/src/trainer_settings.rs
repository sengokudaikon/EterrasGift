//! The trainer's contribution to Settings. Kept outside the shared page and persistence infrastructure so a
//! composition can omit it. Session and catalog actions will be supplied when the trainer is integrated.

use crate::settings::{ModuleChoice, ModuleSection};
use eg_core::settings::LegacyModuleSettings;

pub const ID: &str = "trainer";
pub const LEGACY: LegacyModuleSettings = LegacyModuleSettings {
    module_id: ID,
    enabled_key: Some("trainerEnabled"),
    keys: &["autoConnect", "iKnowWhatImDoing", "hookValues"],
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
                key: "autoConnect",
                title: "Connection",
                description: "Find Last Epoch and connect once an offline character is loaded. Online characters are always refused.",
                note: "",
                default: true,
                on_text: "Automatic",
                off_text: "Manual",
            },
            ModuleSection::Toggle {
                key: "iKnowWhatImDoing",
                title: "I know what I'm doing",
                description: "Writes edits the game would not make itself: Weaver, enchantment and corrupted affixes wherever the slot fits, incompatible affixes and bases, another idol size, Weaver's Will on any unique and Weaver's Touch on any idol. With this off, incompatible affixes can still be listed but not written.",
                note: "Use expendable items. The game may drop or rearrange what it does not support.",
                default: false,
                on_text: "On",
                off_text: "Off",
            },
            ModuleSection::Status {
                title: "Game definitions",
                description: "Affix and unique definitions captured from the game on first connection, keyed by game build and language.",
                text: "No game-data cache loaded",
                action: "Refresh from game",
                unavailable: "Available once an offline character is connected.",
            },
        ],
    }
}
