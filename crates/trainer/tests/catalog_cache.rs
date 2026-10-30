use std::{
    collections::BTreeMap,
    fs::{self, File, FileTimes},
    time::{Duration, SystemTime},
};

use eg_trainer::definitions::entries::{Availability, DefinitionName, DefinitionNameSource, ItemTypeDefinition, RarityDefinition, SubTypeDefinition};
use eg_trainer::{
    definitions::cache::{CATALOG_FILE, CacheError, CatalogCache, KEPT_BUILDS, POINTER_FILE, identity_key},
    definitions::catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog, MAX_CATALOG_BYTES},
};
use eg_trainer_wire::{
    definitions::{ItemLimits, ValueRange},
    locale::GameLocale,
};

fn catalog() -> GameCatalog {
    let range = ValueRange { minimum: 0, maximum: 255 };
    GameCatalog {
        identity: CatalogIdentity {
            binary_hash: "A".repeat(64),
            metadata_hash: "B".repeat(64),
            locale: GameLocale {
                language: "en".into(),
                game_version: "1.2.3".into(),
            },
            extraction_version: EXTRACTION_VERSION,
        },
        limits: ItemLimits {
            forging_potential: range,
            legendary_potential: range,
            weavers_will: range,
            weavers_touch: range,
            level_requirement: range,
        },
        bases: (0..17)
            .map(|id| {
                (
                    id,
                    ItemTypeDefinition {
                        item_type: id,
                        name: DefinitionName {
                            text: format!("Base {id}"),
                            source: DefinitionNameSource::StoredDisplay,
                        },
                        sub_types: vec![SubTypeDefinition {
                            sub_type: 255,
                            name: DefinitionName {
                                text: "金 🪓".into(),
                                source: DefinitionNameSource::InternalName,
                            },
                            level_requirement: 3,
                            class_requirement: 0,
                            cannot_drop: false,
                            is_corrupted: true,
                            implicits: Availability::Unavailable {
                                reason: "numeric getter not audited".into(),
                            },
                        }],
                    },
                )
            })
            .collect(),
        affixes: BTreeMap::new(),
        uniques: BTreeMap::new(),
        rarities: [(
            9,
            RarityDefinition {
                id: 9,
                name: Availability::Unavailable {
                    reason: "formatter not audited".into(),
                },
            },
        )]
        .into(),
    }
}
fn replace_checksum(bytes: &mut Vec<u8>) {
    bytes.truncate(bytes.len() - 64);
    let checksum = eg_core::hashes::of_bytes(bytes);
    bytes.extend_from_slice(checksum.as_bytes());
}
fn pages_start(bytes: &[u8]) -> usize {
    let mut pos = 12;
    for _ in 0..4 {
        let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4 + len;
    }
    pos + 4 + 40 + 4
}

#[test]
fn roundtrip_multi_page_catalog_and_last_pointer_preserves_owned_data() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let expected = catalog();
    assert!(cache.load(&expected.identity).unwrap().is_none());
    assert!(cache.load_last().unwrap().is_none());
    cache.save(&expected).unwrap();
    assert_eq!(cache.load(&expected.identity).unwrap(), Some(expected.clone()));
    assert_eq!(cache.load_last().unwrap(), Some(expected.clone()));
    assert_eq!(fs::read_to_string(temp.path().join(POINTER_FILE)).unwrap(), identity_key(&expected.identity).unwrap());
    let path = cache.path_for(&expected.identity).unwrap();
    let first = fs::read(&path).unwrap();
    cache.save(&expected).unwrap();
    assert_eq!(fs::read(path).unwrap(), first);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    assert_eq!(fs::read_dir(cache.path_for(&expected.identity).unwrap().parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn identity_key_binds_every_field_without_delimiter_collisions() {
    let original = catalog().identity;
    let first = identity_key(&original).unwrap();
    assert!(eg_core::hashes::is_sha256(&first));
    for field in 0..4 {
        let mut changed = original.clone();
        match field {
            0 => changed.binary_hash = "C".repeat(64),
            1 => changed.metadata_hash = "D".repeat(64),
            2 => changed.locale.language = "sr-Latn".into(),
            _ => changed.locale.game_version = "1.2.4".into(),
        }
        assert_ne!(identity_key(&changed).unwrap(), first);
    }
    let mut a = original.clone();
    let mut b = original.clone();
    a.locale.language = "a:b".into();
    a.locale.game_version = "c".into();
    b.locale.language = "a".into();
    b.locale.game_version = "b:c".into();
    assert_ne!(identity_key(&a).unwrap(), identity_key(&b).unwrap());
    let mut invalid = original.clone();
    invalid.extraction_version += 1;
    assert!(matches!(identity_key(&invalid), Err(CacheError::InvalidData)));
    invalid = original.clone();
    invalid.binary_hash = "../escape".into();
    assert!(identity_key(&invalid).is_err());
    invalid = original;
    invalid.locale.language.clear();
    assert!(identity_key(&invalid).is_err());
}

#[test]
fn wrong_identity_inside_valid_checksum_file_and_last_pointer_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let a = catalog();
    cache.save(&a).unwrap();
    let mut b = a.identity.clone();
    b.locale.game_version = "other version".into();
    let b_path = cache.path_for(&b).unwrap();
    fs::create_dir_all(b_path.parent().unwrap()).unwrap();
    fs::copy(cache.path_for(&a.identity).unwrap(), &b_path).unwrap();
    assert!(matches!(cache.load(&b), Err(CacheError::IdentityMismatch)));
    fs::write(temp.path().join(POINTER_FILE), identity_key(&b).unwrap()).unwrap();
    assert!(matches!(cache.load_last(), Err(CacheError::IdentityMismatch)));
    fs::write(temp.path().join(POINTER_FILE), "../not-a-key").unwrap();
    assert!(matches!(cache.load_last(), Err(CacheError::InvalidData)));
    fs::write(temp.path().join(POINTER_FILE), format!("{}\n", identity_key(&a.identity).unwrap())).unwrap();
    assert!(cache.load_last().is_err());
}

#[test]
fn corruption_truncation_tails_and_schema_changes_never_return_catalogs() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let expected = catalog();
    cache.save(&expected).unwrap();
    let path = cache.path_for(&expected.identity).unwrap();
    let original = fs::read(&path).unwrap();
    for length in [0, 7, 12, original.len() / 2, original.len() - 1] {
        fs::write(&path, &original[..length]).unwrap();
        assert!(matches!(cache.load(&expected.identity), Err(CacheError::InvalidData)));
    }
    let mut corrupted = original.clone();
    corrupted[50] ^= 1;
    fs::write(&path, &corrupted).unwrap();
    assert!(cache.load(&expected.identity).is_err());
    let mut bad_schema = original.clone();
    bad_schema[8..12].copy_from_slice(&2u32.to_le_bytes());
    replace_checksum(&mut bad_schema);
    fs::write(&path, bad_schema).unwrap();
    assert!(cache.load(&expected.identity).is_err());
    let mut tail = original.clone();
    tail.insert(tail.len() - 64, 0);
    replace_checksum(&mut tail);
    fs::write(&path, tail).unwrap();
    assert!(cache.load(&expected.identity).is_err());
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "corruption is retained, not silently removed");
}

#[test]
fn recomputed_checksum_cannot_bypass_cross_page_duplicate_validation() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let expected = catalog();
    cache.save(&expected).unwrap();
    let path = cache.path_for(&expected.identity).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let start = pages_start(&bytes);
    let first_length = u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
    let second = start + 4 + first_length;
    let entry_offset = 12 + 4 + expected.identity.locale.language.len() + 4 + expected.identity.locale.game_version.len() + 4;
    assert_eq!(bytes[second + 4 + entry_offset], 16);
    bytes[second + 4 + entry_offset] = 0;
    replace_checksum(&mut bytes);
    fs::write(path, bytes).unwrap();
    assert!(cache.load(&expected.identity).is_err());
}

#[test]
fn oversized_file_is_rejected_before_reading_its_contents() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let expected = catalog();
    let path = cache.path_for(&expected.identity).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    File::create(&path).unwrap().set_len(MAX_CATALOG_BYTES as u64 + 1).unwrap();
    assert!(matches!(cache.load(&expected.identity), Err(CacheError::TooLarge)));
}

#[test]
fn malformed_public_catalog_cannot_replace_existing_atomic_file_or_pointer() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let expected = catalog();
    cache.save(&expected).unwrap();
    let path = cache.path_for(&expected.identity).unwrap();
    let before = fs::read(&path).unwrap();
    let pointer = fs::read(temp.path().join(POINTER_FILE)).unwrap();
    for fault in 0..4 {
        let mut broken = expected.clone();
        match fault {
            0 => {
                broken.bases.get_mut(&0).unwrap().item_type = 1;
            }
            1 => {
                let base = broken.bases.get_mut(&0).unwrap();
                base.sub_types.push(base.sub_types[0].clone());
            }
            2 => broken.limits.forging_potential = ValueRange { minimum: 2, maximum: 1 },
            _ => broken.bases.get_mut(&0).unwrap().sub_types[0].implicits = Availability::Unavailable { reason: "".into() },
        }
        assert!(cache.save(&broken).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read(temp.path().join(POINTER_FILE)).unwrap(), pointer);
    }
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    assert_eq!(cache.load_last().unwrap(), Some(expected));
}

#[test]
fn retention_keeps_current_and_two_newest_and_preserves_unrelated_files() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let mut identities = vec![];
    for index in 0..4 {
        let mut current = catalog();
        current.identity.locale.game_version = format!("build-{index}");
        cache.save(&current).unwrap();
        let path = cache.path_for(&current.identity).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(100 + index)))
            .unwrap();
        if index == 0 {
            fs::write(path.parent().unwrap().join("personal.txt"), "preserve").unwrap();
        }
        identities.push(current.identity);
    }
    assert!(cache.load(&identities[0]).unwrap().is_none());
    for identity in &identities[1..] {
        assert!(cache.load(identity).unwrap().is_some());
    }
    assert!(cache.path_for(&identities[0]).unwrap().parent().unwrap().join("personal.txt").exists());
    assert_eq!(
        fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join(CATALOG_FILE).exists())
            .count(),
        KEPT_BUILDS
    );
    assert_eq!(cache.load_last().unwrap().unwrap().identity, identities[3]);
}

#[test]
fn io_failure_is_explicit_and_pointer_is_not_replaced_by_failed_catalog_write() {
    let temp = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(temp.path());
    let expected = catalog();
    cache.save(&expected).unwrap();
    let mut changed = expected.clone();
    changed.identity.locale.game_version = "blocked".into();
    let path = cache.path_for(&changed.identity).unwrap();
    fs::create_dir_all(&path).unwrap();
    assert!(matches!(cache.save(&changed), Err(CacheError::Io(_))));
    assert_eq!(cache.load_last().unwrap(), Some(expected));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "temporary file is removed after failed rename");
}
