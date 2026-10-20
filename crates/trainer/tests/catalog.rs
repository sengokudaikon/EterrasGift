use eg_trainer::catalog::{CatalogBuilder, CatalogIdentity, EXTRACTION_VERSION, ItemNameSource};
use eg_trainer_wire::{
    catalog::{Availability, CatalogEntries, CatalogKind, CatalogPage, DefinitionName, DefinitionNameSource, ItemTypeDefinition, SubTypeDefinition, UniqueDefinition},
    definitions::{ItemLimits, ValueRange},
    forge::{ForgeSnapshot, ItemStatus, UniqueIdentity},
    locale::GameLocale,
};

fn locale() -> GameLocale {
    GameLocale {
        language: "en-🌿".into(),
        game_version: "1.5-test".into(),
    }
}
fn limits() -> ItemLimits {
    let range = ValueRange { minimum: 0, maximum: 255 };
    ItemLimits {
        forging_potential: range,
        legendary_potential: range,
        weavers_will: range,
        weavers_touch: range,
        level_requirement: range,
    }
}
fn builder() -> CatalogBuilder {
    CatalogBuilder::new(
        CatalogIdentity {
            binary_hash: "A".repeat(64),
            metadata_hash: "B".repeat(64),
            locale: locale(),
            extraction_version: EXTRACTION_VERSION,
        },
        limits(),
    )
    .unwrap()
}
fn base(id: u8) -> ItemTypeDefinition {
    ItemTypeDefinition {
        item_type: id,
        name: DefinitionName {
            text: "Axes".into(),
            source: DefinitionNameSource::StoredDisplay,
        },
        sub_types: vec![SubTypeDefinition {
            sub_type: 5,
            name: DefinitionName {
                text: format!("Hatchet {id} 🪓"),
                source: DefinitionNameSource::InternalName,
            },
            level_requirement: 1,
            class_requirement: 0,
            cannot_drop: false,
            is_corrupted: false,
            implicits: Availability::Available(vec![]),
        }],
    }
}
fn bases(cursor: u32, total: u32) -> CatalogPage {
    CatalogPage {
        locale: locale(),
        kind: CatalogKind::Bases,
        cursor,
        total,
        entries: CatalogEntries::Bases((cursor..total.min(cursor + 16)).map(|id| base(id as u8)).collect()),
    }
}
fn empty(kind: CatalogKind) -> CatalogPage {
    CatalogPage {
        locale: locale(),
        kind,
        cursor: 0,
        total: 0,
        entries: match kind {
            CatalogKind::Bases => CatalogEntries::Bases(vec![]),
            CatalogKind::Affixes => CatalogEntries::Affixes(vec![]),
            CatalogKind::Uniques => CatalogEntries::Uniques(vec![]),
            CatalogKind::Rarities => CatalogEntries::Rarities(vec![]),
        },
    }
}
fn finish_empty_categories(builder: &mut CatalogBuilder) {
    for kind in [CatalogKind::Affixes, CatalogKind::Uniques, CatalogKind::Rarities] {
        builder.push(empty(kind)).unwrap();
    }
}

#[test]
fn complete_capture_assembles_two_pages_and_preserves_owned_provenance() {
    let mut builder = builder();
    assert_eq!(builder.next_cursor(CatalogKind::Bases), Some(0));
    builder.push(bases(0, 17)).unwrap();
    assert_eq!(builder.next_cursor(CatalogKind::Bases), Some(16));
    builder.push(bases(16, 17)).unwrap();
    assert_eq!(builder.next_cursor(CatalogKind::Bases), None);
    finish_empty_categories(&mut builder);
    let catalog = builder.finish().unwrap();
    assert_eq!(catalog.bases.len(), 17);
    assert_eq!(catalog.identity.locale, locale());
    assert_eq!(catalog.bases[&16].sub_types[0].name.source, DefinitionNameSource::InternalName);
}

#[test]
fn drift_duplicate_wrong_cursor_and_replayed_pages_poison_capture_permanently() {
    for fault in 0..6 {
        let mut builder = builder();
        builder.push(bases(0, 17)).unwrap();
        let mut page = bases(16, 17);
        match fault {
            0 => page.locale.language = "sr-Latn-RS".into(),
            1 => page.locale.game_version = "changed".into(),
            2 => page = bases(16, 18),
            3 => {
                if let CatalogEntries::Bases(entries) = &mut page.entries {
                    entries[0].item_type = 0;
                }
            }
            4 => page = bases(0, 17),
            5 => page = bases(17, 17),
            _ => unreachable!(),
        }
        assert!(builder.push(page).is_err(), "fault {fault}");
        assert!(builder.push(bases(16, 17)).is_err());
        assert_eq!(builder.next_cursor(CatalogKind::Bases), None);
        assert!(builder.finish().is_err());
    }
    let mut builder = builder();
    builder.push(empty(CatalogKind::Bases)).unwrap();
    assert!(builder.push(empty(CatalogKind::Bases)).is_err());
    assert!(builder.finish().is_err());
}

#[test]
fn incomplete_or_locally_malformed_pages_cannot_publish_a_catalog() {
    let mut incomplete = builder();
    incomplete.push(bases(0, 17)).unwrap();
    finish_empty_categories(&mut incomplete);
    assert!(incomplete.finish().is_err());
    let mut malformed = builder();
    let mut page = bases(0, 1);
    if let CatalogEntries::Bases(entries) = &mut page.entries {
        let duplicate = entries[0].sub_types[0].clone();
        entries[0].sub_types.push(duplicate);
    }
    assert!(malformed.push(page).is_err());
    assert!(malformed.finish().is_err());
    let identity = CatalogIdentity {
        binary_hash: "bad".into(),
        metadata_hash: "B".repeat(64),
        locale: locale(),
        extraction_version: EXTRACTION_VERSION,
    };
    assert!(CatalogBuilder::new(identity, limits()).is_err());
}

fn item() -> ForgeSnapshot {
    ForgeSnapshot {
        item: 0x12340,
        individual_id: 42,
        fingerprint: 0,
        name: String::new(),
        item_type: 0,
        sub_type: 5,
        rarity: 4,
        class_requirement: 0,
        affixes: vec![],
        implicit_rolls: vec![],
        identity: None,
        status: ItemStatus {
            forging_potential: 0,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 1,
        },
    }
}

#[test]
fn fallback_names_have_explicit_provenance_without_altering_raw_item_or_fingerprint() {
    let mut builder = builder();
    builder.push(bases(0, 1)).unwrap();
    finish_empty_categories(&mut builder);
    let catalog = builder.finish().unwrap();
    let mut item = item();
    let original = item.clone();
    let resolved = catalog.resolve_item_name(&item).unwrap();
    assert_eq!(resolved.text, "Hatchet 0 🪓");
    assert_eq!(
        resolved.source,
        ItemNameSource::BaseDefinition {
            source: DefinitionNameSource::InternalName
        }
    );
    assert_eq!(item, original);
    item.name = "Game's full affixed name".into();
    assert_eq!(catalog.resolve_item_name(&item).unwrap().source, ItemNameSource::RuntimeCache);
    item.name.clear();
    item.sub_type = 6;
    assert!(catalog.resolve_item_name(&item).is_none());
}

#[test]
fn unique_names_require_matching_identity_base_subtype_and_set_semantics() {
    let mut builder = builder();
    builder.push(bases(0, 1)).unwrap();
    finish_empty_categories(&mut builder);
    let mut catalog = builder.finish().unwrap();
    catalog.uniques.insert(
        41,
        UniqueDefinition {
            id: 41,
            name: DefinitionName {
                text: "Authored unique".into(),
                source: DefinitionNameSource::StoredDisplay,
            },
            item_type: 0,
            sub_types: vec![5],
            is_set_item: false,
            set_id: 0,
            legendary_type: 0,
            can_drop_as_legendary: false,
            is_pre_corrupted: false,
            numeric: Availability::Unavailable { reason: "Unaudited".into() },
        },
    );
    let mut item = item();
    item.rarity = 7;
    item.identity = Some(UniqueIdentity { id: 41, rolls: vec![] });
    assert_eq!(
        catalog.resolve_item_name(&item).unwrap().source,
        ItemNameSource::UniqueDefinition {
            source: DefinitionNameSource::StoredDisplay
        }
    );
    for fault in 0..5 {
        let mut wrong = item.clone();
        match fault {
            0 => wrong.item_type = 1,
            1 => wrong.sub_type = 256,
            2 => wrong.sub_type = 6,
            3 => wrong.identity.as_mut().unwrap().id = 42,
            4 => wrong.rarity = 8,
            _ => unreachable!(),
        }
        assert!(catalog.resolve_item_name(&wrong).is_none());
    }
    catalog.uniques.get_mut(&41).unwrap().is_set_item = true;
    item.rarity = 8;
    assert!(catalog.resolve_item_name(&item).is_some());
}
