use eg_core::game_log::LoggedCharacter;
use eg_trainer::features::{FeatureKey, HookId};
use eg_trainer::profiles::{ProfileBook, ProfileKey};
use jiff::Timestamp;
use serde_json::{Map, Value, json};

fn hook(id: &str) -> HookId {
    HookId::parse(id).unwrap()
}
fn key(name: &str) -> ProfileKey {
    ProfileKey {
        name: name.into(),
        class: "Mage".into(),
        cycle: "Swordfish".into(),
    }
}
fn at(time: &str) -> Timestamp {
    time.parse().unwrap()
}
fn default_of(id: &str) -> f64 {
    FeatureKey::Hook(hook(id)).descriptor().values[0].start
}

#[test]
fn a_character_is_known_by_name_class_and_cycle_not_level() {
    let character = |level| LoggedCharacter {
        name: "Mira".into(),
        level,
        online: false,
        cycle: "Swordfish".into(),
        class: "Mage".into(),
    };
    assert_eq!(ProfileKey::of(&character(Some(3))), ProfileKey::of(&character(Some(90))));
    assert_eq!(ProfileKey::of(&character(None)), key("Mira"));
}

#[test]
fn saved_profiles_are_read_fitted_to_the_catalog_and_ordered_by_last_played() {
    let book = ProfileBook::from_settings(Some(&json!({
        "hookValues": { "damage-dealt": [4.0], "zoom": [-150.0], "gone": [1.0], "xp-multiplier": [2.0] },
        "Profiles": [
            { "name": "Oren", "class": "Mage", "cycle": "Swordfish", "lastPlayed": "2026-10-17T09:00:00Z", "wanted": ["god-mode"] },
            {
                "name": "Mira", "class": "Mage", "cycle": "Swordfish", "lastPlayed": "2026-10-18T09:00:00Z",
                "wanted": ["zoom", "gone", "forge-item", 7], "values": { "zoom": [-40.0], "gone": [1.0], "drop-rates": [1.0] }
            },
            { "name": "Mira", "class": "Mage", "cycle": "Swordfish", "lastPlayed": "2026-10-01T09:00:00Z", "wanted": ["god-mode"] },
            { "name": "Never played", "class": "Mage", "cycle": "Swordfish" },
            { "class": "Mage", "cycle": "Swordfish", "wanted": ["god-mode"] },
            "not a profile"
        ]
    })));
    let names: Vec<_> = book.profiles().iter().map(|profile| profile.key.name.as_str()).collect();
    assert_eq!(names, ["Mira", "Oren", "Never played"], "the same character twice keeps the one played last");
    let mira = book.get(&key("Mira")).unwrap();
    assert_eq!(mira.last_played, Some(at("2026-10-18T09:00:00Z")));
    assert_eq!(mira.hooks.wanted().collect::<Vec<_>>(), [hook("zoom")], "only hooks can be wanted");
    assert_eq!(mira.hooks.values(hook("zoom")), [-40.]);
    assert_eq!(mira.hooks.values(hook("damage-dealt")), [4.], "what the profile lacks comes from hookValues");
    assert_eq!(
        mira.hooks.values(hook("drop-rates")),
        FeatureKey::Hook(hook("drop-rates")).descriptor().values.iter().map(|value| value.start).collect::<Vec<_>>(),
        "values of the wrong count are dropped"
    );
    let oren = book.get(&key("Oren")).unwrap();
    assert_eq!(oren.hooks.values(hook("zoom")), [-100.], "saved values are fitted");
    assert_eq!(
        oren.hooks.values(hook("xp-multiplier")),
        FeatureKey::Hook(hook("xp-multiplier"))
            .descriptor()
            .values
            .iter()
            .map(|value| value.start)
            .collect::<Vec<_>>()
    );
    assert!(oren.hooks.is_wanted(hook("god-mode")));
    assert_eq!(book.get(&key("Never played")).unwrap().last_played, None);
}

#[test]
fn nothing_saved_reads_as_no_profiles_with_the_catalogs_values() {
    for slice in [None, Some(json!(null)), Some(json!({ "profiles": "?", "hookValues": [1] })), Some(json!({}))] {
        let mut book = ProfileBook::from_settings(slice.as_ref());
        assert!(book.profiles().is_empty());
        book.played(&key("Mira"), at("2026-10-19T10:00:00Z"));
        assert_eq!(book.get(&key("Mira")).unwrap().hooks.values(hook("damage-dealt")), [default_of("damage-dealt")]);
    }
}

#[test]
fn a_new_character_starts_from_the_c_sharp_apps_values_wanting_nothing() {
    let mut book = ProfileBook::from_settings(Some(&json!({ "hookValues": { "damage-dealt": [5.0] } })));
    book.played(&key("Mira"), at("2026-10-19T10:00:00Z"));
    book.played(&key("Oren"), at("2026-10-19T11:00:00Z"));
    book.hooks_mut(&key("Mira")).unwrap().set_values(hook("damage-dealt"), &[7.]).unwrap();
    book.played(&key("Mira"), at("2026-10-19T12:00:00Z"));
    let names: Vec<_> = book.profiles().iter().map(|profile| profile.key.name.as_str()).collect();
    assert_eq!(names, ["Mira", "Oren"]);
    let mira = book.get(&key("Mira")).unwrap();
    assert_eq!((mira.last_played, mira.hooks.values(hook("damage-dealt"))), (Some(at("2026-10-19T12:00:00Z")), &[7.][..]));
    let oren = book.get(&key("Oren")).unwrap();
    assert_eq!((oren.hooks.wanted().count(), oren.hooks.values(hook("damage-dealt"))), (0, &[5.][..]));
    assert!(book.remove(&key("Oren")));
    assert!(!book.remove(&key("Oren")));
}

#[test]
fn profiles_are_written_beside_the_c_sharp_apps_values_and_read_back_the_same() {
    let mut book = ProfileBook::from_settings(Some(&json!({ "hookValues": { "damage-dealt": [5.0] } })));
    book.played(&key("Mira"), at("2026-10-19T10:00:00Z"));
    let hooks = book.hooks_mut(&key("Mira")).unwrap();
    hooks.set_wanted(hook("zoom"), true);
    hooks.set_values(hook("zoom"), &[-25.5]).unwrap();
    let mut fields: Map<String, Value> = serde_json::from_value(json!({
        "autoConnect": false,
        "hookValues": { "damage-dealt": [5.0] },
        "PROFILES": [{ "name": "stale" }]
    }))
    .unwrap();
    book.write_settings(&mut fields);
    assert_eq!(fields["autoConnect"], json!(false));
    assert_eq!(fields["hookValues"], json!({ "damage-dealt": [5.0] }), "the C# app's values stay as they are");
    assert!(!fields.contains_key("PROFILES"), "one spelling of the key");
    let saved = &fields["profiles"][0];
    assert_eq!(
        (&saved["name"], &saved["lastPlayed"], &saved["wanted"], &saved["values"]["zoom"]),
        (&json!("Mira"), &json!("2026-10-19T10:00:00Z"), &json!(["zoom"]), &json!([-25.5]))
    );
    assert_eq!(ProfileBook::from_settings(Some(&Value::Object(fields))), book);
}
