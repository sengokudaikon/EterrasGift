use eg_trainer_wire::{
    fields::{self, AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate, RateUnavailable},
    protocol::ProtocolError,
};

// Independently authored wire fixture, including non-symmetric identity bytes.
const AVAILABLE: [u8; 64] = [
    0x31, 0x46, 0x47, 0x45, 1, 0, 1, 0, 8, 7, 6, 5, 4, 3, 2, 1, 24, 23, 22, 21, 20, 19, 18, 17, 40, 39, 38, 37, 36, 35, 34, 33, 56, 55, 54, 53, 52, 51, 50, 49, 72, 71, 70, 69, 68,
    67, 66, 65, 88, 87, 86, 85, 84, 83, 82, 81, 0xcd, 0xcc, 0x8c, 0x3f, 0, 0, 0, 0xbe,
];

fn observation() -> AttackLeechRead {
    AttackLeechRead::Available(AttackLeechObservation {
        manager: ObservedObjectIdentity::new(0x0102030405060708, 0x1112131415161718).unwrap(),
        actor: ObservedObjectIdentity::new(0x2122232425262728, 0x3132333435363738).unwrap(),
        stats: ObservedObjectIdentity::new(0x4142434445464748, 0x5152535455565758).unwrap(),
        attack_rate: ObservedRate { raw_bits: 0x3f8ccccd },
        increased_leech_rate: ObservedRate { raw_bits: 0xbe000000 },
    })
}

#[test]
fn independent_available_wire_and_opaque_tokens() {
    let value = observation();
    assert_eq!(fields::encode(&value), AVAILABLE);
    assert_eq!(fields::decode(&AVAILABLE), Ok(value));
    let AttackLeechRead::Available(value) = value else { unreachable!() };
    assert_eq!(value.manager.wrapper_token(), 0x0102030405060708);
    assert_eq!(value.stats.native_token(), 0x5152535455565758);
    assert_eq!(value.attack_rate.finite_value().unwrap().to_bits(), 0x3f8ccccd);
    assert_eq!(value.increased_leech_rate.finite_value(), Ok(-0.125));
    assert_eq!(fields::AVAILABLE_SIZE, 64);
    assert_eq!(fields::UNAVAILABLE_SIZE, 12);
}

#[test]
fn all_typed_unavailable_reasons_have_stable_literal_wire_tags() {
    use AttackLeechUnavailable::*;
    for (tag, reason) in [
        (1, OfflineRequired),
        (2, ManagerAbsent),
        (3, ManagerChanged),
        (4, ActorCacheAbsent),
        (5, StatsAbsent),
        (6, NativeObjectDestroyed),
        (7, GraphChanged),
        (8, UnsupportedStatsClass),
    ] {
        let literal = [0x31, 0x46, 0x47, 0x45, 1, 0, 0, 0, tag, 0, 0, 0];
        assert_eq!(fields::encode(&AttackLeechRead::Unavailable(reason)), literal);
        assert_eq!(fields::decode(&literal), Ok(AttackLeechRead::Unavailable(reason)));
    }
}

#[test]
fn strict_header_version_and_reserved_tags() {
    for (index, error) in [
        (0, ProtocolError::InvalidMagic),
        (1, ProtocolError::InvalidMagic),
        (2, ProtocolError::InvalidMagic),
        (3, ProtocolError::InvalidMagic),
        (7, ProtocolError::InvalidData),
    ] {
        let mut invalid = AVAILABLE;
        invalid[index] ^= 1;
        assert_eq!(fields::decode(&invalid), Err(error));
    }
    for version in [0_u16, 2, u16::MAX] {
        let mut invalid = AVAILABLE;
        invalid[4..6].copy_from_slice(&version.to_le_bytes());
        assert_eq!(fields::decode(&invalid), Err(ProtocolError::InvalidData));
    }
    for tag in [0, 2, 255] {
        let mut invalid = AVAILABLE;
        invalid[6] = tag;
        assert_eq!(fields::decode(&invalid), Err(ProtocolError::InvalidData));
    }
    for tag in [0_u32, 9, u32::MAX] {
        let mut invalid = [0x31, 0x46, 0x47, 0x45, 1, 0, 0, 0, 0, 0, 0, 0];
        invalid[8..12].copy_from_slice(&tag.to_le_bytes());
        assert_eq!(fields::decode(&invalid), Err(ProtocolError::InvalidData));
    }
}

#[test]
fn each_identity_component_must_be_nonzero_but_is_never_interpreted_as_a_pointer() {
    for offset in [8, 16, 24, 32, 40, 48] {
        let mut invalid = AVAILABLE;
        invalid[offset..offset + 8].fill(0);
        assert_eq!(fields::decode(&invalid), Err(ProtocolError::InvalidData));
    }
    assert_eq!(ObservedObjectIdentity::new(0, 1), Err(ProtocolError::InvalidData));
    assert_eq!(ObservedObjectIdentity::new(1, 0), Err(ProtocolError::InvalidData));
    let opaque = ObservedObjectIdentity::new(u64::MAX, u64::MAX).unwrap();
    assert_eq!((opaque.wrapper_token(), opaque.native_token()), (u64::MAX, u64::MAX));
}

#[test]
fn wire_preserves_every_raw_float_pattern_even_when_numeric_value_is_unavailable() {
    let AttackLeechRead::Available(mut value) = observation() else { unreachable!() };
    for bits in [0, 0x80000000, 1, 0x7f7fffff, 0x7f800000, 0xff800000, 0x7fc12345, 0x7f800001, u32::MAX] {
        value.attack_rate.raw_bits = bits;
        value.increased_leech_rate.raw_bits = bits;
        let payload = fields::encode(&AttackLeechRead::Available(value));
        assert_eq!(&payload[56..60], &bits.to_le_bytes());
        assert_eq!(fields::decode(&payload), Ok(AttackLeechRead::Available(value)));
    }
    for bits in [0x7f800000, 0xff800000, 0x7fc12345, 0x7f800001, u32::MAX] {
        assert_eq!(ObservedRate { raw_bits: bits }.finite_value(), Err(RateUnavailable::NonFinite));
    }
    for bits in [0, 0x80000000, 1, 0x7f7fffff] {
        assert_eq!(ObservedRate { raw_bits: bits }.finite_value().unwrap().to_bits(), bits);
    }
}

#[test]
fn original_float_minimum_and_one_ulp_edges_are_classified_without_clamping() {
    let minimum = ObservedRate { raw_bits: 0x3c23d70a };
    assert!((minimum.finite_value().unwrap() as f64) < 0.01_f64);
    assert_eq!(minimum.attack_editor_value(), Ok(0.01_f32));
    assert_eq!(ObservedRate { raw_bits: 0x3c23d709 }.attack_editor_value(), Err(RateUnavailable::OutsideEditorDomain));
    assert_eq!(ObservedRate { raw_bits: 20_f32.to_bits() }.attack_editor_value(), Ok(20.));
    let outside = ObservedRate { raw_bits: 20_f32.to_bits() + 1 };
    assert!(outside.finite_value().unwrap() > 20.);
    assert_eq!(outside.attack_editor_value(), Err(RateUnavailable::OutsideEditorDomain));
    assert_eq!(ObservedRate { raw_bits: (-1_f32).to_bits() }.leech_editor_value(), Ok(-1.));
    assert_eq!(ObservedRate { raw_bits: 100_f32.to_bits() }.leech_editor_value(), Ok(100.));
    assert_eq!(ObservedRate { raw_bits: 101_f32.to_bits() }.leech_editor_value(), Err(RateUnavailable::OutsideEditorDomain));
    assert_eq!(ObservedRate { raw_bits: 0x80000000 }.leech_editor_value().unwrap().to_bits(), 0x80000000);
}
