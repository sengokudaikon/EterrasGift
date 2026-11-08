use eg_trainer::definitions::catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog, ItemNameSource};
use eg_trainer::definitions::entries::{Availability, CatalogEntries, DefinitionName, DefinitionNameSource, ItemTypeDefinition, SubTypeDefinition, UniqueDefinition};
use eg_trainer_wire::{
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
fn identity() -> CatalogIdentity {
    CatalogIdentity {
        binary_hash: "A".repeat(64),
        metadata_hash: "B".repeat(64),
        locale: locale(),
        extraction_version: EXTRACTION_VERSION,
    }
}
/// A catalog of `count` bases and nothing else.
fn catalog(count: u8) -> GameCatalog {
    GameCatalog::new(
        identity(),
        limits(),
        CatalogEntries {
            bases: (0..count).map(base).collect(),
            ..CatalogEntries::default()
        },
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
            affix_effect_modifier: 0.0,
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
#[test]
fn a_catalog_is_made_whole_and_keeps_owned_provenance() {
    let catalog = catalog(17);
    assert_eq!(catalog.bases.len(), 17);
    assert_eq!(catalog.identity.locale, locale());
    assert_eq!(catalog.bases[&16].sub_types[0].name.source, DefinitionNameSource::InternalName);
    assert_eq!(catalog.check(), Ok(()));
}

#[test]
fn one_entry_or_identity_that_fails_fails_the_catalog() {
    let entries = |bases: Vec<ItemTypeDefinition>| CatalogEntries {
        bases,
        ..CatalogEntries::default()
    };
    assert!(GameCatalog::new(identity(), limits(), entries(vec![base(3), base(3)])).is_err(), "two bases share an ID");
    let mut twice = base(3);
    twice.sub_types.push(twice.sub_types[0].clone());
    assert!(GameCatalog::new(identity(), limits(), entries(vec![twice])).is_err(), "a base lists a subtype twice");
    let mut other = identity();
    other.extraction_version += 1;
    assert!(GameCatalog::new(other, limits(), entries(vec![base(3)])).is_err(), "another extraction");
    let mut made = catalog(2);
    made.bases.get_mut(&1).unwrap().item_type = 0;
    assert!(made.check().is_err(), "a base kept under another's ID");
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
    let catalog = catalog(1);
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
    let mut catalog = catalog(1);
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
