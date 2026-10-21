//! Reads a real Last Epoch install. Files only: nothing starts, opens or touches the game process.
//! `EG_LAST_EPOCH=<game folder> cargo test -p eg-game-files --release -- --ignored --nocapture` (`mise run game-files:install`).

use eg_game_files::Install;
use std::time::Instant;

fn install() -> Install {
    let folder = std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder (the one holding Last Epoch.exe)");
    Install::at(folder).unwrap()
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
    // Long-standing definitions, as the live capture and the forge fixture know them.
    let health = definitions.affixes.iter().find(|a| a.id == 25).expect("affix 25");
    assert_eq!((health.name.as_str(), health.kind, health.multi), ("Added Health", 1, false));
    assert_eq!((health.tiers[0].rolls[0].min, health.tiers[0].rolls[0].max), (5.0, 15.0));
    assert!(definitions.affixes.iter().any(|a| a.multi && a.properties.len() > 1));
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
