//! The catalog built from a real install's files. Files only: nothing starts, opens or touches the game process.
//! `mise run game-files:install` runs these (`EG_LAST_EPOCH=<game folder>`), after the game-files checks.

use eg_trainer::definitions::catalog::{Availability, GameLocale};
use std::time::Instant;

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn builds_the_catalog_from_the_install() {
    let folder = std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder");
    let started = Instant::now();
    let files = eg_trainer::definitions::files::read(&std::path::Path::new(&folder).join("Last Epoch.exe")).unwrap();
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
fn uniques_become_affixes_with_permanent_ids() {
    use eg_trainer::definitions::{files, unique_mods};
    use eg_trainer_wire::new_affixes::ID_RANGES;
    let folder = std::path::PathBuf::from(std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder"));
    let store = tempfile::tempdir().unwrap();
    let ledger = store.path().join(unique_mods::LEDGER_FILE);
    let locale = GameLocale {
        language: "en".into(),
        game_version: String::new(),
    };
    let added = |files: &files::FileCatalog| {
        let catalog = files.catalog(locale.clone()).unwrap();
        catalog.affixes.into_values().filter(|a| unique_mods::is_added(a.id)).collect::<Vec<_>>()
    };
    let first = files::read_with_ledger(&folder.join("Last Epoch.exe"), Some(&ledger)).unwrap();
    assert_eq!(first.ledger_problem, None);
    let affixes = added(&first);
    let wires: Vec<_> = affixes.iter().filter_map(unique_mods::new_affix).collect();
    assert_eq!(wires.len(), affixes.len(), "every added affix is one the helper can add");
    let capacity = ID_RANGES.into_iter().flatten().count();
    let by_size = wires.iter().fold(std::collections::BTreeMap::<usize, usize>::new(), |mut sizes, affix| {
        *sizes.entry(affix.properties.len()).or_default() += 1;
        sizes
    });
    println!("{} of {capacity} IDs; lines per affix {by_size:?}", wires.len());
    assert!(wires.len() < capacity, "the range holds them all, with room for new uniques");
    let named = |text: &str| wires.iter().filter(|a| a.display_name.contains(text)).map(|a| a.name.clone()).collect::<Vec<_>>();
    println!("{:?}", named("You have no ward"));
    let no_ward = wires.iter().find(|a| a.display_name == "You have no ward").expect("Fractured Crown's line");
    assert_eq!((no_ward.properties.len(), no_ward.properties[0].property, no_ward.properties[0].tags), (1, 98, 471));
    assert_eq!((no_ward.properties[0].minimum, no_ward.properties[0].maximum), (1.0, 1.0));
    for affix in wires.iter().filter(|a| a.properties.len() >= 3).take(5) {
        println!("{} lines: {}", affix.properties.len(), affix.name);
    }
    // In the game's words, with the value modifier of the unique's base (`Affix.getModifier`).
    let source_of = |name: &str| affixes.iter().filter(|a| a.raw_name.contains(name)).collect::<Vec<_>>();
    let coal = source_of("Stygian Coal");
    let ailments = coal
        .iter()
        .find(|a| a.effects.iter().any(|e| e.heading.contains("Ignite")))
        .expect("Stygian Coal's ailment lines");
    assert_eq!(
        ailments.effects.iter().map(|e| e.heading.as_str()).collect::<Vec<_>>(),
        ["Chance to Ignite on Hit While Channelling", "Chance to apply Damned on Hit While Channelling"]
    );
    assert_eq!(
        ailments.raw_display_name,
        "Chance to Ignite on Hit While Channelling, Chance to apply Damned on Hit While Channelling"
    );
    assert!(
        source_of("Fragment of the Enigma")
            .iter()
            .any(|a| a.effects.iter().any(|e| e.heading == "Increased Spark Charge Explosion Area"))
    );
    assert!(affixes.iter().all(|a| !a.raw_name.contains("Special effect")));
    let shipped = first.catalog(locale.clone()).unwrap();
    let ucenui = &shipped.affixes[&705];
    assert!(ucenui.named_by_title);
    assert_eq!(
        ucenui.effects.iter().map(|e| e.heading.as_str()).collect::<Vec<_>>(),
        ["Cold Penetration", "Lightning Penetration"]
    );
    assert!(!shipped.affixes[&25].named_by_title, "Health says what it does");
    let bases = |item_type: u8| shipped.bases[&item_type].sub_types[0].affix_effect_modifier;
    assert_eq!((bases(3), bases(1), bases(12)), (0.0, 0.5, 2.2));
    let two_handed = affixes
        .iter()
        .find(|a| a.source.as_ref().is_some_and(|s| shipped.uniques[&s.unique].item_type == 12))
        .expect("a two-handed axe's modifier");
    assert_eq!(two_handed.standard_effect, 2.2);
    let again = files::read_with_ledger(&folder.join("Last Epoch.exe"), Some(&ledger)).unwrap();
    assert_eq!(added(&again), affixes, "a second reading gives each the same ID");
}

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn a_second_start_loads_the_saved_catalog_instead_of_reading_the_files() {
    use eg_trainer::definitions::service::{self, GameData};
    let folder = std::path::PathBuf::from(std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder"));
    let store = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let owner = task_runtime::Runtime::start().unwrap();
    let mut catalogs = Vec::new();
    for start in ["first start, reads the files", "second start, loads the saved catalog"] {
        let started = Instant::now();
        let service = service::start(None, Some(store.path().to_path_buf()), None, &owner.tasks());
        let GameData::Ready(catalog) = runtime.block_on(service.for_game(&folder.join("Last Epoch.exe"))) else {
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

/// Every line of the game's unique mods and shipped affixes.
fn stat_lines(player: &eg_game_files::PlayerDefinitions, definitions: &eg_game_files::Definitions) -> Vec<(String, eg_trainer::definitions::stat_text::StatKey)> {
    use eg_trainer::definitions::stat_text::StatKey;
    let mut lines = Vec::new();
    for unique in &player.uniques {
        for m in &unique.mods {
            let line = StatKey {
                property: m.property,
                tags: m.tags,
                special_tag: m.special_tag,
                extra_tag: m.extra_tag,
                modifier_type: m.modifier_type,
                negative: m.minimum < 0.0 && m.maximum <= 0.0,
            };
            lines.push((format!("unique {} {}", unique.id, unique.name), line));
        }
    }
    for affix in &definitions.affixes {
        for (index, p) in affix.properties.iter().enumerate() {
            // An affix's first tier says which way its values go.
            let negative = affix.tiers.first().and_then(|t| t.rolls.get(index)).is_some_and(|r| r.min < 0.0 && r.max <= 0.0);
            let line = StatKey {
                property: p.property,
                tags: p.tags,
                special_tag: p.special_tag,
                extra_tag: p.extra_tag,
                modifier_type: p.modifier_type,
                negative,
            };
            lines.push((format!("affix {} {}", affix.id, affix.name), line));
        }
    }
    lines
}

#[test]
#[ignore = "reads a Last Epoch install named by EG_LAST_EPOCH"]
fn stat_lines_read_as_the_game_writes_them() {
    use eg_trainer::definitions::stat_text::{StatKey, StatTexts};
    use std::collections::{BTreeMap, BTreeSet};
    let folder = std::env::var_os("EG_LAST_EPOCH").expect("set EG_LAST_EPOCH to the game folder");
    let install = eg_game_files::Install::of(std::path::Path::new(&folder).join("Last Epoch.exe")).unwrap();
    let (player, definitions) = (install.player_definitions().unwrap(), install.definitions().unwrap());
    let strings = install.strings("en").unwrap();
    let texts = StatTexts::new(&strings, &player, &definitions);
    let line = |property, tags, special_tag, extra_tag, modifier_type| StatKey {
        property,
        tags,
        special_tag,
        extra_tag,
        modifier_type,
        negative: false,
    };
    // Bones of the Ancestral Pack, whose words the game writes "+2 to All Minion Skills".
    assert_eq!(texts.name(&line(88, 8192, 0, 0, 0)).as_deref(), Some("to All Minion Skills"));
    assert_eq!(texts.subject(&line(88, 8192, 0, 0, 0)).as_deref(), Some("All Minion Skills"));
    // Fragment of the Enigma.
    assert_eq!(texts.name(&line(58, 66, 0, 0, 0)).as_deref(), Some("increased Spark Charge Explosion Area"));
    // Stygian Coal.
    assert_eq!(texts.name(&line(1, 262144, 1, 0, 0)).as_deref(), Some("Chance to Ignite on Hit While Channelling"));
    assert_eq!(texts.name(&line(1, 262144, 39, 0, 0)).as_deref(), Some("Chance to apply Damned on Hit While Channelling"));
    // Affix 705, Ucenui's.
    assert_eq!(texts.name(&line(59, 4, 0, 0, 0)).as_deref(), Some("Cold Penetration"));
    assert_eq!(texts.name(&line(59, 2, 0, 0, 0)).as_deref(), Some("Lightning Penetration"));

    let lines = stat_lines(&player, &definitions);
    let distinct: BTreeMap<_, _> = lines
        .iter()
        .map(|(from, l)| ((l.property, l.tags, l.special_tag, l.extra_tag, l.modifier_type, l.negative), (from, l)))
        .collect();
    let mut internal = Vec::new();
    let mut coverage: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut unnamed = Vec::new();
    for (from, l) in distinct.values() {
        let kind = from.split(' ').next().unwrap_or("");
        let counted = coverage.entry(kind).or_default();
        counted.0 += 1;
        let Some(name) = texts.name(l) else {
            unnamed.push(format!("{from}: {l:?}"));
            continue;
        };
        counted.1 += 1;
        // A property or registry entry's stored name passed through without a table key behind it.
        let table_has = |key: String| strings.get("Properties", &key).is_some();
        let bare = match l.property {
            58 => player
                .abilities
                .list
                .iter()
                .find(|e| i32::from(e.ability_id) == l.tags)
                .and_then(|e| e.labels.get(usize::from(l.special_tag)))
                .is_some_and(|label| !table_has(format!("{}_Name", label.text_key))),
            98 => player
                .player
                .labels
                .get(l.tags as usize)
                .is_some_and(|label| !table_has(format!("{}_Name", label.text_key))),
            130 => definitions
                .idol_altar
                .labels
                .get(l.tags as usize)
                .is_some_and(|label| !table_has(format!("{}_Name", label.text_key))),
            _ => player
                .properties
                .iter()
                .any(|p| p.id == l.property && p.name == name && !table_has(format!("Property_Master_{}_Name", p.name))),
        };
        if bare {
            internal.push(format!("{from}: {l:?} {name:?}"));
        }
    }
    println!("{} lines, {} distinct", lines.len(), distinct.len());
    println!("distinct lines named of total: {coverage:?}");
    println!("{} unnamed:", unnamed.len());
    for line in unnamed.iter().take(40) {
        println!("  {line}");
    }
    assert!(internal.is_empty(), "bare stored names: {internal:#?}");
    assert!(unnamed.is_empty(), "lines the game has words for came out without any: {unnamed:#?}");

    // Against the table the translators work from, which holds what the composition wrote when the line was exported.
    let (mut same, mut exact, mut differ) = (0, 0, Vec::new());
    for (key, text) in strings.tables.get("Descriptors").into_iter().flatten() {
        let parts: Vec<i64> = key.split(',').filter_map(|p| p.parse().ok()).collect();
        let (property, tags, special_tag, extra_tag, prefix) = match parts.as_slice() {
            [p, t, s, m] => (*p, *t, *s, 0, *m),
            [p, t, s, e, m] => (*p, *t, *s, *e, *m),
            _ => continue,
        };
        // The prefix as the type and sign that give it; a property that never takes it is not this line.
        let (modifier_type, negative) = match prefix {
            3 => (1, false),
            4 => (1, true),
            5 => (2, false),
            6 => (2, true),
            _ => (0, false),
        };
        let line = StatKey {
            property: property as u8,
            tags: tags as i32,
            special_tag: special_tag as u8,
            extra_tag: extra_tag as i32,
            modifier_type,
            negative,
        };
        let name = texts.name(&line).unwrap_or_default();
        let wanted = if prefix == 2 { format!("to {text}") } else { text.clone() };
        if name.to_lowercase() == wanted.to_lowercase() {
            same += 1;
            exact += usize::from(name == wanted);
        } else {
            differ.push(format!("{key}: game {name:?}, table {text:?}"));
        }
    }
    println!("Descriptors: {same} read the same ({exact} to the letter), {} differ", differ.len());
    for line in &differ {
        println!("  {line}");
    }
    let samples = [
        ("Stygian Coal", line(1, 262144, 1, 0, 0)),
        ("Stygian Coal", line(1, 262144, 39, 0, 0)),
        ("Stygian Coal", line(58, 373, 2, 0, 2)),
        ("Stygian Coal", line(58, 373, 1, 0, 0)),
        ("Bones of the Ancestral Pack", line(88, 8192, 0, 0, 0)),
        ("Fragment of the Enigma", line(58, 66, 0, 0, 0)),
        ("Ucenui's", line(59, 4, 0, 0, 0)),
        ("Ucenui's", line(59, 2, 0, 0, 0)),
        ("Fractured Crown", line(98, 471, 0, 0, 0)),
        ("Summon Bee", line(9, 0, 0, 353, 1)),
        ("Marrow Shards", line(117, 4096, 19, 143, 2)),
        ("Forged Weapon", line(127, 512, 1, 220, 0)),
        ("Melee void over time", line(0, 4096 | 512 | 16, 0, 0, 1)),
        ("Armor mitigation", line(118, 0, 0, 0, 0)),
        ("Idol altar", line(130, 29, 0, 0, 0)),
        ("Stun duration", line(95, 0, 0, 0, 1)),
        ("Ailment effect", line(43, 8192, 8, 0, 0)),
        ("Bleed duration", line(42, 288, 3, 0, 1)),
    ];
    for (from, l) in samples {
        println!("{from}: {:?} / {:?}\n    {:?}", texts.name(&l), texts.subject(&l), texts.explanation(&l));
    }
    let explained = distinct.values().filter(|(_, l)| texts.explanation(l).is_some()).count();
    println!("{explained} of {} distinct lines have an explanation", distinct.len());

    // Every language resolves the same lines. `dev` and `key` are the tables' debug locales.
    for locale in install.locales().unwrap().into_keys().filter(|l| !matches!(l.as_str(), "dev" | "key")) {
        let strings = install.strings(&locale).unwrap();
        let texts = StatTexts::new(&strings, &player, &definitions);
        let missing: BTreeSet<String> = distinct
            .values()
            .filter(|(_, l)| texts.name(l).is_none())
            .map(|(from, l)| format!("{from}: {l:?}"))
            .collect();
        println!(
            "{locale}: {} of {} have no words; {:?} / {:?} / {:?}",
            missing.len(),
            distinct.len(),
            texts.name(&line(59, 4, 0, 0, 0)),
            texts.name(&line(1, 262144, 39, 0, 0)),
            texts.name(&line(88, 8192, 0, 0, 0))
        );
        assert!(missing.is_empty(), "{locale} lacks words: {missing:#?}");
    }
}
