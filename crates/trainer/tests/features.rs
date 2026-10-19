use std::collections::BTreeSet;

use eg_trainer::features::{
    DESCRIPTORS, FeatureError, FeatureIntents, FeatureKey, FeatureKind, FieldSetId, HookId, NativeSupport, PAGES, PhysicalState, ReportedState, SOURCE_COMMIT, SOURCE_FILES,
    SavedValues, TrainerArea,
};

type ValueGolden<'a> = (&'a str, f64, f64, f64, f64, Option<&'a str>);
type FieldGolden<'a> = (&'a str, f64, f64, f64);

fn hook(id: &str) -> HookId {
    HookId::parse(id).unwrap()
}

fn saved(id: &str, values: &[f64]) -> SavedValues {
    SavedValues {
        id: id.to_owned(),
        values: values.to_vec(),
    }
}

// Literal navigation-order source inventory, not generated from the Rust registry.
// TrainerCatalog.cs at e1c1ed4e9e0ab6404574113f2dcf1179b2113629.
const SOURCE_IDS: &str = "\
xp-multiplier instant-level max-skill-level points-earned free-skill-points free-passive-points free-weaver-points passive-chain skill-slots item-requirements set-bonus offhand primordial-limit
currency keys woven-echoes duplicate
god-mode damage-taken one-hit-kill damage-dealt cast-speed projectiles area attack-leech zero-mana-cost infinite-mana no-cooldowns endless-buffs move-speed auto-target
forge-item forge-level shards runes-glyphs woven-enchanter
drop-rates boss-drops extra-rares force-rarity always-t7 perfect-rolls pickup-range
monolith-run skip-quest-echoes max-bonus-stability echo-cycle echo-type reveal-objective
faction-standing faction-gains instant-rank prophecy-any-zone prophecy-max-lp prophecy-rewards prophecy-uses hunter-cost
rune-wardens rune-prison experimental-uniques class-experimentals loot-lizards eternity-cache favor-cache nemesis mysterious-rift omen-windows morditas-timer monster-density pack-size monster-rarity champions eternity-cache-anywhere reveal-map waypoints zoom";

#[test]
fn all_source_ids_are_ordered_unique_typed_and_unsupported() {
    assert_eq!(SOURCE_COMMIT, "e1c1ed4e9e0ab6404574113f2dcf1179b2113629");
    assert!(SOURCE_FILES.contains(&(
        "src/Trainer/EterrasGift.Trainer.Application/Features/TrainerCatalog.cs",
        "8cf232cea9f2199672031235c61c170644051382c09aa33442cf550c4b21e2c1"
    )));
    let expected: Vec<_> = SOURCE_IDS.split_whitespace().collect();
    assert_eq!(expected.len(), 76);
    assert_eq!(DESCRIPTORS.iter().map(|descriptor| descriptor.key.as_str()).collect::<Vec<_>>(), expected);
    assert_eq!(expected.iter().copied().collect::<BTreeSet<_>>().len(), 76);
    assert_eq!(DESCRIPTORS.iter().filter(|descriptor| matches!(descriptor.key, FeatureKey::Hook(_))).count(), 68);
    assert_eq!(DESCRIPTORS.iter().filter(|descriptor| matches!(descriptor.key, FeatureKey::Fields(_))).count(), 7);
    assert_eq!(DESCRIPTORS.iter().filter(|descriptor| descriptor.key == FeatureKey::Forge).count(), 1);
    for descriptor in DESCRIPTORS {
        assert_eq!(FeatureKey::parse(descriptor.key.as_str()), Some(descriptor.key));
        assert_eq!(descriptor.key.descriptor(), descriptor);
        assert_eq!(descriptor.native_support, NativeSupport::Unsupported);
        assert!(!descriptor.title.is_empty());
        assert!(!descriptor.description.is_empty());
        assert_eq!(descriptor.fills_section, descriptor.key == FeatureKey::Forge);
        match descriptor.key {
            FeatureKey::Hook(id) => {
                assert_eq!(HookId::parse(id.as_str()), Some(id));
                assert!(FieldSetId::parse(id.as_str()).is_none());
                assert!(matches!(descriptor.kind, FeatureKind::Switch | FeatureKind::SwitchWithValues));
                assert!(!descriptor.legacy_owners.is_empty());
                assert!(descriptor.fields.is_empty());
            }
            FeatureKey::Fields(id) => {
                assert_eq!(FieldSetId::parse(id.as_str()), Some(id));
                assert!(HookId::parse(id.as_str()).is_none());
                assert!(!descriptor.fields.is_empty());
                assert!(descriptor.values.is_empty());
                assert!(descriptor.legacy_owners.is_empty());
            }
            FeatureKey::Forge => {
                assert!(HookId::parse("forge-item").is_none());
                assert!(FieldSetId::parse("forge-item").is_none());
            }
        }
    }
    for unknown in ["", "XP-MULTIPLIER", "xp-multiplier ", "unknown", "currency\0"] {
        assert!(FeatureKey::parse(unknown).is_none());
    }
}

#[test]
fn pages_and_original_section_order_are_preserved() {
    use TrainerArea::*;
    let expected = [
        (Character, "Character", "IconCharacter", vec!["Progression", "Trees", "Equipment rules"], true, 13),
        (Stash, "Stash", "IconStash", vec!["Currency", "Keys", "Woven echoes", "Duplicate"], false, 4),
        (Combat, "Combat", "IconCombat", vec!["Survival", "Offense", "Resources", "Movement"], false, 14),
        (Forge, "Forge", "IconForge", vec!["Item", "Crafting rules"], false, 5),
        (Loot, "Loot", "IconLoot", vec!["Drops", "Quality", "Pickup"], false, 7),
        (Monolith, "Monolith", "IconMonolith", vec!["Run", "Echo web"], false, 6),
        (Factions, "Factions", "IconFactions", vec!["Standing", "Prophecies", "Vendors"], false, 8),
        (World, "World", "IconWorld", vec!["Encounters", "Monsters", "Dungeons", "Map"], false, 19),
    ];
    assert_eq!(PAGES.len(), expected.len());
    for (page, (area, title, icon, sections, stacked, count)) in PAGES.iter().zip(expected) {
        assert_eq!(
            (page.area, page.title, page.icon, page.sections, page.stacked),
            (area, title, icon, sections.as_slice(), stacked)
        );
        assert_eq!(DESCRIPTORS.iter().filter(|descriptor| descriptor.area == area).count(), count);
        for descriptor in DESCRIPTORS.iter().filter(|descriptor| descriptor.area == area) {
            assert!(page.sections.contains(&descriptor.section));
        }
    }
}

#[test]
fn source_parameter_defaults_bounds_steps_choices_and_units() {
    // Literal expansion of the source's Multiplier/Count/Rarity factories and
    // exceptional FeatureValue constructors. Step is presentation, not a grid.
    let expected: &[(&str, &[ValueGolden<'_>])] = &[
        (
            "xp-multiplier",
            &[("Character experience", 2., 1., 100., 0.5, Some("×")), ("Skill experience", 2., 1., 100., 0.5, Some("×"))],
        ),
        ("duplicate", &[("Stack multiplier", 2., 1., 20., 1., Some("×"))]),
        ("damage-taken", &[("Divide by", 2., 1., 100., 0.5, None)]),
        ("damage-dealt", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        ("cast-speed", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        ("projectiles", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        ("area", &[("Increased area", 3., 0., 50., 0.5, None)]),
        ("move-speed", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        (
            "drop-rates",
            &[
                ("Item drop chance", 0.1, 0., 1., 0.05, None),
                ("Item drop multiplier", 1., 0., 100., 0.5, Some("×")),
                ("Gold drop chance", 0.1, 0., 1., 0.05, None),
                ("Gold drop multiplier", 2., 0., 100., 0.5, Some("×")),
            ],
        ),
        ("force-rarity", &[("Rarity", 3., 0., 100., 0.5, None)]),
        ("echo-type", &[("Echo type", 1., 0., 100., 0.5, None)]),
        ("faction-gains", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        ("prophecy-rewards", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        ("rune-wardens", &[("Rune Wardens", 2., 0., 20., 1., None)]),
        ("loot-lizards", &[("Loot lizards", 3., 0., 20., 1., None)]),
        (
            "morditas-timer",
            &[
                ("Starting time", 2., 1., 10., 0.5, Some("×")),
                ("Timer speed", 0.5, 0., 1., 0.05, Some("×")),
                ("Time per kill", 2., 1., 10., 0.5, Some("×")),
            ],
        ),
        ("monster-density", &[("Multiplier", 2., 0., 100., 0.5, Some("×"))]),
        ("pack-size", &[("Multiplier", 2., 0., 10., 0.5, Some("×"))]),
        (
            "monster-rarity",
            &[
                ("Normal", 1., 0., 20., 0.5, Some("×")),
                ("Magic", 2., 0., 20., 0.5, Some("×")),
                ("Rare", 3., 0., 20., 0.5, Some("×")),
            ],
        ),
        ("champions", &[("Champions", 2., 0., 20., 1., None)]),
        ("zoom", &[("Zoom distance", -30., -100., 0., 0.5, None)]),
    ];
    assert_eq!(DESCRIPTORS.iter().filter(|descriptor| !descriptor.values.is_empty()).count(), expected.len());
    for (id, specifications) in expected {
        let descriptor = FeatureKey::parse(id).unwrap().descriptor();
        let actual: Vec<_> = descriptor
            .values
            .iter()
            .map(|value| (value.label, value.start, value.minimum, value.maximum, value.step, value.unit))
            .collect();
        assert_eq!(actual, *specifications, "{id}");
        for value in descriptor.values {
            assert!(value.accepts(value.start));
            if value.choices.is_empty() {
                assert_eq!(value.default.parse::<f64>().unwrap(), value.start);
            } else {
                assert_eq!(value.choices[value.start as usize], value.default);
            }
        }
    }
    assert_eq!(
        FeatureKey::parse("force-rarity").unwrap().descriptor().values[0].choices,
        ["Normal", "Magic", "Rare", "Unique", "Set"]
    );
    assert_eq!(
        FeatureKey::parse("echo-type").unwrap().descriptor().values[0].choices,
        [
            "Normal",
            "Arena",
            "Shade",
            "Beacon",
            "Vessel of Chaos",
            "Vessel of Memory",
            "Sanctuary of Eterra",
            "Cemetery of the Erased",
            "Super Arena",
            "Super Beacon"
        ]
    );
}

#[test]
fn all_seven_field_sets_preserve_ui_bounds_and_faction_groups() {
    let expected: &[(&str, &[FieldGolden<'_>])] = &[
        ("points-earned", &[("Passive points", 0., 65535., 1.), ("Weaver points", 0., 65535., 1.)]),
        (
            "currency",
            &[("Gold", 0., 2147483647., 1.), ("Soul embers", 0., 2147483647., 1.), ("Ancient bones", 0., 2147483647., 1.)],
        ),
        ("keys", &[("A count for each one you hold", 0., 2147483647., 1.)]),
        ("woven-echoes", &[("A count for each one you hold", 0., 2147483647., 1.)]),
        ("attack-leech", &[("Attack rate", 0.01, 20., 0.1), ("Increased leech rate", -1., 100., 0.1)]),
        (
            "monolith-run",
            &[("Corruption", 0., 2147483647., 1.), ("Gaze of Orobyss", 0., 10000., 1.), ("Options rerolled", 0., 1., 1.)],
        ),
    ];
    for (id, fields) in expected {
        let descriptor = FeatureKey::Fields(FieldSetId::parse(id).unwrap()).descriptor();
        assert_eq!(
            descriptor
                .fields
                .iter()
                .map(|field| (field.label, field.minimum, field.maximum, field.step))
                .collect::<Vec<_>>(),
            *fields
        );
        assert!(descriptor.fields.iter().all(|field| field.group.is_none()));
    }
    let standing = FeatureKey::parse("faction-standing").unwrap().descriptor();
    assert_eq!(standing.fields.len(), 12);
    for (group, fields) in ["Circle of Fortune", "Merchant's Guild", "Forgotten Knights", "Weaver"]
        .into_iter()
        .zip(standing.fields.as_chunks::<3>().0)
    {
        assert_eq!(fields.iter().map(|field| field.label).collect::<Vec<_>>(), ["Rank", "Favor", "Reputation"]);
        assert!(fields.iter().all(|field| field.group == Some(group) && field.minimum == 0. && field.step == 1.));
        assert_eq!(fields.iter().map(|field| field.maximum).collect::<Vec<_>>(), [2147483647., 999999., 2147483647.]);
    }
    // These are source UI bounds; runtime rank caps and joined-faction checks are
    // deliberately not fabricated as static catalog authority.
}

#[test]
fn needs_hints_and_help_match_source_order_and_text() {
    let needs: Vec<_> = DESCRIPTORS
        .iter()
        .filter_map(|descriptor| descriptor.needs.map(|needs| (descriptor.key.as_str(), needs)))
        .collect();
    assert_eq!(
        needs,
        [
            ("free-weaver-points", "At least one Weaver point earned."),
            ("keys", "At least one of the item in your stash."),
            ("woven-echoes", "At least one of the echo in your stash."),
            ("duplicate", "Drop the item while this is on."),
            ("move-speed", "Re-equip your boots after changing it."),
            ("monolith-run", "A Monolith run selected. Open a timeline and choose its difficulty first.")
        ]
    );
    assert_eq!(
        FeatureKey::parse("area").unwrap().descriptor().help(),
        "Area skills use this increased area instead of your own.\nIncreased area: 1 is 100% increased area."
    );
    assert_eq!(
        FeatureKey::parse("morditas-timer").unwrap().descriptor().help(),
        "Gives a Rage of Morditas hunt more time: a longer start, a slower countdown, more time for each kill.\nStarting time: From the next hunt you start.\nTimer speed: 1 is the game's own; 0 stops the countdown, so the hunt ends when no foes are left.\nTime per kill: As in the game, a kill adds less the longer the hunt has grown."
    );
    assert_eq!(
        FeatureKey::parse("zoom").unwrap().descriptor().help(),
        "Sets how far the camera is from your character.\nZoom distance: The game's own is -17.5."
    );
}

#[test]
fn saved_values_fit_legacy_rules_and_switches_start_off() {
    let state = FeatureIntents::from_saved([
        saved("xp-multiplier", &[0., 120.]),
        saved("damage-dealt", &[f64::NAN]),
        saved("cast-speed", &[f64::INFINITY]),
        saved("projectiles", &[f64::NEG_INFINITY]),
        saved("force-rarity", &[2.5]),
        saved("echo-type", &[9.]),
        saved("zoom", &[-150.]),
        saved("rune-wardens", &[3.25]),
        saved("drop-rates", &[0.123, 1.25, 0.777, 2.75]),
        saved("unknown", &[f64::NAN]),
        saved("currency", &[1., 2., 3.]),
        saved("forge-item", &[]),
        saved("god-mode", &[]),
        saved("morditas-timer", &[8.]),
    ])
    .unwrap();
    assert_eq!(state.wanted().count(), 0);
    for descriptor in DESCRIPTORS {
        if let FeatureKey::Hook(id) = descriptor.key {
            assert!(!state.is_wanted(id));
            assert!(state.reported(id).is_none());
            assert_eq!(state.physical(id), PhysicalState::Unknown);
        }
    }
    for (id, values) in [
        ("xp-multiplier", vec![1., 100.]),
        ("damage-dealt", vec![2.]),
        ("cast-speed", vec![2.]),
        ("projectiles", vec![2.]),
        ("force-rarity", vec![3.]),
        ("echo-type", vec![9.]),
        ("zoom", vec![-100.]),
        ("rune-wardens", vec![3.25]),
        ("drop-rates", vec![0.123, 1.25, 0.777, 2.75]),
        ("morditas-timer", vec![2., 0.5, 2.]),
    ] {
        assert_eq!(state.values(hook(id)), values, "{id}");
    }
    for invalid in [-1., 5., f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.1] {
        let fitted = FeatureIntents::from_saved([saved("force-rarity", &[invalid])]).unwrap();
        assert_eq!(fitted.values(hook("force-rarity")), [3.]);
    }
}

#[test]
fn live_edits_are_strict_transactional_and_do_not_quantize_steps() {
    let mut state = FeatureIntents::default();
    let xp = hook("xp-multiplier");
    state.set_wanted(xp, true).unwrap();
    let before = state.clone();
    for values in [&[5.][..], &[5., f64::NAN], &[5., f64::INFINITY], &[5., 100.01], &[0.99, 5.]] {
        assert!(state.set_values(xp, values).is_err());
        assert_eq!(state, before);
    }
    assert!(matches!(state.set_values(xp, &[1.]), Err(FeatureError::WrongArity { expected: 2, actual: 1, .. })));
    assert!(state.set_values(xp, &[1., 100.]).unwrap());
    assert!(!state.set_values(xp, &[1., 100.]).unwrap());
    assert!(state.set_values(hook("rune-wardens"), &[3.25]).unwrap());
    assert_eq!(state.values(hook("rune-wardens")), [3.25]);
    assert!(state.set_values(hook("drop-rates"), &[0.123, 1.234, 0.456, 7.89]).unwrap());
    assert!(state.set_values(hook("zoom"), &[-17.25]).unwrap());
    for invalid in [-1., 5., 1.2, f64::NAN, f64::INFINITY] {
        let before = state.clone();
        assert!(state.set_values(hook("force-rarity"), &[invalid]).is_err());
        assert_eq!(state, before);
    }
    assert!(state.set_values(hook("force-rarity"), &[0.]).unwrap());
    assert!(state.set_values(hook("force-rarity"), &[4.]).unwrap());
    assert!(state.is_wanted(xp));
    assert_eq!(state.report_revision(), 0);
}

#[test]
fn duplicate_bulk_ids_reject_everything_including_unknown_and_bad_arity() {
    let mut state = FeatureIntents::default();
    state.set_wanted(hook("god-mode"), true).unwrap();
    let before = state.clone();
    for duplicate in ["unknown", "currency", "xp-multiplier", "god-mode"] {
        let error = state
            .restore_values([saved("zoom", &[-42.]), saved(duplicate, &[9.]), saved(duplicate, &[10.])])
            .unwrap_err();
        assert_eq!(error, FeatureError::DuplicateId(duplicate.to_owned()));
        assert_eq!(state, before);
    }
    assert!(state.restore_values([saved("not-present", &[8.]), saved("xp-multiplier", &[9.])]).is_ok());
    assert_eq!(state, before);
}

#[test]
fn persistence_only_carries_values_never_wanted_or_reports() {
    let mut current = FeatureIntents::default();
    let id = hook("damage-dealt");
    current.set_values(id, &[3.125]).unwrap();
    current.set_wanted(id, true).unwrap();
    current.replace_reported([(id, ReportedState { on: true, failure: None })]).unwrap();
    let saved = current.saved_values();
    assert!(saved.iter().all(|entry| !entry.values.is_empty()));
    assert!(saved.iter().all(|entry| HookId::parse(&entry.id).is_some()));
    assert!(saved.iter().all(|entry| !["god-mode", "currency", "forge-item"].contains(&entry.id.as_str())));
    let restored = FeatureIntents::from_saved(saved.clone()).unwrap();
    assert_eq!(restored.values(id), [3.125]);
    assert_eq!(restored.wanted().count(), 0);
    assert_eq!(restored.reported(id), None);
    assert_eq!(restored.physical(id), PhysicalState::Unknown);
    // Saved data is owned; editing it cannot mutate the source session.
    let mut edited = saved;
    edited.iter_mut().find(|entry| entry.id == "damage-dealt").unwrap().values[0] = 8.;
    assert_eq!(current.values(id), [3.125]);
    current.set_wanted(id, false).unwrap();
    assert_eq!(current.values(id), [3.125]);
}

#[test]
fn wanted_logical_report_physical_unknown_and_revisions_are_independent() {
    let mut state = FeatureIntents::default();
    let id = hook("god-mode");
    assert_eq!((state.intent_revision(), state.report_revision()), (0, 0));
    assert!(!state.set_wanted(id, false).unwrap());
    assert!(state.set_wanted(id, true).unwrap());
    assert!(!state.set_wanted(id, true).unwrap());
    assert_eq!((state.intent_revision(), state.report_revision()), (1, 0));
    state
        .replace_reported([(
            id,
            ReportedState {
                on: false,
                failure: Some("source target is unsupported".to_owned()),
            },
        )])
        .unwrap();
    assert!(state.is_wanted(id));
    assert_eq!(state.physical(id), PhysicalState::Unknown);
    assert_eq!((state.intent_revision(), state.report_revision()), (1, 1));
    let before = state.clone();
    assert!(
        state
            .replace_reported([(id, ReportedState { on: true, failure: None }), (id, ReportedState { on: false, failure: None })])
            .is_err()
    );
    assert_eq!(state, before);
    state.replace_reported([]).unwrap();
    assert_eq!(state.physical(id), PhysicalState::Unknown);
    assert!(state.is_wanted(id));
    assert_eq!((state.intent_revision(), state.report_revision()), (1, 2));
    state.restore_values([saved("damage-dealt", &[7.])]).unwrap();
    assert_eq!((state.intent_revision(), state.report_revision()), (2, 2));
}

#[test]
fn shared_owner_metadata_does_not_couple_switches_or_claim_effects() {
    let health = ["god-mode", "damage-taken", "one-hit-kill", "damage-dealt"];
    for id in health {
        assert_eq!(FeatureKey::parse(id).unwrap().descriptor().legacy_owners, ["healthDamageHook"]);
    }
    assert_eq!(FeatureKey::parse("xp-multiplier").unwrap().descriptor().legacy_owners, ["gainExpHook"]);
    assert_eq!(FeatureKey::parse("instant-level").unwrap().descriptor().legacy_owners, ["gainExpHook"]);
    let owners: BTreeSet<_> = DESCRIPTORS.iter().flat_map(|descriptor| descriptor.legacy_owners.iter().copied()).collect();
    assert_eq!(owners.len(), 74);
    let mut state = FeatureIntents::default();
    state.set_wanted(hook("god-mode"), true).unwrap();
    state.set_wanted(hook("damage-taken"), true).unwrap();
    state.set_wanted(hook("god-mode"), false).unwrap();
    assert!(state.is_wanted(hook("damage-taken")));
    assert!(!state.is_wanted(hook("one-hit-kill")));
    for id in health {
        assert_eq!(state.physical(hook(id)), PhysicalState::Unknown);
        assert_eq!(state.reported(hook(id)), None);
        assert_eq!(FeatureKey::parse(id).unwrap().descriptor().native_support, NativeSupport::Unsupported);
    }
}
