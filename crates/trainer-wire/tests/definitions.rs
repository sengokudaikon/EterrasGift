use eg_trainer_wire::{
    definitions::{self, ItemLimits, ValueRange},
    protocol::ProtocolError,
};

fn expected() -> ItemLimits {
    ItemLimits {
        forging_potential: ValueRange { minimum: 0, maximum: 255 },
        legendary_potential: ValueRange { minimum: 0, maximum: 4 },
        weavers_will: ValueRange { minimum: 1, maximum: 28 },
        weavers_touch: ValueRange { minimum: 3, maximum: 15 },
        level_requirement: ValueRange { minimum: 0, maximum: 100 },
    }
}

fn golden() -> [u8; 40] {
    // Independently authored native appendInt(min), appendInt(max), five times.
    [
        0, 0, 0, 0, 255, 0, 0, 0, // forging potential
        0, 0, 0, 0, 4, 0, 0, 0, // legendary potential
        1, 0, 0, 0, 28, 0, 0, 0, // Weaver's Will
        3, 0, 0, 0, 15, 0, 0, 0, // Weaver's Touch
        0, 0, 0, 0, 100, 0, 0, 0, // level requirement
    ]
}

#[test]
fn literal_limits_match_independent_wire_order() {
    assert_eq!(definitions::decode(&golden()), Ok(expected()));
    assert_eq!(definitions::encode(&expected()), Ok(golden().to_vec()));
}

#[test]
fn both_byte_bounds_are_accepted_in_every_range() {
    for byte in [0_u8, 255] {
        for slot in 0..5 {
            let mut wire = golden();
            wire[slot * 8..slot * 8 + 4].copy_from_slice(&i32::from(byte).to_le_bytes());
            wire[slot * 8 + 4..slot * 8 + 8].copy_from_slice(&i32::from(byte).to_le_bytes());
            let limits = definitions::decode(&wire).unwrap();
            assert_eq!(definitions::encode(&limits).unwrap(), wire);
        }
    }
}

#[test]
fn negative_wide_or_reversed_ranges_are_rejected_in_every_position() {
    for slot in 0..5 {
        for (minimum, maximum) in [(-1_i32, 0), (0, -1), (0, 256), (256, 256), (100, 99), (i32::MIN, i32::MAX)] {
            let mut wire = golden();
            wire[slot * 8..slot * 8 + 4].copy_from_slice(&minimum.to_le_bytes());
            wire[slot * 8 + 4..slot * 8 + 8].copy_from_slice(&maximum.to_le_bytes());
            assert_eq!(definitions::decode(&wire), Err(ProtocolError::InvalidData));
        }
    }
    for slot in 0..5 {
        let mut limits = expected();
        let fields = [
            &mut limits.forging_potential,
            &mut limits.legendary_potential,
            &mut limits.weavers_will,
            &mut limits.weavers_touch,
            &mut limits.level_requirement,
        ];
        *fields.into_iter().nth(slot).unwrap() = ValueRange { minimum: 200, maximum: 199 };
        assert_eq!(definitions::encode(&limits), Err(ProtocolError::InvalidData));
    }
}

#[test]
fn range_membership_includes_endpoints_and_rejects_outside_values() {
    let range = ValueRange { minimum: 10, maximum: 20 };
    for (value, inside) in [(0, false), (9, false), (10, true), (15, true), (20, true), (21, false), (255, false)] {
        assert_eq!(range.contains(value), inside, "{value}");
    }
    assert!(ValueRange { minimum: 0, maximum: 255 }.contains(255));
    assert!(!ValueRange { minimum: 20, maximum: 10 }.contains(15));
}
