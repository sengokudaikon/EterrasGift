//! Reads a real Last Epoch install. Files only: nothing starts, opens or touches the game process.
//! `EG_LAST_EPOCH=<game folder> mise run game-files:install`, which also checks the rebuilt layouts (`src/layout_check.rs`).

use eg_game_files::Install;
use std::time::Instant;

fn install() -> Install {
    let folder = std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder (the one holding Last Epoch.exe)");
    Install::of(std::path::Path::new(&folder).join("Last Epoch.exe")).unwrap()
}

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn reads_definitions_from_the_install() {
    let install = install();
    let started = Instant::now();
    let definitions = install.definitions().unwrap();
    println!(
        "build {}: {} affixes, {} item types in {:.2?}",
        install.build_hash().unwrap(),
        definitions.affixes.len(),
        definitions.item_types.len(),
        started.elapsed()
    );
    // Long-standing definitions (Added Health, helmets).
    let health = definitions.affixes.iter().find(|a| a.id == 25).expect("affix 25");
    assert_eq!((health.name.as_str(), health.kind, health.multi), ("Added Health", 1, false));
    assert_eq!((health.tiers[0].rolls[0].min, health.tiers[0].rolls[0].max), (5.0, 15.0));
    assert!(definitions.affixes.iter().any(|a| a.multi && a.properties.len() > 1));
    println!("{} ailments, {} idol altar properties", definitions.ailments.len(), definitions.idol_altar.list.len());
    assert!(definitions.ailments.iter().any(|a| a.effect_of_increased_effectiveness == 1));
    assert!(!definitions.idol_altar.list.is_empty());
    // An ability's ID is its place in the list, and the object's name keys its text.
    assert_eq!(definitions.abilities[353].as_ref().map(|a| a.key.as_str()), Some("SummonBee"));
    assert!(definitions.classes.iter().any(|c| c.name == "Mage"));
    assert_eq!(definitions.idol_altar.labels.len(), definitions.idol_altar.list.len());
    let helmets = definitions.item_types.iter().find(|t| t.id == 0).expect("item type 0");
    assert_eq!(helmets.name, "Helmets");
    assert!(!helmets.sub_types.is_empty());
}

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn reads_text_for_every_shipped_locale() {
    let install = install();
    let locales = install.locales().unwrap();
    println!("locales: {:?}", locales.keys().collect::<Vec<_>>());
    assert!(locales.contains_key("en"));
    let started = Instant::now();
    let strings = install.strings("en").unwrap();
    let count: usize = strings.tables.values().map(|t| t.len()).sum();
    println!(
        "en: {} tables, {count} strings ({} unkeyed, {} shadowed) in {:.2?}",
        strings.tables.len(),
        strings.unkeyed,
        strings.shadowed,
        started.elapsed()
    );
    assert_eq!(strings.get("Item_Affixes", "Item_Affix_25_DisplayName"), Some("Health"));
    assert_eq!(strings.get("Item_Names", "Item_BaseType_Name_0"), Some("Helmet"));
    assert!(install.strings("xx").is_err());
}

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn reads_uniques_properties_and_sets_from_the_player_data() {
    let install = install();
    let started = Instant::now();
    let player = install.player_definitions().unwrap();
    println!(
        "{} uniques, {} properties, {} set bonuses in {:.2?}",
        player.uniques.len(),
        player.properties.len(),
        player.set_bonuses.len(),
        started.elapsed()
    );
    let revenge = player.uniques.iter().find(|u| u.id == 5).expect("unique 5");
    assert_eq!((revenge.name.as_str(), revenge.is_set_item, revenge.set_id), ("IsadoraRevenge", true, 1));
    assert!(revenge.mods.iter().any(|m| m.property == 59 && m.can_roll));
    assert_eq!(player.ailment_names[&1], "Ignite");
    assert_eq!(player.ailment_names[&39], "Damned");
    assert_eq!(player.conditional_damage[0].description, "to Stunned Enemies");
    assert!(player.stat_limits.iter().any(|l| l.has_proc_limit));
    assert_eq!(player.player.labels[0].text_key, "Property_Player_0");
    assert!(player.tracker.labels.is_empty());
    let isadora = player.set_bonuses.iter().find(|s| s.id == 1).expect("set 1");
    assert_eq!((isadora.name.as_str(), isadora.items.as_slice()), ("Isadora's", &[5u16, 16, 26][..]));
    assert!(player.properties.iter().any(|p| p.id == 31 && p.display_added_as_percentage));
    println!(
        "constants {:?}; {} ability entries, {} player, {} tracker",
        player.constants,
        player.abilities.list.len(),
        player.player.list.len(),
        player.tracker.list.len()
    );
    assert!(player.constants.max_level > 0 && player.constants.forging_potential.0 <= player.constants.forging_potential.1);
    assert!(!player.abilities.list.is_empty() && !player.player.list.is_empty());
}
