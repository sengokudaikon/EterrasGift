use eg_game_access::{
    catalog::{AffixDefinition, Availability, DefinitionName, DefinitionNameSource, ItemTypeDefinition, SubTypeDefinition, UniqueDefinition},
    definitions::{ItemLimits, ValueRange},
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    locale::GameLocale,
    process::ProcessIdentity,
};
use eg_trainer::{
    catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    edits::*,
};
use std::{collections::BTreeMap, path::PathBuf};

fn definition(id: u16, kind: i32) -> AffixDefinition {
    AffixDefinition {
        id,
        raw_name: format!("affix{id}"),
        raw_display_name: String::new(),
        game_display_name: Availability::Unavailable { reason: "No formatter".into() },
        kind,
        special: 0,
        class_requirement: 0,
        item_types: Some(vec![10, 25]),
        tier_count: 8,
        standard_effect: 1.0,
        numeric: Availability::Unavailable {
            reason: "No numeric invocation".into(),
        },
    }
}
fn name() -> DefinitionName {
    DefinitionName {
        text: "authored".into(),
        source: DefinitionNameSource::StoredDisplay,
    }
}
fn catalog() -> GameCatalog {
    let identity = CatalogIdentity {
        binary_hash: "a".repeat(64),
        metadata_hash: "b".repeat(64),
        locale: GameLocale {
            language: "en".into(),
            game_version: "1.5".into(),
        },
        extraction_version: EXTRACTION_VERSION,
    };
    let range = |minimum, maximum| ValueRange { minimum, maximum };
    let limits = ItemLimits {
        forging_potential: range(3, 211),
        legendary_potential: range(1, 6),
        weavers_will: range(2, 49),
        weavers_touch: range(4, 77),
        level_requirement: range(0, 137),
    };
    let bases = [10, 25, 26]
        .into_iter()
        .map(|item_type| {
            (
                item_type,
                ItemTypeDefinition {
                    item_type,
                    name: name(),
                    sub_types: [0, 1, 2]
                        .into_iter()
                        .map(|sub_type| SubTypeDefinition {
                            sub_type,
                            name: name(),
                            level_requirement: 1,
                            class_requirement: 0,
                            cannot_drop: false,
                            is_corrupted: false,
                            implicits: Availability::Available(vec![]),
                        })
                        .collect(),
                },
            )
        })
        .collect();
    let mut affixes = BTreeMap::new();
    for (id, kind) in [(1, 0), (2, 1), (3, 0), (4, 1), (65500, 0)] {
        affixes.insert(id, definition(id, kind));
    }
    let uniques = [(911, false, 0, true), (912, true, 0, false), (913, false, 1, false)]
        .into_iter()
        .map(|(id, is_set_item, legendary_type, can_drop_as_legendary)| {
            (
                id,
                UniqueDefinition {
                    id,
                    name: name(),
                    item_type: 10,
                    sub_types: vec![0, 1, 2],
                    is_set_item,
                    set_id: if is_set_item { 42 } else { 0 },
                    legendary_type,
                    can_drop_as_legendary,
                    is_pre_corrupted: false,
                    numeric: Availability::Unavailable {
                        reason: "No tooltip mapping".into(),
                    },
                },
            )
        })
        .collect();
    GameCatalog {
        identity,
        limits,
        bases,
        affixes,
        uniques,
        rarities: BTreeMap::new(),
    }
}
fn item() -> ForgeSnapshot {
    let affix = |slot, id, kind| AffixSnapshot {
        slot,
        id,
        tier: 4,
        roll: 120,
        kind,
        seal: 0,
        name: format!("a{id}"),
        special: 0,
    };
    let mut item = ForgeSnapshot {
        item: 0x12340,
        individual_id: 17,
        fingerprint: 0,
        name: "original Ω".into(),
        item_type: 10,
        sub_type: 0,
        rarity: 2,
        class_requirement: 0,
        affixes: vec![affix(0, 1, 0), affix(1, 2, 1)],
        implicit_rolls: vec![10, 20],
        identity: None,
        status: ItemStatus {
            forging_potential: 100,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 40,
        },
    };
    refresh(&mut item);
    item
}
fn refresh(item: &mut ForgeSnapshot) {
    item.fingerprint = item.legacy_fingerprint()
}
fn process() -> ProcessIdentity {
    ProcessIdentity {
        pid: 123,
        created: 456,
        executable: PathBuf::from("C:/authored/Last Epoch.exe"),
    }
}
fn observation(item: ForgeSnapshot, catalog: &GameCatalog) -> ExpectedObservation {
    ExpectedObservation::new(1, process(), catalog.identity.clone(), item).unwrap()
}
fn provenance() -> CatalogProvenance {
    CatalogProvenance::RuntimeRead {
        generation: 1,
        process: process(),
    }
}
fn prepared(item: ForgeSnapshot, edit: ItemEdit) -> PreparedEdit {
    let c = catalog();
    prepare(&observation(item, &c), &c, &provenance(), edit, true, None).unwrap()
}
fn replacement() -> ItemEdit {
    ItemEdit::Affix(AffixEdit {
        slot: 1,
        id: 4,
        tier: 5,
        roll: 200,
        seal: SealState::Sealed,
        allow_incompatible: false,
    })
}

#[test]
fn confirmed_undo_rebases_only_slot_order_and_names_never_a_changed_saved_value() {
    use eg_trainer::undo::ForgeUndoHistory;
    let catalog = catalog();
    for changed_level in [false, true] {
        let mut history = ForgeUndoHistory::default();
        history.begin_batch();
        let first = prepared(item(), roll(0, 42));
        history.confirmed(&first, 1, first.projected(), false).unwrap();
        let seal = prepared(
            first.projected().clone(),
            ItemEdit::Affix(AffixEdit {
                slot: 1,
                id: 2,
                tier: 4,
                roll: 120,
                seal: SealState::Sealed,
                allow_incompatible: false,
            }),
        );
        history.confirmed(&seal, 1, seal.projected(), false).unwrap();
        let undo = history.current().unwrap();
        let undo_plan = undo.prepare(undo.after(), 1, &catalog, &provenance()).unwrap();
        let undo = prepared(undo.after().snapshot().clone(), undo_plan.edit_intent().clone());
        let mut actual = undo.projected().clone();
        actual.name = "Rebuilt name".into();
        for affix in &mut actual.affixes {
            affix.name = "Refreshed name".into();
        }
        if changed_level {
            actual.status.level_requirement += 1;
        }
        refresh(&mut actual);
        // The ordinary affix readback policy permits a rebuilt level cache.
        assert!(undo.inspect_readback(Some(&actual)).confirmed());
        let result = history.confirmed(&undo, 1, &actual, true);
        if changed_level {
            assert_eq!(result, Err(EditError::StaleItem));
            assert!(history.is_empty());
        } else {
            result.unwrap();
            assert_eq!(history.len(), 1);
            let previous = history.current().unwrap();
            assert_eq!(previous.after().snapshot(), &actual);
            assert!(matches!(previous.reverse_edit(), ItemEdit::Roll(e) if e.changes == [RollChange { index: 1, value: 120 }]));
            let restore = previous.prepare(previous.after(), 1, &catalog, &provenance()).unwrap();
            assert_eq!(restore.projected().affixes[1].id, 1);
            assert_eq!(restore.projected().affixes[1].roll, 120);
        }
    }
}
fn roll(index: i32, value: i32) -> ItemEdit {
    ItemEdit::Roll(RollEdit {
        target: RollTarget::Affix,
        changes: vec![RollChange { index, value }],
        allow_incompatible: false,
    })
}
fn status(field: StatusField, value: i32, unrestricted: bool) -> ItemEdit {
    ItemEdit::Status(StatusEdit { field, value, unrestricted })
}
fn unique(mut i: ForgeSnapshot, rarity: u8, id: u16) -> ForgeSnapshot {
    i.rarity = rarity;
    i.identity = Some(UniqueIdentity { id, rolls: vec![10, 20, 30] });
    if rarity == 8 {
        i.affixes.clear()
    }
    refresh(&mut i);
    i
}

#[test]
fn construction_requires_structural_snapshot_not_just_a_pointer() {
    let c = catalog();
    let mut i = item();
    i.fingerprint ^= 1;
    assert_eq!(ExpectedObservation::new(1, process(), c.identity.clone(), i), Err(EditError::InvalidObservation));
    assert_eq!(ExpectedObservation::new(0, process(), c.identity.clone(), item()), Err(EditError::InvalidObservation));
    let mut p = process();
    p.created = 0;
    assert_eq!(ExpectedObservation::new(1, p, c.identity, item()), Err(EditError::InvalidObservation));
}
#[test]
fn cached_definitions_cannot_prepare_a_write_and_catalog_identity_is_pinned() {
    let mut c = catalog();
    let expected = observation(item(), &c);
    assert_eq!(
        prepare(&expected, &c, &CatalogProvenance::CachedOnly, roll(0, 42), true, None),
        Err(EditError::CatalogNotVerified)
    );
    c.identity.locale.language = "de".into();
    assert_eq!(prepare(&expected, &c, &provenance(), roll(0, 42), true, None), Err(EditError::IdentityChanged));
}
#[test]
fn full_expected_state_detects_names_categories_and_set_identity_outside_legacy_fnv() {
    let c = catalog();
    let before = unique(item(), 8, 912);
    let expected = observation(before.clone(), &c);
    let p = prepare(&expected, &c, &provenance(), status(StatusField::Corrupted, 1, false), false, None).unwrap();
    for field in 0..4 {
        let mut changed = before.clone();
        match field {
            0 => changed.name.push('x'),
            1 => changed.identity.as_mut().unwrap().id = 911,
            2 => changed.identity.as_mut().unwrap().rolls[0] = 99,
            _ => changed.class_requirement = 3,
        }
        assert_eq!(before.fingerprint, changed.legacy_fingerprint());
        refresh(&mut changed);
        assert_eq!(p.recheck(&observation(changed, &c), true, false), Err(EditError::StaleItem));
    }
    let before = item();
    let mut changed = before.clone();
    changed.affixes[0].special = 5;
    changed.affixes[0].name.push('x');
    assert_eq!(before.fingerprint, changed.legacy_fingerprint());
    assert!(!observation(before, &c).matches(&observation(changed, &c)));
}
#[test]
fn submission_identity_binds_generation_creation_time_and_catalog_even_for_equal_pointers() {
    let c = catalog();
    let before = item();
    let expected = observation(before.clone(), &c);
    let p = prepare(&expected, &c, &provenance(), roll(0, 42), false, None).unwrap();
    let other = ExpectedObservation::new(2, process(), c.identity.clone(), before.clone()).unwrap();
    assert_eq!(p.recheck(&other, true, false), Err(EditError::IdentityChanged));
    let mut pid_reuse = process();
    pid_reuse.created += 1;
    let other = ExpectedObservation::new(1, pid_reuse, c.identity.clone(), before).unwrap();
    assert_eq!(p.recheck(&other, true, false), Err(EditError::IdentityChanged));
    assert_eq!(p.recheck(&expected, false, false), Err(EditError::OfflineRequired));
    assert_eq!(p.recheck(&expected, true, false), Ok(()));
}
#[test]
fn seal_moves_are_stable_and_later_drafts_follow_their_original_affix() {
    let before = item();
    let p = prepared(before.clone(), replacement());
    assert_eq!(p.projected().affixes.iter().map(|a| (a.slot, a.id, a.seal)).collect::<Vec<_>>(), [(0, 4, 1), (1, 1, 0)]);
    assert_eq!(p.moves(), &BTreeMap::from([(0, 1), (1, 0)]));
    assert_eq!(roll(0, 99).follow(p.moves()), roll(1, 99));
    assert_eq!(before, item());
    assert_eq!(p.projected().fingerprint, p.projected().legacy_fingerprint());
}
#[test]
fn replacing_affix_requires_save_id_display_tier_valid_roll_and_current_definition() {
    let c = catalog();
    let expected = observation(item(), &c);
    for (id, tier, value, error) in [
        (4096, 1, 0, EditError::InvalidValues),
        (65500, 1, 0, EditError::InvalidValues),
        (1, 0, 0, EditError::InvalidValues),
        (1, 9, 0, EditError::InvalidValues),
        (1, 1, 256, EditError::InvalidValues),
        (999, 1, 0, EditError::MissingDefinition),
    ] {
        let e = ItemEdit::Affix(AffixEdit {
            slot: 0,
            id,
            tier,
            roll: value,
            seal: SealState::Normal,
            allow_incompatible: true,
        });
        assert_eq!(prepare(&expected, &c, &provenance(), e, true, None), Err(error));
    }
    let mut c = catalog();
    c.affixes.get_mut(&1).unwrap().tier_count = 3;
    assert_eq!(prepare(&expected, &c, &provenance(), roll(0, 42), true, None), Err(EditError::TierUnavailable));
}
#[test]
fn high_readable_ids_are_read_only_when_rebuild_would_truncate_twelve_bit_serialization() {
    let c = catalog();
    let mut i = item();
    i.affixes[0].id = 65500;
    refresh(&mut i);
    let expected = observation(i.clone(), &c);
    assert_eq!(
        prepare(&expected, &c, &provenance(), roll(0, 42), false, None),
        Err(EditError::AffixIdNotSerializable(65500))
    );
    assert_eq!(
        prepare(&expected, &c, &provenance(), status(StatusField::Corrupted, 1, false), false, None),
        Err(EditError::AffixIdNotSerializable(65500))
    );
    // The source's level edit is a direct Int32 write and does not rebuild ID.
    assert!(prepare(&expected, &c, &provenance(), status(StatusField::LevelRequirement, 80, false), false, None).is_ok());
    assert!(prepare(&expected, &c, &provenance(), ItemEdit::RemoveAffix(RemoveAffixEdit { slot: 0, id: 65500 }), false, None).is_ok());
}
#[test]
fn new_layout_problems_are_rejected_but_existing_problems_stay_editable() {
    let c = catalog();
    let before = item();
    let expected = observation(before.clone(), &c);
    let normal_eight = ItemEdit::Affix(AffixEdit {
        slot: 0,
        id: 1,
        tier: 8,
        roll: 0,
        seal: SealState::Normal,
        allow_incompatible: false,
    });
    assert_eq!(
        prepare(&expected, &c, &provenance(), normal_eight, false, None),
        Err(EditError::NewLayoutProblem(LayoutProblem::NormalTierEight))
    );
    let mut before = before;
    before.affixes[0].tier = 8;
    refresh(&mut before);
    assert!(prepare(&observation(before, &c), &c, &provenance(), roll(0, 0), false, None).is_ok());
    let before = unique(item(), 9, 911);
    let expected = observation(before, &c);
    let corrupted = ItemEdit::Affix(AffixEdit {
        slot: 0,
        id: 1,
        tier: 4,
        roll: 0,
        seal: SealState::Corrupted,
        allow_incompatible: false,
    });
    assert_eq!(
        prepare(&expected, &c, &provenance(), corrupted, false, None),
        Err(EditError::NewLayoutProblem(LayoutProblem::LegendaryUncorrupted))
    );
}
#[test]
fn existing_unique_capacity_problem_never_escapes_hard_wire_bound_32() {
    let c = catalog();
    let mut before = unique(item(), 9, 911);
    before.affixes = (0..32)
        .map(|slot| AffixSnapshot {
            slot,
            id: 1,
            tier: 1,
            roll: 0,
            kind: 0,
            seal: 0,
            name: String::new(),
            special: 0,
        })
        .collect();
    refresh(&mut before);
    let add = ItemEdit::AddAffix(AddAffixEdit {
        id: 3,
        tier: 1,
        roll: 0,
        seal: SealState::Normal,
        kind: AffixKind::Prefix,
        category: Some(0),
        allow_incompatible: true,
    });
    assert_eq!(
        prepare(&observation(before, &c), &c, &provenance(), add, true, None),
        Err(EditError::ProjectedSnapshotOutOfBounds)
    );
}
#[test]
fn add_remove_compute_rarity_and_refuse_removing_shifted_id() {
    let before = item();
    let add = ItemEdit::AddAffix(AddAffixEdit {
        id: 3,
        tier: 8,
        roll: 255,
        seal: SealState::Primordial,
        kind: AffixKind::Prefix,
        category: None,
        allow_incompatible: false,
    });
    let p = prepared(before.clone(), add);
    assert_eq!(p.projected().rarity, 2);
    assert_eq!(p.projected().affixes.len(), 3);
    let p = prepared(before.clone(), ItemEdit::RemoveAffix(RemoveAffixEdit { slot: 0, id: 1 }));
    assert_eq!(p.projected().rarity, 1);
    assert_eq!(p.projected().affixes[0].slot, 0);
    let c = catalog();
    assert_eq!(
        prepare(
            &observation(before, &c),
            &c,
            &provenance(),
            ItemEdit::RemoveAffix(RemoveAffixEdit { slot: 0, id: 2 }),
            false,
            None
        ),
        Err(EditError::InvalidValues)
    );
}
#[test]
fn override_is_two_distinct_inputs_and_unknown_definition_never_bypasses() {
    let c = catalog();
    let expected = observation(item(), &c);
    let mut e = AffixEdit {
        slot: 0,
        id: 2,
        tier: 4,
        roll: 50,
        seal: SealState::Normal,
        allow_incompatible: true,
    };
    assert_eq!(
        prepare(&expected, &c, &provenance(), ItemEdit::Affix(e.clone()), false, None),
        Err(EditError::OverrideRequired)
    );
    assert!(prepare(&expected, &c, &provenance(), ItemEdit::Affix(e.clone()), true, None).is_ok());
    e.allow_incompatible = false;
    assert_eq!(
        prepare(&expected, &c, &provenance(), ItemEdit::Affix(e.clone()), true, None),
        Err(EditError::Incompatible(CompatibilityReason::SlotType))
    );
    e.allow_incompatible = true;
    e.id = 999;
    assert_eq!(prepare(&expected, &c, &provenance(), ItemEdit::Affix(e), true, None), Err(EditError::MissingDefinition));
}
#[test]
fn compatibility_preserves_class_category_equipment_duplicate_and_omen_rules() {
    let c = catalog();
    let mut i = item();
    let slot = i.affixes[0].clone();
    let mut d = definition(3, 0);
    d.class_requirement = 4;
    i.class_requirement = 1;
    assert_eq!(compatibility(&c, &i, &d, Some(&slot), Some(1), false).reason, CompatibilityReason::ClassRequirement);
    d.class_requirement = 0;
    d.special = 5;
    assert_eq!(compatibility(&c, &i, &d, Some(&slot), None, false).reason, CompatibilityReason::SpecialCategory);
    i.item_type = 25;
    i.sub_type = 2;
    assert!(compatibility(&c, &i, &d, Some(&slot), None, false).allowed);
    i.item_type = 29;
    i.sub_type = 10;
    d.special = 0;
    d.item_types = Some(vec![31]);
    assert!(compatibility(&c, &i, &d, Some(&slot), None, false).allowed);
    i.sub_type = 9;
    assert_eq!(compatibility(&c, &i, &d, Some(&slot), None, false).reason, CompatibilityReason::EquipmentType);
    d.item_types = None;
    assert_eq!(compatibility(&c, &i, &d, Some(&slot), None, true).reason, CompatibilityReason::MissingMetadata);
}
#[test]
fn roll_changes_are_bounded_distinct_and_unique_or_implicit_require_full_bound_evidence() {
    let c = catalog();
    let expected = observation(item(), &c);
    for changes in [
        vec![],
        vec![RollChange { index: 0, value: 1 }; 2],
        vec![RollChange { index: 0, value: -1 }],
        (0..33).map(|index| RollChange { index, value: 0 }).collect(),
    ] {
        assert_eq!(
            prepare(
                &expected,
                &c,
                &provenance(),
                ItemEdit::Roll(RollEdit {
                    target: RollTarget::Affix,
                    changes,
                    allow_incompatible: false
                }),
                false,
                None
            ),
            Err(EditError::InvalidValues)
        );
    }
    let implicit = ItemEdit::Roll(RollEdit {
        target: RollTarget::Implicit,
        changes: vec![RollChange { index: 0, value: 255 }],
        allow_incompatible: false,
    });
    assert_eq!(prepare(&expected, &c, &provenance(), implicit.clone(), false, None), Err(EditError::MissingRollEvidence));
    let absent = || Availability::Unavailable {
        reason: "No audited tooltip".into(),
    };
    let valid = RollEvidence::from_runtime(
        &expected,
        absent(),
        Availability::Available(vec![RollBinding {
            editable: true,
            roll_ids: vec![0],
        }]),
    );
    let p = prepare(&expected, &c, &provenance(), implicit.clone(), false, Some(&valid)).unwrap();
    assert_eq!(p.projected().implicit_rolls, [255, 20]);
    let invalid = RollEvidence::from_runtime(
        &expected,
        absent(),
        Availability::Available(vec![RollBinding {
            editable: true,
            roll_ids: vec![0, 1],
        }]),
    );
    assert_eq!(
        prepare(&expected, &c, &provenance(), implicit.clone(), false, Some(&invalid)),
        Err(EditError::InvalidRollMapping)
    );
    let mut different = item();
    different.name.push('x');
    let stale = RollEvidence::from_runtime(
        &observation(different, &c),
        absent(),
        Availability::Available(vec![RollBinding {
            editable: true,
            roll_ids: vec![0],
        }]),
    );
    assert_eq!(prepare(&expected, &c, &provenance(), implicit, false, Some(&stale)), Err(EditError::MissingRollEvidence));
}
#[test]
fn unique_shared_rolls_need_every_mapping_in_range_and_set_rolls_stay_noneditable() {
    let c = catalog();
    let edit = ItemEdit::Roll(RollEdit {
        target: RollTarget::Unique,
        changes: vec![RollChange { index: 1, value: 99 }],
        allow_incompatible: false,
    });
    for (rarity, ids, ok) in [(7, vec![0, 1], true), (9, vec![1, 2], true), (7, vec![1, 3], false), (8, vec![1], false)] {
        let expected = observation(unique(item(), rarity, 911), &c);
        let evidence = RollEvidence::from_runtime(
            &expected,
            Availability::Available(vec![RollBinding { editable: true, roll_ids: ids }]),
            Availability::Available(vec![]),
        );
        let result = prepare(&expected, &c, &provenance(), edit.clone(), false, Some(&evidence));
        assert_eq!(result.is_ok(), ok);
    }
}
#[test]
fn status_uses_game_limits_and_native_unique_rules_instead_of_ui_byte_limits() {
    let c = catalog();
    let expected = observation(item(), &c);
    for (field, value, ok) in [
        (StatusField::ForgingPotential, 2, false),
        (StatusField::ForgingPotential, 211, true),
        (StatusField::LevelRequirement, 137, true),
        (StatusField::LevelRequirement, 138, false),
        (StatusField::Corrupted, 2, false),
    ] {
        assert_eq!(prepare(&expected, &c, &provenance(), status(field, value, false), false, None).is_ok(), ok);
    }
    let expected = observation(unique(item(), 7, 911), &c);
    assert!(prepare(&expected, &c, &provenance(), status(StatusField::LegendaryPotential, 6, false), false, None).is_ok());
    assert_eq!(
        prepare(&expected, &c, &provenance(), status(StatusField::WeaversWill, 10, false), false, None),
        Err(EditError::InvalidStatus)
    );
    assert!(prepare(&expected, &c, &provenance(), status(StatusField::WeaversWill, 10, true), true, None).is_ok());
    let mut i = unique(item(), 9, 911);
    i.status.legendary_potential = 1;
    refresh(&mut i);
    let expected = observation(i, &c);
    assert_eq!(
        prepare(&expected, &c, &provenance(), status(StatusField::WeaversWill, 10, true), true, None),
        Err(EditError::InvalidStatus)
    );
    let mut i = unique(item(), 9, 911);
    i.status.weavers_will = 80;
    i.status.legendary_potential = 1;
    refresh(&mut i);
    let expected = observation(i, &c);
    assert!(prepare(&expected, &c, &provenance(), status(StatusField::WeaversWill, 80, true), true, None).is_ok());
    assert!(prepare(&expected, &c, &provenance(), status(StatusField::WeaversWill, 81, true), true, None).is_err());
}
#[test]
fn idol_type_changes_need_override_and_identity_changes_follow_game_rarity() {
    let c = catalog();
    let expected = observation(item(), &c);
    assert_eq!(
        prepare(
            &expected,
            &c,
            &provenance(),
            ItemEdit::BaseType(BaseTypeEdit {
                sub_type: 1,
                allow_incompatible: true,
                item_type: Some(25)
            }),
            true,
            None
        ),
        Err(EditError::InvalidBase)
    );
    let mut i = item();
    i.item_type = 25;
    refresh(&mut i);
    let expected = observation(i, &c);
    let edit = ItemEdit::BaseType(BaseTypeEdit {
        sub_type: 1,
        allow_incompatible: false,
        item_type: Some(26),
    });
    assert_eq!(prepare(&expected, &c, &provenance(), edit.clone(), false, None), Err(EditError::OverrideRequired));
    assert!(prepare(&expected, &c, &provenance(), edit, true, None).is_ok());
    let expected = observation(unique(item(), 7, 911), &c);
    let set = ItemEdit::UniqueIdentity(UniqueIdentityEdit {
        unique_id: 912,
        sub_type: 1,
        rarity: 8,
        unrestricted: true,
    });
    assert_eq!(prepare(&expected, &c, &provenance(), set, true, None), Err(EditError::InvalidUniqueIdentity));
    let p = prepare(
        &expected,
        &c,
        &provenance(),
        ItemEdit::UniqueIdentity(UniqueIdentityEdit {
            unique_id: 913,
            sub_type: 1,
            rarity: 9,
            unrestricted: true,
        }),
        true,
        None,
    )
    .unwrap();
    assert_eq!(p.projected().identity.as_ref().unwrap().rolls, [10, 20, 30]);
    assert_eq!(p.projected().rarity, 9);
}
#[test]
fn readback_ignores_only_source_owned_name_level_and_class_recomputations() {
    let p = prepared(item(), roll(0, 42));
    let mut after = p.projected().clone();
    after.name = "new name".into();
    after.affixes[0].name = "new affix".into();
    after.status.level_requirement = 99;
    refresh(&mut after);
    assert!(p.inspect_readback(Some(&after)).confirmed());
    after.status.forging_potential -= 1;
    refresh(&mut after);
    assert!(!p.inspect_readback(Some(&after)).confirmed());
    let p = prepared(item(), status(StatusField::LevelRequirement, 80, false));
    let mut after = p.projected().clone();
    after.status.level_requirement = 79;
    refresh(&mut after);
    assert!(!p.inspect_readback(Some(&after)).confirmed());
    let p = prepared(
        item(),
        ItemEdit::BaseType(BaseTypeEdit {
            sub_type: 1,
            allow_incompatible: false,
            item_type: None,
        }),
    );
    let mut after = p.projected().clone();
    after.class_requirement = 8;
    after.status.level_requirement = 99;
    refresh(&mut after);
    assert!(p.inspect_readback(Some(&after)).confirmed());
    assert!(!p.inspect_readback(None).confirmed());
}
#[test]
fn strict_affix_verifies_collateral_fields_but_override_requires_submitted_moved_slot() {
    let p = prepared(item(), replacement());
    let mut after = p.projected().clone();
    after.affixes[1].roll = 1;
    refresh(&mut after);
    assert!(!p.inspect_readback(Some(&after)).confirmed());
    let mut edit = match replacement() {
        ItemEdit::Affix(e) => e,
        _ => unreachable!(),
    };
    edit.allow_incompatible = true;
    let p = prepared(item(), ItemEdit::Affix(edit));
    let mut after = p.projected().clone();
    after.status.ruined = true;
    after.affixes[1].roll = 1;
    refresh(&mut after);
    assert!(p.inspect_readback(Some(&after)).confirmed());
    // A match in another slot is diagnostic; it cannot prove this submitted slot.
    after.affixes.swap(0, 1);
    for (slot, a) in after.affixes.iter_mut().enumerate() {
        a.slot = slot as u8
    }
    refresh(&mut after);
    let diagnostics = p.inspect_readback(Some(&after));
    assert!(!diagnostics.confirmed());
    assert_eq!(diagnostics.matching_slots, [1]);
}
#[test]
fn added_unknown_category_tolerance_never_hides_category_change_on_an_existing_affix() {
    let p = prepared(
        item(),
        ItemEdit::AddAffix(AddAffixEdit {
            id: 3,
            tier: 1,
            roll: 0,
            seal: SealState::Normal,
            kind: AffixKind::Prefix,
            category: None,
            allow_incompatible: false,
        }),
    );
    let mut after = p.projected().clone();
    after.affixes[2].special = 5;
    after.class_requirement = 8;
    refresh(&mut after);
    assert!(p.inspect_readback(Some(&after)).confirmed());
    after.affixes[0].special = 6;
    refresh(&mut after);
    assert!(!p.inspect_readback(Some(&after)).confirmed());
}
#[test]
fn strict_affix_readback_and_preflight_both_include_set_identity() {
    let c = catalog();
    let mut i = unique(item(), 8, 912);
    i.affixes.push(AffixSnapshot {
        slot: 0,
        id: 1,
        tier: 1,
        roll: 0,
        kind: 0,
        seal: 0,
        name: String::new(),
        special: 0,
    });
    refresh(&mut i);
    let p = prepare(
        &observation(i, &c),
        &c,
        &provenance(),
        ItemEdit::Affix(AffixEdit {
            slot: 0,
            id: 3,
            tier: 1,
            roll: 5,
            seal: SealState::Normal,
            allow_incompatible: false,
        }),
        false,
        None,
    )
    .unwrap();
    let mut after = p.projected().clone();
    after.identity.as_mut().unwrap().id = 911;
    refresh(&mut after);
    assert!(!p.inspect_readback(Some(&after)).confirmed());
    assert_eq!(p.recheck(&observation(after, &c), true, false), Err(EditError::StaleItem));
}

#[test]
fn forge_base_and_identity_plans_preserve_conditional_reverse_rules() {
    use eg_trainer::{
        undo::ForgeUndo,
        writes::{ForgePlan, WriteRecord, WriteStatus},
    };
    let c = catalog();
    let mut plain = item();
    plain.affixes.clear();
    refresh(&mut plain);
    let mut idol = plain.clone();
    idol.item_type = 25;
    refresh(&mut idol);
    let mut unique_item = unique(plain.clone(), 7, 911);
    refresh(&mut unique_item);
    for (before, edit, override_enabled) in [
        (
            plain,
            ItemEdit::BaseType(BaseTypeEdit {
                sub_type: 1,
                item_type: None,
                allow_incompatible: false,
            }),
            false,
        ),
        (
            idol,
            ItemEdit::BaseType(BaseTypeEdit {
                sub_type: 2,
                item_type: Some(26),
                allow_incompatible: false,
            }),
            true,
        ),
        (
            unique_item,
            ItemEdit::UniqueIdentity(UniqueIdentityEdit {
                unique_id: 912,
                sub_type: 1,
                rarity: 8,
                unrestricted: false,
            }),
            false,
        ),
    ] {
        let expected = observation(before.clone(), &c);
        let plan = ForgePlan::edit(&expected, 17, &c, &provenance(), edit, override_enabled).unwrap();
        let mut after = plan.projected().clone();
        after.name = "new game name".into();
        after.class_requirement = 6;
        after.status.level_requirement = 86;
        if before.item_type == 25 {
            after.implicit_rolls = vec![7];
        }
        refresh(&mut after);
        assert!(plan.matches(&after));
        let undo = ForgeUndo::from_confirmed(&WriteRecord {
            plan: plan.clone(),
            status: WriteStatus::Confirmed(after.clone()),
        })
        .unwrap();
        let current = observation(after.clone(), &c);
        let reverse = undo.prepare_with_override(&current, 17, &c, &provenance(), override_enabled).unwrap();
        assert_eq!(reverse.projected().item_type, before.item_type);
        assert_eq!(reverse.projected().sub_type, before.sub_type);
        assert_eq!(reverse.projected().rarity, before.rarity);
        assert_eq!(reverse.projected().identity, before.identity);
        if override_enabled {
            assert_eq!(plan.recheck(&expected, 17, false), Err(EditError::OverrideRequired));
            assert_eq!(undo.prepare_with_override(&current, 17, &c, &provenance(), false), Err(EditError::OverrideRequired));
        }
        after.status.forging_potential += 1;
        refresh(&mut after);
        assert!(!plan.matches(&after));
        assert_eq!(
            undo.prepare_with_override(&observation(after, &c), 17, &c, &provenance(), override_enabled),
            Err(EditError::StaleItem)
        );
    }
}
#[test]
fn forge_plans_refuse_unimplemented_edits_and_set_identity_changes_are_stale() {
    use eg_trainer::writes::ForgePlan;
    let c = catalog();
    let expected = observation(item(), &c);
    assert!(ForgePlan::edit(&expected, 17, &c, &provenance(), replacement(), true).is_ok());
    let mut set = unique(item(), 8, 912);
    refresh(&mut set);
    let expected = observation(set.clone(), &c);
    let plan = ForgePlan::edit(
        &expected,
        17,
        &c,
        &provenance(),
        ItemEdit::UniqueIdentity(UniqueIdentityEdit {
            unique_id: 911,
            sub_type: 1,
            rarity: 7,
            unrestricted: false,
        }),
        false,
    )
    .unwrap();
    let fingerprint = set.fingerprint;
    set.identity.as_mut().unwrap().id = 913;
    refresh(&mut set);
    assert_eq!(set.fingerprint, fingerprint); // Original legacy fingerprint omits set identities.
    assert_eq!(plan.recheck(&observation(set, &c), 17, false), Err(EditError::StaleItem));
}

#[test]
fn forge_affix_roll_batch_and_reverse_keep_unselected_state_and_reject_stale_targets() {
    use eg_game_access::write::ForgeMutation;
    use eg_trainer::{
        undo::ForgeUndo,
        writes::{ForgePlan, WriteRecord, WriteStatus},
    };
    let c = catalog();
    let before = item();
    let expected = observation(before.clone(), &c);
    let edit = ItemEdit::Roll(RollEdit {
        target: RollTarget::Affix,
        changes: vec![RollChange { index: 1, value: 255 }, RollChange { index: 0, value: 0 }],
        allow_incompatible: false,
    });
    let plan = ForgePlan::edit(&expected, 17, &c, &provenance(), edit.clone(), false).unwrap();
    let ForgeMutation::AffixRolls { rolls, .. } = plan.mutation() else {
        panic!("affix rolls required")
    };
    assert_eq!(rolls.changes().iter().map(|c| (c.slot, c.value)).collect::<Vec<_>>(), vec![(1, 255), (0, 0)]);
    let mut after = plan.projected().clone();
    after.status.level_requirement = 86;
    after.name = "derived name".into();
    refresh(&mut after);
    assert!(plan.matches(&after));
    let undo = ForgeUndo::from_confirmed(&WriteRecord {
        plan: plan.clone(),
        status: WriteStatus::Confirmed(after.clone()),
    })
    .unwrap();
    let reverse = undo.prepare(&observation(after.clone(), &c), 17, &c, &provenance()).unwrap();
    assert_eq!(reverse.projected().affixes, before.affixes);
    after.affixes[0].roll = 13;
    refresh(&mut after);
    assert!(!plan.matches(&after));
    assert_eq!(undo.prepare(&observation(after, &c), 17, &c, &provenance()), Err(EditError::StaleItem));
    for target in [RollTarget::Unique, RollTarget::Implicit] {
        let ItemEdit::Roll(mut edit) = edit.clone() else { unreachable!() };
        edit.target = target;
        assert_eq!(
            ForgePlan::edit(&expected, 17, &c, &provenance(), ItemEdit::Roll(edit), false),
            Err(EditError::InvalidValues)
        );
    }
    let bad = ItemEdit::Roll(RollEdit {
        target: RollTarget::Affix,
        changes: vec![RollChange { index: 0, value: 1 }, RollChange { index: 2, value: 1 }],
        allow_incompatible: false,
    });
    assert_eq!(ForgePlan::edit(&expected, 17, &c, &provenance(), bad, false), Err(EditError::InvalidValues));
}

#[test]
fn level_cache_readback_uses_the_same_exact_rule_in_pure_and_submittable_plans() {
    use eg_trainer::{edits::ReadbackPolicy, writes::ForgePlan};
    let c = catalog();
    let expected = observation(item(), &c);
    let intent = status(StatusField::LevelRequirement, 80, false);
    let pure = prepare(&expected, &c, &provenance(), intent.clone(), false, None).unwrap();
    let plan = ForgePlan::edit(&expected, 17, &c, &provenance(), intent.clone(), false).unwrap();
    assert!(plan.matches(pure.projected()));
    assert_eq!(pure.inspect_readback(Some(pure.projected())).policy, ReadbackPolicy::LevelRequirement);
    for fault in 0..4 {
        let mut actual = pure.projected().clone();
        match fault {
            0 => actual.name.push_str(" changed"),
            1 => actual.affixes[0].name.push_str(" changed"),
            2 => actual.fingerprint ^= 1,
            _ => actual.status.level_requirement -= 1,
        }
        assert!(!pure.inspect_readback(Some(&actual)).confirmed());
        assert!(!plan.matches(&actual));
    }
    assert_eq!(ForgePlan::edit(&expected, 0, &c, &provenance(), intent, false), Err(EditError::InvalidObservation));
}

#[test]
fn inserted_sealed_affix_is_undone_at_its_new_slot_even_when_id_already_exists() {
    use eg_trainer::{
        undo::ForgeUndo,
        writes::{ForgePlan, WriteRecord, WriteStatus},
    };
    let c = catalog();
    let before = item();
    let expected = observation(before.clone(), &c);
    let edit = ItemEdit::AddAffix(AddAffixEdit {
        id: 1,
        tier: 2,
        roll: 239,
        seal: SealState::Sealed,
        kind: AffixKind::Prefix,
        category: Some(0),
        allow_incompatible: true,
    });
    let plan = ForgePlan::edit(&expected, 17, &c, &provenance(), edit, true).unwrap();
    assert_eq!((plan.projected().affixes[0].id, plan.projected().affixes[0].seal), (1, 1));
    let token = ForgeUndo::from_confirmed(&WriteRecord {
        status: WriteStatus::Confirmed(plan.projected().clone()),
        plan,
    })
    .unwrap();
    assert!(matches!(token.reverse_edit(),ItemEdit::RemoveAffix(e) if e.slot==0 && e.id==1));
    let reverse = token.prepare(token.after(), 17, &c, &provenance()).unwrap();
    assert_eq!(reverse.projected().affixes, before.affixes);
    let mut changed = token.after().snapshot().clone();
    changed.affixes[0].roll += 1;
    refresh(&mut changed);
    assert_eq!(token.prepare(&observation(changed, &c), 17, &c, &provenance()), Err(EditError::StaleItem));
}
#[test]
fn removed_affix_restore_uses_current_catalog_and_can_require_override() {
    use eg_trainer::{
        undo::ForgeUndo,
        writes::{ForgePlan, WriteRecord, WriteStatus},
    };
    let mut c = catalog();
    let expected = observation(item(), &c);
    let plan = ForgePlan::edit(&expected, 17, &c, &provenance(), ItemEdit::RemoveAffix(RemoveAffixEdit { slot: 0, id: 1 }), false).unwrap();
    let token = ForgeUndo::from_confirmed(&WriteRecord {
        status: WriteStatus::Confirmed(plan.projected().clone()),
        plan,
    })
    .unwrap();
    assert!(token.prepare(token.after(), 17, &c, &provenance()).is_ok());
    c.affixes.get_mut(&1).unwrap().special = 1;
    assert_eq!(token.prepare(token.after(), 17, &c, &provenance()), Err(EditError::OverrideRequired));
    assert!(token.prepare_with_override(token.after(), 17, &c, &provenance(), true).unwrap().unrestricted());
}
#[test]
fn removing_the_only_unserializable_id_can_repair_an_item_but_cannot_offer_undo() {
    use eg_trainer::{
        undo::ForgeUndo,
        writes::{ForgePlan, WriteRecord, WriteStatus},
    };
    let c = catalog();
    let mut before = item();
    before.affixes[0].id = 65500;
    refresh(&mut before);
    let plan = ForgePlan::edit(
        &observation(before, &c),
        17,
        &c,
        &provenance(),
        ItemEdit::RemoveAffix(RemoveAffixEdit { slot: 0, id: 65500 }),
        false,
    )
    .unwrap();
    assert!(plan.projected().affixes.iter().all(|a| a.id <= 4095));
    assert_eq!(
        ForgeUndo::from_confirmed(&WriteRecord {
            status: WriteStatus::Confirmed(plan.projected().clone()),
            plan
        }),
        Err(EditError::InvalidValues)
    );
}
