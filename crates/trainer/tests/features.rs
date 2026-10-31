use std::collections::BTreeSet;

use eg_trainer::features::{DESCRIPTORS, FeatureError, FeatureIntents, FeatureKey, FeatureKind, FieldSetId, HookId, NativeSupport, PAGES, SavedValues};
use eg_trainer_wire::hooks::{GameplayHook, ValueKind};

fn hook(id: &str) -> HookId {
    HookId::parse(id).unwrap()
}

fn saved(id: &str, values: &[f64]) -> SavedValues {
    SavedValues {
        id: id.to_owned(),
        values: values.to_vec(),
    }
}

#[test]
fn ids_are_unique_and_each_key_is_typed_by_its_kind() {
    let ids: BTreeSet<_> = DESCRIPTORS.iter().map(|descriptor| descriptor.key.as_str()).collect();
    assert_eq!(ids.len(), DESCRIPTORS.len());
    assert_eq!(DESCRIPTORS.iter().filter(|descriptor| descriptor.key == FeatureKey::Forge).count(), 1);
    for descriptor in DESCRIPTORS {
        assert_eq!(FeatureKey::parse(descriptor.key.as_str()), Some(descriptor.key));
        assert_eq!(descriptor.key.descriptor(), descriptor);
        assert!(!descriptor.title.get().is_empty());
        assert!(!descriptor.description.get().is_empty());
        match descriptor.key {
            FeatureKey::Hook(id) => {
                assert_eq!(HookId::parse(id.as_str()), Some(id));
                assert!(FieldSetId::parse(id.as_str()).is_none());
                assert!(matches!(descriptor.kind, FeatureKind::Switch | FeatureKind::SwitchWithValues | FeatureKind::Choice));
                assert!(descriptor.fields.is_empty());
            }
            FeatureKey::Fields(id) => {
                assert_eq!(FieldSetId::parse(id.as_str()), Some(id));
                assert!(HookId::parse(id.as_str()).is_none());
                assert!(!descriptor.fields.is_empty());
                assert!(descriptor.values.is_empty());
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
fn every_gameplay_hook_runs_exactly_one_hook_feature() {
    for hook in GameplayHook::ALL {
        let descriptor = FeatureKey::parse(hook.id()).unwrap().descriptor();
        assert_eq!(descriptor.key, FeatureKey::Hook(HookId::parse(hook.id()).unwrap()));
        assert_eq!(descriptor.native_support(), NativeSupport::GameplayOverride(hook));
    }
    let supported = DESCRIPTORS.iter().filter(|descriptor| descriptor.native_support() != NativeSupport::Unsupported).count();
    assert_eq!(supported, GameplayHook::ALL.len());
}

#[test]
fn each_hooks_values_are_what_the_helper_takes() {
    for hook in GameplayHook::ALL {
        let descriptor = FeatureKey::parse(hook.id()).unwrap().descriptor();
        let id = hook.id();
        match (hook.value_kind(), descriptor.values) {
            (ValueKind::Switch, []) => assert_eq!(descriptor.kind, FeatureKind::Switch, "{id}"),
            (ValueKind::Percent, [value]) => assert_eq!((value.minimum, value.maximum, value.step, value.choices.len()), (0.0, 100.0, 1.0, 0), "{id}"),
            (ValueKind::Real { min, max, infinity, .. }, [value]) => {
                assert_eq!((value.minimum, value.maximum, value.choices.len()), (min, infinity.unwrap_or(max), 0), "{id}");
            }
            (ValueKind::RealPair { min, max, .. }, [first, second]) => {
                for value in [first, second] {
                    assert_eq!((value.minimum, value.maximum, value.choices.len()), (min, max, 0), "{id}");
                }
            }
            (ValueKind::Choice(sent), [value]) => assert_eq!(value.choices.len(), sent.len(), "{id}"),
            (kind, values) => panic!("{id}: {kind:?} with {} values", values.len()),
        }
    }
}

#[test]
fn every_feature_sits_in_a_section_of_its_areas_one_page() {
    for page in PAGES {
        assert_eq!(PAGES.iter().filter(|other| other.area == page.area).count(), 1);
    }
    for descriptor in DESCRIPTORS {
        let page = PAGES.iter().find(|page| page.area == descriptor.area).unwrap();
        assert!(page.sections.contains(&descriptor.section), "{}", descriptor.key.as_str());
    }
}

#[test]
fn values_start_in_their_ranges() {
    for descriptor in DESCRIPTORS {
        for value in descriptor.values {
            assert!(value.minimum <= value.maximum && value.step > 0.0, "{}", descriptor.key.as_str());
            assert!(value.accepts(value.start), "{}", descriptor.key.as_str());
        }
        for field in descriptor.fields {
            assert!(field.minimum <= field.maximum && field.step > 0.0, "{}", descriptor.key.as_str());
        }
    }
}

#[test]
fn help_joins_description_needs_and_value_hints() {
    assert_eq!(
        FeatureKey::parse("morditas-timer").unwrap().descriptor().help(),
        "More time in Rage of Morditas hunts: a longer start, a slower countdown and more time per kill.\n\
         Starting time: From your next hunt.\n\
         Timer speed: 1 is normal. 0 stops the countdown, and the hunt ends when no foes are left.\n\
         Time per kill: Kills add less as the hunt goes on, as in the game."
    );
    assert_eq!(
        FeatureKey::parse("movement-speed").unwrap().descriptor().help(),
        "You move faster.\nRe-equip your boots after changing it."
    );
}

#[test]
fn saved_values_fit_legacy_rules_and_switches_start_off() {
    let state = FeatureIntents::from_saved([
        saved("experience-multiplier", &[0., 120.]),
        saved("enemy-damage", &[f64::NAN]),
        saved("ability-speed", &[f64::INFINITY]),
        saved("extra-projectiles", &[f64::NEG_INFINITY]),
        saved("new-item-rarity", &[2.5]),
        saved("one-echo-type", &[9.]),
        saved("camera-distance", &[-150.]),
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
        }
    }
    for (id, values) in [
        ("experience-multiplier", vec![1., 100.]),
        ("enemy-damage", vec![2.]),
        ("ability-speed", vec![2.]),
        ("extra-projectiles", vec![2.]),
        ("new-item-rarity", vec![3.]),
        ("one-echo-type", vec![9.]),
        ("camera-distance", vec![-100.]),
        ("rune-wardens", vec![3.25]),
        ("drop-rates", vec![0.123, 1.25, 0.777, 2.75]),
        ("morditas-timer", vec![2., 0.5, 2.]),
    ] {
        assert_eq!(state.values(hook(id)), values, "{id}");
    }
    for invalid in [-1., 5., f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.1] {
        let fitted = FeatureIntents::from_saved([saved("new-item-rarity", &[invalid])]).unwrap();
        assert_eq!(fitted.values(hook("new-item-rarity")), [3.]);
    }
}

#[test]
fn live_edits_are_strict_transactional_and_do_not_quantize_steps() {
    let mut state = FeatureIntents::default();
    let xp = hook("experience-multiplier");
    state.set_wanted(xp, true);
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
    assert!(state.set_values(hook("camera-distance"), &[-17.25]).unwrap());
    for invalid in [-1., 5., 1.2, f64::NAN, f64::INFINITY] {
        let before = state.clone();
        assert!(state.set_values(hook("new-item-rarity"), &[invalid]).is_err());
        assert_eq!(state, before);
    }
    assert!(state.set_values(hook("new-item-rarity"), &[0.]).unwrap());
    assert!(state.set_values(hook("new-item-rarity"), &[4.]).unwrap());
    assert!(state.is_wanted(xp));
}

#[test]
fn duplicate_bulk_ids_reject_everything_including_unknown_and_bad_arity() {
    let mut state = FeatureIntents::default();
    state.set_wanted(hook("incoming-damage"), true);
    let before = state.clone();
    for duplicate in ["unknown", "currency", "experience-multiplier", "god-mode"] {
        let error = state
            .restore_values([saved("camera-distance", &[-42.]), saved(duplicate, &[9.]), saved(duplicate, &[10.])])
            .unwrap_err();
        assert_eq!(error, FeatureError::DuplicateId(duplicate.to_owned()));
        assert_eq!(state, before);
    }
    assert!(state.restore_values([saved("not-present", &[8.]), saved("experience-multiplier", &[9.])]).is_ok());
    assert_eq!(state, before);
}

#[test]
fn saved_values_carry_only_hooks_with_values_and_leave_switches_to_the_profile() {
    let mut current = FeatureIntents::default();
    let id = hook("enemy-damage");
    current.set_values(id, &[3.125]).unwrap();
    current.set_wanted(id, true);
    let saved = current.saved_values();
    assert!(saved.iter().all(|entry| !entry.values.is_empty()));
    assert!(saved.iter().all(|entry| HookId::parse(&entry.id).is_some()));
    assert!(saved.iter().all(|entry| !["god-mode", "currency", "forge-item"].contains(&entry.id.as_str())));
    let restored = FeatureIntents::from_saved(saved.clone()).unwrap();
    assert_eq!(restored.values(id), [3.125]);
    assert_eq!(restored.wanted().count(), 0, "a profile saves its switches itself");
    // Saved data is owned; editing it cannot mutate the source.
    let mut edited = saved;
    edited.iter_mut().find(|entry| entry.id == "enemy-damage").unwrap().values[0] = 8.;
    assert_eq!(current.values(id), [3.125]);
    assert!(current.set_wanted(id, false));
    assert!(!current.set_wanted(id, false), "already off");
    assert_eq!(current.values(id), [3.125]);
}

#[test]
fn hooks_sharing_a_game_method_keep_independent_switches() {
    let mut state = FeatureIntents::default();
    state.set_wanted(hook("incoming-damage"), true);
    state.set_wanted(hook("enemy-damage"), true);
    state.set_wanted(hook("incoming-damage"), false);
    assert!(state.is_wanted(hook("enemy-damage")));
    assert!(HookId::parse("one-hit-kill").is_none());
}
