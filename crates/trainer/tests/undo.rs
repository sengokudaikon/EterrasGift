use eg_game_access::{
    definitions::{ItemLimits, ValueRange},
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    locale::GameLocale,
    process::ProcessIdentity,
    protocol::ResponseCode,
};
use eg_trainer::{
    catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    edits::{CatalogProvenance, EditError, ExpectedObservation},
    undo::LevelUndo,
    writes::{LevelRequirementPlan, WriteRecord, WriteStatus},
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

fn provenance() -> CatalogProvenance {
    CatalogProvenance::RuntimeRead {
        generation: GENERATION,
        process: process(),
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
    let plan = LevelRequirementPlan::new(&observation(snapshot(old)), MANAGER, &catalog(), &provenance(), 17).unwrap();
    let status = WriteStatus::Confirmed(plan.projected().clone());
    WriteRecord { plan, status }
}

#[test]
fn fresh_exact_after_observation_prepares_only_the_original_level_with_full_identity() {
    let confirmed = record(87);
    let token = LevelUndo::from_confirmed(&confirmed).unwrap();
    assert_eq!(token.manager(), MANAGER);
    assert_eq!(token.restore_value(), 87);
    assert_eq!(token.after().snapshot(), confirmed.plan.projected());
    let current = observation(confirmed.plan.projected().clone());
    let plan = token.prepare(&current, MANAGER, &catalog(), &provenance()).unwrap();
    assert_eq!(plan.before(), &current);
    assert_eq!(plan.projected(), confirmed.plan.before().snapshot());
    assert_eq!(plan.value(), 87);
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
        assert_eq!(LevelUndo::from_confirmed(&record), Err(EditError::InvalidObservation));
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
        assert_eq!(LevelUndo::from_confirmed(&record), Err(EditError::InvalidObservation));
    }
}

#[test]
fn fields_outside_legacy_fingerprint_still_make_undo_stale() {
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
    for alter in [
        |s: &mut ForgeSnapshot| s.name.push('x'),
        |s: &mut ForgeSnapshot| s.class_requirement += 1,
        |s: &mut ForgeSnapshot| s.affixes[0].special += 1,
        |s: &mut ForgeSnapshot| s.affixes[0].name.push('x'),
        |s: &mut ForgeSnapshot| s.individual_id += 1,
    ] {
        let mut current = token.after().snapshot().clone();
        alter(&mut current);
        assert_eq!(current.legacy_fingerprint(), token.after().snapshot().fingerprint);
        assert_eq!(token.prepare(&observation(current), MANAGER, &catalog(), &provenance()), Err(EditError::StaleItem));
    }
}

#[test]
fn same_id_replacement_and_other_item_changes_cannot_be_restored_by_old_token() {
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
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
        assert_eq!(token.prepare(&observation(changed), MANAGER, &catalog(), &provenance()), Err(EditError::StaleItem));
    }
}

#[test]
fn manager_and_generation_changes_invalidate_the_undo_identity() {
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
    for manager in [0, MANAGER + 16] {
        assert_eq!(token.prepare(token.after(), manager, &catalog(), &provenance()), Err(EditError::IdentityChanged));
    }
    let current = ExpectedObservation::new(GENERATION + 1, process(), catalog().identity, token.after().snapshot().clone()).unwrap();
    assert_eq!(token.prepare(&current, MANAGER, &catalog(), &provenance()), Err(EditError::IdentityChanged));
}

#[test]
fn process_creation_image_or_pid_change_cannot_reuse_undo() {
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
    for alter in [
        |p: &mut ProcessIdentity| p.pid += 1,
        |p: &mut ProcessIdentity| p.created += 1,
        |p: &mut ProcessIdentity| p.executable.set_file_name("Other.exe"),
    ] {
        let mut target = process();
        alter(&mut target);
        let current = ExpectedObservation::new(GENERATION, target, catalog().identity, token.after().snapshot().clone()).unwrap();
        assert_eq!(token.prepare(&current, MANAGER, &catalog(), &provenance()), Err(EditError::IdentityChanged));
    }
}

#[test]
fn changed_catalog_identity_or_a_cached_catalog_cannot_authorize_restore() {
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
    for alter in [
        |c: &mut CatalogIdentity| c.binary_hash = "c".repeat(64),
        |c: &mut CatalogIdentity| c.metadata_hash = "c".repeat(64),
        |c: &mut CatalogIdentity| c.locale.language = "fr-FR".into(),
        |c: &mut CatalogIdentity| c.locale.game_version.push('x'),
        |c: &mut CatalogIdentity| c.extraction_version += 1,
    ] {
        let mut changed = catalog();
        alter(&mut changed.identity);
        assert_eq!(token.prepare(token.after(), MANAGER, &changed, &provenance()), Err(EditError::IdentityChanged));
        let current = ExpectedObservation::new(GENERATION, process(), changed.identity.clone(), token.after().snapshot().clone()).unwrap();
        assert_eq!(token.prepare(&current, MANAGER, &changed, &provenance()), Err(EditError::IdentityChanged));
    }
    for provenance in [
        CatalogProvenance::CachedOnly,
        CatalogProvenance::RuntimeRead {
            generation: GENERATION + 1,
            process: process(),
        },
        CatalogProvenance::RuntimeRead {
            generation: GENERATION,
            process: ProcessIdentity { pid: 29, ..process() },
        },
    ] {
        assert_eq!(token.prepare(token.after(), MANAGER, &catalog(), &provenance), Err(EditError::CatalogNotVerified));
    }
}

#[test]
fn old_level_outside_current_literal_range_is_unavailable_without_override() {
    let token = LevelUndo::from_confirmed(&record(138)).unwrap();
    assert_eq!(token.prepare(token.after(), MANAGER, &catalog(), &provenance()), Err(EditError::InvalidStatus));
    for old in [-1, 256, 0x01000057] {
        assert_eq!(LevelUndo::from_confirmed(&record(old)), Err(EditError::InvalidStatus));
    }
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
    let mut narrower = catalog();
    narrower.limits.level_requirement.maximum = 86;
    assert_eq!(token.prepare(token.after(), MANAGER, &narrower, &provenance()), Err(EditError::InvalidStatus));
}

#[test]
fn literal_endpoints_can_be_restored_and_the_prior_token_is_stale_after_restore() {
    for old in [0, 137] {
        let confirmed = record(old);
        let token = LevelUndo::from_confirmed(&confirmed).unwrap();
        let restored = token.prepare(token.after(), MANAGER, &catalog(), &provenance()).unwrap();
        assert_eq!(restored.value(), old);
        assert_eq!(restored.projected(), confirmed.plan.before().snapshot());
        let current = observation(restored.projected().clone());
        assert_eq!(token.prepare(&current, MANAGER, &catalog(), &provenance()), Err(EditError::StaleItem));
    }
}

#[test]
fn unique_set_or_legendary_drift_is_stale_and_cannot_create_an_original_level_plan() {
    let token = LevelUndo::from_confirmed(&record(87)).unwrap();
    for rarity in [7, 8, 9] {
        let mut changed = token.after().snapshot().clone();
        changed.rarity = rarity;
        changed.identity = Some(UniqueIdentity { id: 911, rolls: vec![0, 255] });
        changed.fingerprint = changed.legacy_fingerprint();
        let current = observation(changed);
        assert_eq!(token.prepare(&current, MANAGER, &catalog(), &provenance()), Err(EditError::StaleItem));
        assert_eq!(LevelRequirementPlan::new(&current, MANAGER, &catalog(), &provenance(), 87), Err(EditError::InvalidStatus));
    }
}

#[test]
fn pure_preparation_failure_preserves_the_owned_token_and_never_changes_the_record() {
    let record = record(87);
    let saved = record.clone();
    let token = LevelUndo::from_confirmed(&record).unwrap();
    let saved_token = token.clone();
    assert_eq!(
        token.prepare(token.after(), MANAGER, &catalog(), &CatalogProvenance::CachedOnly),
        Err(EditError::CatalogNotVerified)
    );
    assert_eq!(token, saved_token);
    assert_eq!(record, saved);
    let next = token.prepare(token.after(), MANAGER, &catalog(), &provenance()).unwrap();
    assert_eq!(next.value(), 87);
    assert_eq!(record, saved);
}
