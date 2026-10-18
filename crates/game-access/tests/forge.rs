use eg_game_access::{
    forge::{self, AffixSnapshot, ForgeSnapshot, ItemStatus, MAX_AFFIXES, MAX_IMPLICIT_ROLLS, MAX_NAME_BYTES, MAX_UNIQUE_ROLLS, UniqueIdentity},
    protocol::{MAX_RESPONSE_SIZE, ProtocolError},
};

fn snapshot() -> ForgeSnapshot {
    ForgeSnapshot {
        item: 0x1122_3344_5566_7788,
        individual_id: 0xaabb_ccdd,
        fingerprint: 0x3b1b_c599_d1d6_a98d,
        name: "剑".into(),
        item_type: 42,
        sub_type: 513,
        rarity: 9,
        class_requirement: -7,
        affixes: vec![AffixSnapshot {
            slot: 0,
            id: 513,
            tier: 256,
            roll: 250,
            kind: -2,
            seal: 3,
            name: "火".into(),
            special: -123,
        }],
        implicit_rolls: vec![1, 128, 255],
        identity: Some(UniqueIdentity { id: 258, rolls: vec![0, 255] }),
        status: ItemStatus {
            forging_potential: 77,
            forging_potential_type: -3,
            legendary_potential: 2,
            weavers_will: 16,
            weavers_touch: 4,
            corrupted: true,
            ruined: false,
            level_requirement: -5,
        },
    }
}

fn golden() -> Vec<u8> {
    // Independently assembled LE wire bytes. Fingerprint input is the sequence
    // of int32s recorded in legacy_fingerprint_matches_the_native_known_vector.
    let hex = concat!(
        "018877665544332211ddccbbaa8da9d6d199c51b3b0300e58991",
        "2a010209f9ffffff010001020001fafeffffff030000000300e781ab85ffffff",
        "030180ff0102010200ff4dfdffffff0210040100fbffffff"
    );
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn seal(snapshot: &mut ForgeSnapshot) {
    snapshot.fingerprint = snapshot.legacy_fingerprint();
}

#[test]
fn empty_forge_is_exactly_one_zero_byte() {
    assert_eq!(forge::encode(None), Ok(vec![0]));
    assert_eq!(forge::decode(&[0]), Ok(None));
    for bytes in [&[][..], &[0, 0], &[2], &[255]] {
        assert_eq!(forge::decode(bytes), Err(ProtocolError::InvalidData));
    }
}

#[test]
fn owned_snapshot_matches_independent_wire_example() {
    let expected = snapshot();
    let wire = golden();
    assert_eq!(wire.len(), 82);
    assert_eq!(forge::encode(Some(&expected)), Ok(wire.clone()));
    assert_eq!(forge::decode(&wire), Ok(Some(expected)));
    for length in 0..wire.len() {
        assert!(forge::decode(&wire[..length]).is_err(), "truncated at {length}");
    }
    assert!(forge::decode(&[wire.as_slice(), &[0]].concat()).is_err());
    assert!(forge::decode(&vec![0; MAX_RESPONSE_SIZE + 1]).is_err());
}

#[test]
fn legacy_fingerprint_matches_the_native_known_vector() {
    // Native item.h mixes all of these as four LE bytes, never their wire width.
    let values: [i32; 25] = [42, 1, 513, 256, 250, -2, 3, 513, 9, 258, 2, 0, 255, 3, 1, 128, 255, 77, -3, 2, 16, 4, 1, 0, -5];
    let mut independent = 0xcbf2_9ce4_8422_2325_u64;
    for value in values {
        for byte in value.to_le_bytes() {
            independent ^= u64::from(byte);
            independent = independent.wrapping_mul(0x100_0000_01b3);
        }
    }
    assert_eq!(independent, 0x3b1b_c599_d1d6_a98d);
    assert_eq!(snapshot().legacy_fingerprint(), independent);
}

#[test]
fn set_identity_is_preserved_but_excluded_from_legacy_fingerprint() {
    for rarity in [7, 8, 9] {
        let mut original = snapshot();
        original.rarity = rarity;
        seal(&mut original);
        let mut changed = original.clone();
        changed.identity = Some(UniqueIdentity {
            id: 65535,
            rolls: vec![34, 89, 13],
        });
        if rarity == 8 {
            assert_eq!(original.legacy_fingerprint(), changed.legacy_fingerprint());
        } else {
            assert_ne!(original.legacy_fingerprint(), changed.legacy_fingerprint());
            seal(&mut changed);
        }
        assert_eq!(forge::decode(&forge::encode(Some(&changed)).unwrap()), Ok(Some(changed)));
    }
}

#[test]
fn legacy_fingerprint_intentionally_omits_presentation_and_external_identity_fields() {
    let original = snapshot();
    let mut changed = original.clone();
    changed.item = 42;
    changed.individual_id = 12;
    changed.name = "Another name".into();
    changed.class_requirement = i32::MIN;
    changed.affixes[0].name = "Another affix name".into();
    changed.affixes[0].special = i32::MAX;
    assert_eq!(changed.legacy_fingerprint(), original.fingerprint);
    assert!(forge::encode(Some(&changed)).is_ok());
}

#[test]
fn bounded_maximum_snapshot_and_empty_names_round_trip() {
    let mut maximum = snapshot();
    maximum.name = "é".repeat(MAX_NAME_BYTES / 2);
    maximum.affixes = (0..MAX_AFFIXES)
        .map(|slot| AffixSnapshot {
            slot: slot as u8,
            name: "x".repeat(MAX_NAME_BYTES),
            tier: if slot == 0 { 1 } else { 256 },
            ..maximum.affixes[0].clone()
        })
        .collect();
    maximum.identity.as_mut().unwrap().rolls = vec![255; MAX_UNIQUE_ROLLS];
    maximum.implicit_rolls = vec![255; MAX_IMPLICIT_ROLLS];
    seal(&mut maximum);
    let bytes = forge::encode(Some(&maximum)).unwrap();
    assert!(bytes.len() < MAX_RESPONSE_SIZE);
    assert_eq!(forge::decode(&bytes), Ok(Some(maximum)));
    let mut empty = snapshot();
    empty.name.clear();
    empty.affixes[0].name.clear();
    empty.affixes.clear();
    empty.implicit_rolls.clear();
    empty.identity.as_mut().unwrap().rolls.clear();
    seal(&mut empty);
    assert_eq!(forge::decode(&forge::encode(Some(&empty)).unwrap()), Ok(Some(empty)));
}

#[test]
fn encode_rejects_invalid_structure_even_with_a_recomputed_fingerprint() {
    let original = snapshot();
    let mutations: [fn(&mut ForgeSnapshot); 11] = [
        |item| item.item = 0,
        |item| item.name = "x".repeat(MAX_NAME_BYTES + 1),
        |item| item.affixes[0].name = "x".repeat(MAX_NAME_BYTES + 1),
        |item| item.affixes[0].tier = 0,
        |item| item.affixes[0].tier = 257,
        |item| item.affixes[0].slot = 1,
        |item| {
            item.affixes = (0..=MAX_AFFIXES)
                .map(|slot| AffixSnapshot {
                    slot: slot as u8,
                    ..item.affixes[0].clone()
                })
                .collect()
        },
        |item| item.implicit_rolls = vec![0; MAX_IMPLICIT_ROLLS + 1],
        |item| item.identity.as_mut().unwrap().rolls = vec![0; MAX_UNIQUE_ROLLS + 1],
        |item| item.identity = None,
        |item| item.rarity = 6,
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut item = original.clone();
        mutate(&mut item);
        seal(&mut item);
        assert!(forge::encode(Some(&item)).is_err(), "invalid structure {index}");
    }
}

#[test]
fn malformed_wire_flags_counts_utf8_and_tiers_are_rejected() {
    let original = golden();
    // Offsets refer to the independent 82-byte wire example, not the encoder.
    for (position, byte) in [(0, 2), (23, 0xff), (34, 33), (35, 1), (38, 0), (51, 0xff), (58, 4), (62, 2), (65, 9), (76, 2), (77, 255)] {
        let mut bytes = original.clone();
        bytes[position] = byte;
        // tier 256 is 00 01; turn it into 0 without changing its width.
        if position == 38 {
            bytes[39] = 0;
        }
        assert!(forge::decode(&bytes).is_err(), "malformed field at {position}");
    }
    for position in [21, 49] {
        let mut bytes = original.clone();
        bytes[position..position + 2].copy_from_slice(&4097_u16.to_le_bytes());
        assert!(forge::decode(&bytes).is_err());
    }
    let mut zero_item = original;
    zero_item[1..9].fill(0);
    assert!(forge::decode(&zero_item).is_err());
}

#[test]
fn stale_fingerprint_and_identity_rarity_mismatch_are_rejected() {
    let mut stale = snapshot();
    stale.status.forging_potential += 1;
    assert!(forge::encode(Some(&stale)).is_err());
    let mut wire = golden();
    wire[13] ^= 1;
    assert!(forge::decode(&wire).is_err());
    // Set the rarity to a non-identity kind, and repair only the fingerprint.
    let mut wrong_rarity = snapshot();
    wrong_rarity.rarity = 6;
    wire = golden();
    wire[29] = 6;
    wire[13..21].copy_from_slice(&wrong_rarity.legacy_fingerprint().to_le_bytes());
    assert!(forge::decode(&wire).is_err());
    // Conversely, unique-backed rarity requires an identity even with no rolls.
    let mut missing_identity = snapshot();
    missing_identity.identity = None;
    wire = golden();
    wire[62] = 0;
    wire.drain(63..68);
    wire[13..21].copy_from_slice(&missing_identity.legacy_fingerprint().to_le_bytes());
    assert!(forge::decode(&wire).is_err());
}

#[test]
fn unknown_enum_values_and_plain_items_are_preserved() {
    let mut plain = snapshot();
    plain.rarity = 255;
    plain.identity = None;
    plain.class_requirement = i32::MIN;
    plain.status.forging_potential_type = i32::MAX;
    plain.affixes[0].kind = i32::MIN;
    plain.affixes[0].seal = i32::MAX;
    plain.affixes[0].special = -123456;
    seal(&mut plain);
    assert_eq!(forge::decode(&forge::encode(Some(&plain)).unwrap()), Ok(Some(plain)));
}
