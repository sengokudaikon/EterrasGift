//! The catalog built from a real install's files. Files only: nothing starts, opens or touches the game process.
//! `mise run game-files:install` runs these (`EG_LAST_EPOCH=<game folder>`), after the game-files checks.

use eg_trainer::definitions::catalog::{Availability, GameLocale};
use std::time::Instant;

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn builds_the_catalog_from_the_install() {
    let folder = std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder");
    let started = Instant::now();
    let files = eg_trainer::definitions::files::read(std::path::Path::new(&folder)).unwrap();
    let catalog = files
        .catalog(GameLocale {
            language: "en".into(),
            game_version: String::new(),
        })
        .unwrap();
    println!(
        "{} bases, {} affixes ({} with numeric), {} uniques ({} with numeric) in {:.2?}; limits {:?}",
        catalog.bases.len(),
        catalog.affixes.len(),
        catalog.affixes.values().filter(|a| matches!(a.numeric, Availability::Available(_))).count(),
        catalog.uniques.len(),
        catalog.uniques.values().filter(|u| matches!(u.numeric, Availability::Available(_))).count(),
        started.elapsed(),
        catalog.limits
    );
    let unavailable: std::collections::BTreeMap<_, usize> = catalog
        .affixes
        .values()
        .filter_map(|a| match &a.numeric {
            Availability::Unavailable { reason } => Some(reason.clone()),
            _ => None,
        })
        .fold(Default::default(), |mut m, r| {
            *m.entry(r).or_default() += 1;
            m
        });
    println!("unavailable affix numerics: {unavailable:?}");
    let health = &catalog.affixes[&25];
    let Availability::Available(tiers) = &health.numeric else { panic!("affix 25 numeric") };
    assert_eq!((tiers[0].properties[0].minimum, tiers[0].properties[0].maximum), (5.0, 15.0));
    assert_eq!(catalog.limits.forging_potential.maximum, 255);
    // The game's names, not the stored internal ones ("Blood Rage Minion Pack").
    assert_eq!(health.game_display_name, Availability::Available("Health".into()));
    assert_eq!(catalog.affixes[&1138].game_display_name, Availability::Available("You have Pack Rage".into()));
}

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn a_second_start_loads_the_saved_catalog_instead_of_reading_the_files() {
    use eg_trainer::definitions::service::{self, GameData};
    let folder = std::path::PathBuf::from(std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder"));
    let store = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let mut catalogs = Vec::new();
    for start in ["first start, reads the files", "second start, loads the saved catalog"] {
        let started = Instant::now();
        let service = service::start(None, Some(store.path().to_path_buf()), None).unwrap();
        let GameData::Ready(catalog) = runtime.block_on(service.for_folder(&folder)) else {
            panic!("{start}: {:?}", service.current());
        };
        println!("{start}: {:.2?}", started.elapsed());
        let german = catalog
            .catalog(GameLocale {
                language: "de".into(),
                game_version: String::new(),
            })
            .unwrap();
        catalogs.push((catalog.saved().unwrap(), german));
    }
    assert_eq!(catalogs[0], catalogs[1]);
    // A session in another language takes the game's names in it, saved and loaded with the catalog.
    let (english, german) = &catalogs[1];
    assert_ne!(german.uniques[&3].name.text, english.uniques[&3].name.text);
    assert_ne!(german.affixes[&25].game_display_name, english.affixes[&25].game_display_name);
    // Without the text tables' grammar tags ("[ms]Juwelenstirnreif").
    assert_eq!(german.bases[&0].sub_types[1].name.text, "Juwelenstirnreif");
    println!(
        "de: {:?} / {:?} / {:?}",
        german.uniques[&3].name.text, german.affixes[&25].game_display_name, german.bases[&0].sub_types[1].name.text
    );
}
