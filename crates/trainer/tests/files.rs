//! The catalog built from a real install's files. Files only: nothing starts, opens or touches the game process.
//! `EG_LAST_EPOCH=<game folder> cargo test -p eg-trainer --release --test files -- --ignored --nocapture`.

use eg_trainer::catalog::{Availability, GameLocale};
use std::time::Instant;

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn builds_the_catalog_from_the_install() {
    let folder = std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder");
    let started = Instant::now();
    let files = eg_trainer::files::read(std::path::Path::new(&folder)).unwrap();
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
}
