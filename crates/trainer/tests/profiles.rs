use eg_core::game_log::LoggedCharacter;
use eg_trainer::features::profiles::{ProfileBook, ProfileKey};
use eg_trainer::features::{FeatureKey, HookId};
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
fn legacy_enabled_rifts_become_portal_and_boss_selection_roundtrips() {
    let mut book = ProfileBook::from_settings(Some(&json!({"profiles": [
        {"name": "Mira", "class": "Mage", "cycle": "Swordfish", "wanted": ["mysterious-rift"]}
    ]})));
    let id = hook("mysterious-rift");
    let hooks = book.hooks_mut(&key("Mira")).unwrap();
    assert!(hooks.is_wanted(id));
    assert_eq!(hooks.values(id), [1.0]);
    hooks.set_values(id, &[2.0]).unwrap();
    let mut fields = serde_json::Map::new();
    book.write_settings(&mut fields);
    let loaded = ProfileBook::from_settings(Some(&Value::Object(fields)));
    let hooks = &loaded.get(&key("Mira")).unwrap().hooks;
    assert!(hooks.is_wanted(id));
    assert_eq!(hooks.values(id), [2.0]);
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
        "hookValues": { "enemy-damage": [4.0], "camera-distance": [-150.0], "gone": [1.0], "experience-multiplier": [2.0] },
        "Profiles": [
            { "name": "Oren", "class": "Mage", "cycle": "Swordfish", "lastPlayed": "2026-10-17T09:00:00Z", "wanted": ["god-mode"] },
            {
                "name": "Mira", "class": "Mage", "cycle": "Swordfish", "lastPlayed": "2026-10-18T09:00:00Z",
                "wanted": ["camera-distance", "gone", "forge-item", 7], "values": { "camera-distance": [-40.0], "gone": [1.0], "drop-rates": [1.0] }
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
    assert_eq!(mira.hooks.wanted().collect::<Vec<_>>(), [hook("camera-distance")], "only hooks can be wanted");
    assert_eq!(mira.hooks.values(hook("camera-distance")), [-40.]);
    assert_eq!(mira.hooks.values(hook("enemy-damage")), [4.], "what the profile lacks comes from hookValues");
    assert_eq!(
        mira.hooks.values(hook("drop-rates")),
        FeatureKey::Hook(hook("drop-rates")).descriptor().values.iter().map(|value| value.start).collect::<Vec<_>>(),
        "values of the wrong count are dropped"
    );
    let oren = book.get(&key("Oren")).unwrap();
    assert_eq!(oren.hooks.values(hook("camera-distance")), [-100.], "saved values are fitted");
    assert_eq!(
        oren.hooks.values(hook("experience-multiplier")),
        FeatureKey::Hook(hook("experience-multiplier"))
            .descriptor()
            .values
            .iter()
            .map(|value| value.start)
            .collect::<Vec<_>>()
    );
    assert!(oren.hooks.is_wanted(hook("incoming-damage")));
    assert_eq!(book.get(&key("Never played")).unwrap().last_played, None);
}

#[test]
fn nothing_saved_reads_as_no_profiles_with_the_catalogs_values() {
    for slice in [None, Some(json!(null)), Some(json!({ "profiles": "?", "hookValues": [1] })), Some(json!({}))] {
        let book = ProfileBook::from_settings(slice.as_ref());
        assert!(book.profiles().is_empty());
        assert_eq!(book.template().values(hook("enemy-damage")), [default_of("enemy-damage")]);
    }
}

#[test]
fn profiles_are_only_made_when_kept_and_a_character_without_one_starts_from_the_c_sharp_apps_values() {
    let mut book = ProfileBook::from_settings(Some(&json!({ "hookValues": { "enemy-damage": [5.0] } })));
    assert!(!book.played(&key("Mira"), at("2026-10-19T09:00:00Z")), "playing makes no profile");
    assert!(book.profiles().is_empty());
    let template = book.template().clone();
    assert_eq!((template.wanted().count(), template.values(hook("enemy-damage"))), (0, &[5.][..]));
    book.create(key("Mira"), template.clone(), at("2026-10-19T10:00:00Z"));
    let mut oren = template;
    oren.set_wanted(hook("camera-distance"), true);
    book.create(key("Oren"), oren.clone(), at("2026-10-19T11:00:00Z"));
    book.hooks_mut(&key("Mira")).unwrap().set_values(hook("enemy-damage"), &[7.]).unwrap();
    assert!(book.played(&key("Mira"), at("2026-10-19T12:00:00Z")));
    let names: Vec<_> = book.profiles().iter().map(|profile| profile.key.name.as_str()).collect();
    assert_eq!(names, ["Mira", "Oren"]);
    let mira = book.get(&key("Mira")).unwrap();
    assert_eq!((mira.last_played, mira.hooks.values(hook("enemy-damage"))), (Some(at("2026-10-19T12:00:00Z")), &[7.][..]));
    assert_eq!(book.get(&key("Oren")).unwrap().hooks, oren);
    assert_eq!(book.remove(&key("Oren")).map(|profile| profile.hooks), Some(oren));
    assert!(book.remove(&key("Oren")).is_none());
}

#[test]
fn profiles_are_written_beside_the_c_sharp_apps_values_and_read_back_the_same() {
    let mut book = ProfileBook::from_settings(Some(&json!({ "hookValues": { "enemy-damage": [5.0] } })));
    book.create(key("Mira"), book.template().clone(), at("2026-10-19T10:00:00Z"));
    let hooks = book.hooks_mut(&key("Mira")).unwrap();
    hooks.set_wanted(hook("camera-distance"), true);
    hooks.set_values(hook("camera-distance"), &[-25.5]).unwrap();
    let mut fields: Map<String, Value> = serde_json::from_value(json!({
        "autoConnect": false,
        "hookValues": { "enemy-damage": [5.0] },
        "PROFILES": [{ "name": "stale" }]
    }))
    .unwrap();
    book.write_settings(&mut fields);
    assert_eq!(fields["autoConnect"], json!(false));
    assert_eq!(fields["hookValues"], json!({ "enemy-damage": [5.0] }), "hookValues stays as it is");
    assert!(!fields.contains_key("PROFILES"), "one spelling of the key");
    let saved = &fields["profiles"][0];
    assert_eq!(
        (&saved["name"], &saved["lastPlayed"], &saved["wanted"], &saved["values"]["camera-distance"]),
        (&json!("Mira"), &json!("2026-10-19T10:00:00Z"), &json!(["camera-distance"]), &json!([-25.5]))
    );
    assert_eq!(ProfileBook::from_settings(Some(&Value::Object(fields))), book);
}

#[test]
fn combined_damage_migrates_divisors_and_god_mode_with_new_settings_taking_precedence() {
    for (wanted, values, expected_on, expected) in [
        (json!(["damage-taken"]), json!({"damage-taken":[4]}), true, 25.0),
        (json!(["damage-taken"]), json!({"damage-taken":[3]}), true, 33.0),
        (json!(["god-mode", "damage-taken"]), json!({"damage-taken":[4]}), true, 0.0),
        (json!(["damage-taken"]), json!({}), true, 50.0),
        (json!([]), json!({"damage-taken":[4]}), false, 25.0),
        (json!(["god-mode"]), json!({"incoming-damage":[75]}), false, 75.0),
        (json!(["incoming-damage", "god-mode"]), json!({"incoming-damage":[75]}), true, 75.0),
    ] {
        let book = ProfileBook::from_settings(Some(
            &json!({"profiles":[{"name":"Mira","class":"Mage","cycle":"Swordfish","wanted":wanted,"values":values}]}),
        ));
        let hooks = &book.get(&key("Mira")).unwrap().hooks;
        assert_eq!(hooks.is_wanted(hook("incoming-damage")), expected_on);
        assert_eq!(hooks.values(hook("incoming-damage")), [expected]);
        let mut saved = Map::new();
        book.write_settings(&mut saved);
        let text = serde_json::to_string(&saved).unwrap();
        assert!(!text.contains("god-mode") && !text.contains("damage-taken"));
        assert_eq!(book, ProfileBook::from_settings(Some(&Value::Object(saved))));
    }
    let book = ProfileBook::from_settings(Some(&json!({"hookValues":{"damage-taken":[5]}})));
    assert_eq!(book.template().values(hook("incoming-damage")), [20.0]);
    assert!(!book.template().is_wanted(hook("incoming-damage")));
}

#[test]
fn one_hit_profiles_migrate_to_the_multiplier_endpoint_and_round_trip() {
    let book = ProfileBook::from_settings(Some(&json!({"profiles":[{
        "name":"Mira", "class":"Sentinel", "cycle":"1",
        "wanted":["one-hit-kill", "enemy-damage"], "values":{"enemy-damage":[2.5]}
    }]})));
    let hooks = &book.profiles()[0].hooks;
    assert_eq!(hooks.values(hook("enemy-damage")), [101.0]);
    assert!(hooks.wanted().any(|id| id == hook("enemy-damage")));
    assert!(HookId::parse("one-hit-kill").is_none());
    let mut fields = Map::new();
    book.write_settings(&mut fields);
    assert!(!fields["profiles"].to_string().contains("one-hit-kill"));
    assert_eq!(ProfileBook::from_settings(Some(&Value::Object(fields))), book);
}

#[test]
fn renamed_ids_read_as_their_current_names_and_the_current_name_wins() {
    let book = ProfileBook::from_settings(Some(&json!({
        "hookValues": { "zoom": [-40.0], "damage-dealt": [3.0], "enemy-damage": [4.0] },
        "profiles": [{
            "name": "Mira", "class": "Mage", "cycle": "Swordfish",
            "wanted": ["cast-speed", "offhand"], "values": { "cast-speed": [2.0], "xp-multiplier": [3.0, 4.0] }
        }]
    })));
    assert_eq!(book.template().values(hook("camera-distance")), [-40.]);
    assert_eq!(book.template().values(hook("enemy-damage")), [4.], "the current name's value is kept");
    let mira = &book.profiles()[0].hooks;
    assert_eq!(mira.wanted().collect::<Vec<_>>(), [hook("ability-speed"), hook("any-offhand")]);
    assert_eq!(mira.values(hook("ability-speed")), [2.]);
    assert_eq!(mira.values(hook("experience-multiplier")), [3., 4.]);
    let mut fields = Map::new();
    book.write_settings(&mut fields);
    let saved = fields["profiles"].to_string();
    assert!(!saved.contains("cast-speed") && !saved.contains("\"offhand\""), "profiles are written with the current IDs");
    assert_eq!(ProfileBook::from_settings(Some(&Value::Object(fields))).profiles(), book.profiles());
}
