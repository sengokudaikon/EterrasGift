//! Cheat profiles: for each offline character the user chose to keep one for, the hooks it wants on and their values,
//! kept across runs in `moduleSettings.trainer.profiles`. Pure: the worker owns the book (`backend`), and the
//! composition saves it through a [`ProfileSaver`].
//!
//! `settings.json`'s `hookValues` holds one set of values for every character, with every switch off each run. Here
//! those values are what a character without a profile starts from in play; the key is left as it is, since other
//! readers of the settings still use it.

use crate::features::{FeatureIntents, HookId, SavedValues};
use eg_core::game_log::LoggedCharacter;
use jiff::Timestamp;
use serde_json::{Map, Value};
use std::{fmt, sync::Arc};

/// The settings key the profiles are saved under, in `moduleSettings.trainer`.
pub const PROFILES_KEY: &str = "profiles";
/// `settings.json`'s `hookValues`: values for every character, read as the template for a new profile.
pub const HOOK_VALUES_KEY: &str = "hookValues";

/// A character as Player.log names it when it loads, case-sensitive as logged. The level is left out: it changes. Two
/// characters with the same name, class and cycle share a profile.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProfileKey {
    pub name: String,
    pub class: String,
    pub cycle: String,
}

impl ProfileKey {
    pub fn of(character: &LoggedCharacter) -> Self {
        Self {
            name: character.name.clone(),
            class: character.class.clone(),
            cycle: character.cycle.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub key: ProfileKey,
    /// When the character last entered play with the trainer connected.
    pub last_played: Option<Timestamp>,
    pub hooks: FeatureIntents,
}

/// Every kept profile, the last played first, and the template a character without one starts from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileBook {
    profiles: Vec<Profile>,
    template: FeatureIntents,
}

impl ProfileBook {
    /// Reads the trainer's settings slice. Whatever the registry no longer has is dropped: an entry without its name,
    /// class or cycle, an unknown hook, values of the wrong count (out-of-range ones are fitted to the registry). A
    /// second entry for the same character is dropped too, keeping the one played last.
    pub fn from_settings(slice: Option<&Value>) -> Self {
        let template = FeatureIntents::from_saved(field(slice, HOOK_VALUES_KEY).map(saved_values).unwrap_or_default()).unwrap_or_default();
        let mut profiles: Vec<Profile> = field(slice, PROFILES_KEY)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| profile(entry, &template))
            .collect();
        profiles.sort_by_key(|profile| std::cmp::Reverse(profile.last_played));
        let mut seen = std::collections::BTreeSet::new();
        profiles.retain(|profile| seen.insert(profile.key.clone()));
        Self { profiles, template }
    }

    /// Writes the profiles into the trainer's settings slice, replacing what was saved under any spelling of the key.
    /// The template's `hookValues` stay as they are.
    pub fn write_settings(&self, fields: &mut Map<String, Value>) {
        let aliases: Vec<_> = fields.keys().filter(|name| name.eq_ignore_ascii_case(PROFILES_KEY)).cloned().collect();
        for alias in aliases {
            fields.remove(&alias);
        }
        fields.insert(PROFILES_KEY.into(), Value::Array(self.profiles.iter().map(to_json).collect()));
    }

    /// The last played first.
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    pub fn get(&self, key: &ProfileKey) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.key == *key)
    }

    pub fn hooks_mut(&mut self, key: &ProfileKey) -> Option<&mut FeatureIntents> {
        self.profiles.iter_mut().find(|profile| profile.key == *key).map(|profile| &mut profile.hooks)
    }

    /// What a character without a profile starts from: nothing wanted, with `settings.json`'s `hookValues`.
    pub fn template(&self) -> &FeatureIntents {
        &self.template
    }

    /// The character entered play: its profile becomes the last played. False when it has none; none is made.
    pub fn played(&mut self, key: &ProfileKey, at: Timestamp) -> bool {
        let Some(index) = self.profiles.iter().position(|profile| profile.key == *key) else {
            return false;
        };
        let mut profile = self.profiles.remove(index);
        profile.last_played = Some(at);
        self.profiles.insert(0, profile);
        true
    }

    /// Keeps `hooks` as the profile of a character in play, the last played. One it already had is replaced.
    pub fn create(&mut self, key: ProfileKey, hooks: FeatureIntents, at: Timestamp) {
        self.profiles.retain(|profile| profile.key != key);
        self.profiles.insert(
            0,
            Profile {
                key,
                last_played: Some(at),
                hooks,
            },
        );
    }

    /// The profile removed, if there was one.
    pub fn remove(&mut self, key: &ProfileKey) -> Option<Profile> {
        let index = self.profiles.iter().position(|profile| profile.key == *key)?;
        Some(self.profiles.remove(index))
    }
}

/// Saves the profiles when they change: the composition supplies it, through the settings service. It is called on
/// the trainer's worker and must not block.
#[derive(Clone)]
pub struct ProfileSaver(Arc<dyn Fn(&ProfileBook) + Send + Sync>);

impl ProfileSaver {
    pub fn new(save: impl Fn(&ProfileBook) + Send + Sync + 'static) -> Self {
        Self(Arc::new(save))
    }

    pub fn save(&self, book: &ProfileBook) {
        (self.0)(book);
    }
}

impl fmt::Debug for ProfileSaver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProfileSaver")
    }
}

/// A field of the slice, under any spelling of its key, as the settings' other readers take them.
fn field<'a>(slice: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    slice?.as_object()?.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value)
}

/// Feature IDs as the C# app and earlier versions saved them, and each one's ID now: every feature has one name, its
/// title's words (2026-10-29). Settings are read through this; the profiles are written with the current IDs.
const RENAMED: &[(&str, &str)] = &[
    ("xp-multiplier", "experience-multiplier"),
    ("instant-level", "level-up-per-gain"),
    ("max-skill-level", "skills-to-level-20"),
    ("free-skill-points", "unlimited-skill-points"),
    ("free-passive-points", "unlimited-passive-points"),
    ("free-weaver-points", "unlimited-weaver-points"),
    ("passive-chain", "any-mastery-passives"),
    ("skill-slots", "specialization-slots"),
    ("item-requirements", "equip-without-requirements"),
    ("set-bonus", "full-set-bonus"),
    ("offhand", "any-offhand"),
    ("primordial-limit", "several-primordials"),
    ("duplicate", "duplicate-on-drop"),
    ("damage-dealt", "enemy-damage"),
    ("cast-speed", "ability-speed"),
    ("projectiles", "extra-projectiles"),
    ("area", "fixed-area"),
    ("attack-rate", "fixed-attack-rate"),
    ("leech-rate", "fixed-leech-rate"),
    ("zero-mana-cost", "no-mana-cost"),
    ("infinite-mana", "refill-mana"),
    ("endless-buffs", "buffs-never-expire"),
    ("move-speed", "movement-speed"),
    ("auto-target", "auto-aim"),
    ("forge-level", "forge-above-level"),
    ("shards", "shards-to-99"),
    ("runes-glyphs", "runes-glyphs-to-99"),
    ("woven-enchanter", "unlimited-woven-enchanter"),
    ("boss-drops", "boss-loot-chance"),
    ("extra-rares", "extra-rare"),
    ("force-rarity", "new-item-rarity"),
    ("always-t7", "tier-7-affixes"),
    ("perfect-rolls", "max-random-rolls"),
    ("pickup-range", "pickup-anywhere"),
    ("echo-cycle", "cycle-echo-types"),
    ("echo-type", "one-echo-type"),
    ("instant-rank", "rank-up-per-gain"),
    ("prophecy-any-zone", "prophecy-any-distance"),
    ("prophecy-rewards", "prophecy-reward-multiplier"),
    ("hunter-cost", "free-hunter-vendor"),
    ("rune-prison", "rune-prison-complex"),
    ("eternity-cache", "endgame-cache"),
    ("monster-density", "pack-count"),
    ("waypoints", "any-waypoint"),
    ("zoom", "camera-distance"),
];

/// The current ID for one read from the settings.
fn current_id(id: &str) -> &str {
    RENAMED.iter().find(|(old, _)| *old == id).map_or(id, |(_, new)| new)
}

/// `{ "id": [numbers] }`; an entry that is not a list of numbers is dropped. An entry under an old ID takes the current
/// one, unless the settings also hold the current one.
fn saved_values(values: &Value) -> Vec<SavedValues> {
    let entries = values.as_object();
    let mut saved: Vec<_> = entries
        .into_iter()
        .flatten()
        .filter(|(id, _)| current_id(id) == id.as_str() || !entries.is_some_and(|entries| entries.contains_key(current_id(id))))
        .filter_map(|(id, numbers)| {
            let values = numbers.as_array()?.iter().map(Value::as_f64).collect::<Option<Vec<_>>>()?;
            Some(SavedValues {
                id: current_id(id).to_owned(),
                values,
            })
        })
        .collect();
    if !saved.iter().any(|s| s.id == "incoming-damage")
        && let Some(old) = saved.iter().find(|s| s.id == "damage-taken" && s.values.len() == 1 && s.values[0].is_finite())
    {
        saved.push(SavedValues {
            id: "incoming-damage".into(),
            values: vec![(100.0 / old.values[0].clamp(1.0, 100.0)).round()],
        });
    }
    saved.retain(|s| s.id != "damage-taken" && s.id != "god-mode");
    saved
}

fn profile(entry: &Value, template: &FeatureIntents) -> Option<Profile> {
    let text = |key| field(Some(entry), key).and_then(Value::as_str).map(str::to_owned);
    let key = ProfileKey {
        name: text("name")?,
        class: text("class")?,
        cycle: text("cycle")?,
    };
    let last_played = field(Some(entry), "lastPlayed").and_then(Value::as_str).and_then(|time| time.parse().ok());
    let mut hooks = template.clone();
    // A JSON object holds each ID once, so the values cannot be refused as duplicates.
    let _ = hooks.restore_values(field(Some(entry), "values").map(saved_values).unwrap_or_default());
    for id in field(Some(entry), "wanted").and_then(Value::as_array).into_iter().flatten() {
        if let Some(id) = id.as_str().map(current_id).and_then(HookId::parse) {
            hooks.set_wanted(id, true);
        }
    }
    let wanted: Vec<_> = field(Some(entry), "wanted")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let has_new = field(Some(entry), "values")
        .and_then(Value::as_object)
        .is_some_and(|values| values.contains_key("incoming-damage"))
        || wanted.contains(&"incoming-damage");
    if !has_new && (wanted.contains(&"god-mode") || wanted.contains(&"damage-taken")) {
        let id = HookId::parse("incoming-damage").unwrap();
        if wanted.contains(&"god-mode") {
            let _ = hooks.set_values(id, &[0.0]);
        }
        hooks.set_wanted(id, true);
    }
    if wanted.contains(&"one-hit-kill") {
        let id = HookId::parse("enemy-damage").unwrap();
        let _ = hooks.set_values(id, &[101.0]);
        hooks.set_wanted(id, true);
    }
    Some(Profile { key, last_played, hooks })
}

fn to_json(profile: &Profile) -> Value {
    let mut entry = Map::new();
    entry.insert("name".into(), profile.key.name.clone().into());
    entry.insert("class".into(), profile.key.class.clone().into());
    entry.insert("cycle".into(), profile.key.cycle.clone().into());
    if let Some(time) = profile.last_played {
        entry.insert("lastPlayed".into(), time.to_string().into());
    }
    entry.insert("wanted".into(), profile.hooks.wanted().map(|id| Value::from(id.as_str())).collect());
    entry.insert(
        "values".into(),
        Value::Object(
            profile
                .hooks
                .saved_values()
                .into_iter()
                .map(|saved| (saved.id, saved.values.into_iter().map(Value::from).collect()))
                .collect(),
        ),
    );
    Value::Object(entry)
}
