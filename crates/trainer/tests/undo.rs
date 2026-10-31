use eg_trainer::{
    definitions::catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    item::edit::{EditError, ExpectedObservation, ItemEdit, StatusEdit, StatusField},
    item::undo::ForgeUndo,
    write::{ForgePlan, WriteRecord, WriteStatus},
};
use eg_trainer_wire::{
    definitions::{ItemLimits, ValueRange},
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    locale::GameLocale,
    process::ProcessIdentity,
    protocol::ResponseCode,
};
use std::collections::BTreeMap;

const MANAGER: u64 = 0x7000;
const GENERATION: u64 = 41;

fn catalog() -> GameCatalog {
    GameCatalog {
        identity: CatalogIdentity {
            binary_hash: "a".repeat(64),
            metadata_hash: "b".repeat(64),
            locale: GameLocale {
                language: "sr-Latn-RS".into(),
                game_version: "authored".into(),
            },
            extraction_version: EXTRACTION_VERSION,
        },
        limits: ItemLimits {
            forging_potential: ValueRange { minimum: 3, maximum: 211 },
            legendary_potential: ValueRange { minimum: 1, maximum: 6 },
            weavers_will: ValueRange { minimum: 2, maximum: 49 },
            weavers_touch: ValueRange { minimum: 4, maximum: 77 },
            level_requirement: ValueRange { minimum: 0, maximum: 137 },
        },
        bases: BTreeMap::new(),
        affixes: BTreeMap::new(),
        uniques: BTreeMap::new(),
        rarities: BTreeMap::new(),
    }
}

fn process() -> ProcessIdentity {
    ProcessIdentity {
        pid: 19,
        created: 23,
        executable: std::env::temp_dir().join("AuthoredPlayer.exe"),
    }
}

fn snapshot(level: i32) -> ForgeSnapshot {
    let mut item = ForgeSnapshot {
        item: 0x9010,
        individual_id: 17,
        fingerprint: 0,
        name: "After Ω 🌿".into(),
        item_type: 17,
        sub_type: 513,
        rarity: 4,
        class_requirement: 5,
        // Read-only high save ID must not accidentally prevent the direct level
        // cache operation, which never rebuilds serialized affix IDs.
        affixes: vec![AffixSnapshot {
            slot: 0,
            id: 65500,
            tier: 8,
            roll: 120,
            kind: 1,
            seal: 2,
            name: "Affix Ω".into(),
            special: 3,
        }],
        implicit_rolls: vec![0, 128, 255],
        identity: None,
        status: ItemStatus {
            forging_potential: 237,
            forging_potential_type: 2,
            legendary_potential: 3,
            weavers_will: 41,
            weavers_touch: 9,
            corrupted: true,
            ruined: false,
            level_requirement: level,
        },
    };
    item.fingerprint = item.legacy_fingerprint();
    item
}

fn observation(snapshot: ForgeSnapshot) -> ExpectedObservation {
    ExpectedObservation::new(GENERATION, process(), catalog().identity, snapshot).unwrap()
}

fn record(old: i32) -> WriteRecord {
    let plan = ForgePlan::edit(
        &observation(snapshot(old)),
        MANAGER,
        &catalog(),
        ItemEdit::Status(StatusEdit {
            field: StatusField::LevelRequirement,
            value: 17,
            unrestricted: false,
        }),
        false,
    )
    .unwrap();
    let status = WriteStatus::Confirmed(plan.projected().clone());
    WriteRecord { plan, status }
}

#[test]
fn fresh_exact_after_observation_prepares_only_the_original_level_with_full_identity() {
    let confirmed = record(87);
    let token = ForgeUndo::from_confirmed(&confirmed).unwrap();
    assert_eq!(token.manager(), MANAGER);
    assert!(matches!(token.reverse_edit(), eg_trainer::item::edit::ItemEdit::Status(edit) if edit.value == 87));
    assert_eq!(token.after().snapshot(), confirmed.plan.projected());
    let current = observation(confirmed.plan.projected().clone());
    let plan = token.prepare(&current, MANAGER, &catalog()).unwrap();
    assert_eq!(plan.before(), &current);
    assert_eq!(plan.projected(), confirmed.plan.before().snapshot());
    assert_eq!(plan.projected().status.level_requirement, 87);
    assert_eq!(plan.manager(), MANAGER);
    assert_eq!(token.after().generation(), GENERATION);
    assert_eq!(token.after().process(), &process());
    assert_eq!(token.after().catalog_identity(), &catalog().identity);
}

#[test]
fn only_confirmed_records_can_create_undo_tokens() {
    for status in [
        WriteStatus::Pending,
        WriteStatus::NotKept(record(87).plan.projected().clone()),
        WriteStatus::NotApplied {
            code: ResponseCode::StaleItem,
            message: "Changed".into(),
        },
        WriteStatus::uncertain("lost reply"),
    ] {
        let mut record = record(87);
        record.status = status;
        assert_eq!(ForgeUndo::from_confirmed(&record), Err(EditError::InvalidObservation));
    }
}

#[test]
fn confirmed_label_without_the_exact_projected_payload_is_not_enough() {
    for alter in [
        |s: &mut ForgeSnapshot| s.name.push('x'),
        |s: &mut ForgeSnapshot| s.class_requirement += 1,
        |s: &mut ForgeSnapshot| s.affixes[0].special += 1,
    ] {
        let mut record = record(87);
        let mut wrong = record.plan.projected().clone();
        alter(&mut wrong);
        assert_eq!(wrong.fingerprint, wrong.legacy_fingerprint());
        record.status = WriteStatus::Confirmed(wrong);
        assert_eq!(ForgeUndo::from_confirmed(&record), Err(EditError::InvalidObservation));
    }
}

#[test]
fn same_id_replacement_and_other_item_changes_cannot_be_restored_by_old_token() {
    let token = ForgeUndo::from_confirmed(&record(87)).unwrap();
    for alter in [
        |s: &mut ForgeSnapshot| s.item += 16,
        |s: &mut ForgeSnapshot| s.status.level_requirement += 1,
        |s: &mut ForgeSnapshot| s.affixes[0].roll += 1,
        |s: &mut ForgeSnapshot| s.implicit_rolls[0] += 1,
        |s: &mut ForgeSnapshot| s.status.corrupted = false,
    ] {
        let mut changed = token.after().snapshot().clone();
        alter(&mut changed);
        changed.fingerprint = changed.legacy_fingerprint();
        assert_eq!(token.prepare(&observation(changed), MANAGER, &catalog()), Err(EditError::StaleItem));
    }
}

#[test]
fn manager_and_generation_changes_invalidate_the_undo_identity() {
    let token = ForgeUndo::from_confirmed(&record(87)).unwrap();
    for manager in [0, MANAGER + 16] {
        assert_eq!(token.prepare(token.after(), manager, &catalog()), Err(EditError::IdentityChanged));
    }
    let current = ExpectedObservation::new(GENERATION + 1, process(), catalog().identity, token.after().snapshot().clone()).unwrap();
    assert_eq!(token.prepare(&current, MANAGER, &catalog()), Err(EditError::IdentityChanged));
}

#[test]
fn process_creation_image_or_pid_change_cannot_reuse_undo() {
    let token = ForgeUndo::from_confirmed(&record(87)).unwrap();
    for alter in [
        |p: &mut ProcessIdentity| p.pid += 1,
        |p: &mut ProcessIdentity| p.created += 1,
        |p: &mut ProcessIdentity| p.executable.set_file_name("Other.exe"),
    ] {
        let mut target = process();
        alter(&mut target);
        let current = ExpectedObservation::new(GENERATION, target, catalog().identity, token.after().snapshot().clone()).unwrap();
        assert_eq!(token.prepare(&current, MANAGER, &catalog()), Err(EditError::IdentityChanged));
    }
}

#[test]
fn changed_catalog_identity_cannot_authorize_restore() {
    let token = ForgeUndo::from_confirmed(&record(87)).unwrap();
    for alter in [
        |c: &mut CatalogIdentity| c.binary_hash = "c".repeat(64),
        |c: &mut CatalogIdentity| c.metadata_hash = "c".repeat(64),
        |c: &mut CatalogIdentity| c.locale.language = "fr-FR".into(),
        |c: &mut CatalogIdentity| c.locale.game_version.push('x'),
        |c: &mut CatalogIdentity| c.extraction_version += 1,
    ] {
        let mut changed = catalog();
        alter(&mut changed.identity);
        assert_eq!(token.prepare(token.after(), MANAGER, &changed), Err(EditError::IdentityChanged));
        let current = ExpectedObservation::new(GENERATION, process(), changed.identity.clone(), token.after().snapshot().clone()).unwrap();
        assert_eq!(token.prepare(&current, MANAGER, &changed), Err(EditError::IdentityChanged));
    }
}

#[test]
fn old_level_outside_current_literal_range_is_unavailable_without_override() {
    let token = ForgeUndo::from_confirmed(&record(138)).unwrap();
    assert_eq!(token.prepare(token.after(), MANAGER, &catalog()), Err(EditError::InvalidStatus));
    for old in [-1, 256, 0x01000057] {
        assert_eq!(ForgeUndo::from_confirmed(&record(old)), Err(EditError::InvalidStatus));
    }
    let token = ForgeUndo::from_confirmed(&record(87)).unwrap();
    let mut narrower = catalog();
    narrower.limits.level_requirement.maximum = 86;
    assert_eq!(token.prepare(token.after(), MANAGER, &narrower), Err(EditError::InvalidStatus));
}

#[test]
fn literal_endpoints_can_be_restored_and_the_prior_token_is_stale_after_restore() {
    for old in [0, 137] {
        let confirmed = record(old);
        let token = ForgeUndo::from_confirmed(&confirmed).unwrap();
        let restored = token.prepare(token.after(), MANAGER, &catalog()).unwrap();
        assert_eq!(restored.projected().status.level_requirement, old);
        assert_eq!(restored.projected(), confirmed.plan.before().snapshot());
        let current = observation(restored.projected().clone());
        assert_eq!(token.prepare(&current, MANAGER, &catalog()), Err(EditError::StaleItem));
    }
}

#[test]
fn unique_set_or_legendary_drift_is_stale_and_cannot_create_an_original_level_plan() {
    let token = ForgeUndo::from_confirmed(&record(87)).unwrap();
    for rarity in [7, 8, 9] {
        let mut changed = token.after().snapshot().clone();
        changed.rarity = rarity;
        changed.identity = Some(UniqueIdentity { id: 911, rolls: vec![0, 255] });
        changed.fingerprint = changed.legacy_fingerprint();
        let current = observation(changed);
        assert_eq!(token.prepare(&current, MANAGER, &catalog()), Err(EditError::StaleItem));
        assert_eq!(
            ForgePlan::edit(
                &current,
                MANAGER,
                &catalog(),
                ItemEdit::Status(StatusEdit {
                    field: StatusField::LevelRequirement,
                    value: 87,
                    unrestricted: false
                }),
                false
            ),
            Err(EditError::InvalidStatus)
        );
    }
}

#[test]
fn pure_preparation_failure_preserves_the_owned_token_and_never_changes_the_record() {
    let record = record(87);
    let saved = record.clone();
    let token = ForgeUndo::from_confirmed(&record).unwrap();
    let saved_token = token.clone();
    assert_eq!(token.prepare(token.after(), 0, &catalog()), Err(EditError::IdentityChanged));
    assert_eq!(token, saved_token);
    assert_eq!(record, saved);
    let next = token.prepare(token.after(), MANAGER, &catalog()).unwrap();
    assert_eq!(next.projected().status.level_requirement, 87);
    assert_eq!(record, saved);
}

#[test]
fn affix_undo_follows_the_reordered_slot_and_rechecks_override() {
    use eg_trainer::definitions::catalog::{AffixDefinition, Availability};
    use eg_trainer::item::edit::{AffixEdit, SealState};
    let mut data = catalog();
    for id in [1, 2] {
        data.affixes.insert(
            id,
            AffixDefinition {
                id,
                raw_name: "authored".into(),
                raw_display_name: String::new(),
                game_display_name: Availability::Unavailable { reason: "not formatted".into() },
                kind: 0,
                special: 0,
                class_requirement: 0,
                item_types: Some(vec![17]),
                tier_count: 8,
                standard_effect: 1.0,
                numeric: Availability::Unavailable { reason: "not mapped".into() },
                level_requirement: 0,
                source: None,
            },
        );
    }
    let mut before = snapshot(10);
    before.affixes = (0..2)
        .map(|slot| AffixSnapshot {
            slot,
            id: 1,
            tier: 2,
            roll: 10 + slot,
            kind: 0,
            seal: 0,
            name: String::new(),
            special: 0,
        })
        .collect();
    before.class_requirement = 0;
    before.fingerprint = before.legacy_fingerprint();
    let plan = ForgePlan::edit(
        &observation(before),
        MANAGER,
        &data,
        ItemEdit::Affix(AffixEdit {
            slot: 1,
            id: 2,
            tier: 3,
            roll: 200,
            seal: SealState::Sealed,
            allow_incompatible: true,
        }),
        true,
    )
    .unwrap();
    assert_eq!(plan.moves()[&1], 0);
    let token = ForgeUndo::from_confirmed(&WriteRecord {
        status: WriteStatus::Confirmed(plan.projected().clone()),
        plan,
    })
    .unwrap();
    assert!(matches!(token.reverse_edit(), ItemEdit::Affix(e) if e.slot==0 && e.id==1 && e.tier==2 && e.roll==11 && e.seal==SealState::Normal));
    assert_eq!(token.prepare(token.after(), MANAGER, &data), Err(EditError::OverrideRequired));
    let reverse = token.prepare_with_override(token.after(), MANAGER, &data, true).unwrap();
    assert_eq!(
        reverse.projected().affixes.iter().map(|a| (a.id, a.roll, a.seal)).collect::<Vec<_>>(),
        vec![(1, 11, 0), (1, 10, 0)]
    );
}
