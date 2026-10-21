use eg_trainer_wire::{
    fields::{self, AttackLeechObservation, AttackLeechRead, AttackLeechTarget, ObservedObjectIdentity, ObservedRate},
    forge::{ForgeSnapshot, ItemStatus},
    protocol::{DispatcherSpec, ProtocolError, REQUEST_HEADER_SIZE, Request, RequestFrame},
    rate_write::AttackLeechRateWrite,
    write::ForgeWrite,
};

fn dispatcher() -> DispatcherSpec {
    DispatcherSpec {
        rva: 0x1020304050607080,
        bytes: *b"fixture-proof-16",
    }
}
fn observation() -> AttackLeechObservation {
    AttackLeechObservation {
        manager: ObservedObjectIdentity::new(0x0102030405060708, 0x1112131415161718).unwrap(),
        actor: ObservedObjectIdentity::new(0x2122232425262728, 0x3132333435363738).unwrap(),
        stats: ObservedObjectIdentity::new(0x4142434445464748, 0x5152535455565758).unwrap(),
        attack_rate: ObservedRate { raw_bits: 0x7fc12345 },
        increased_leech_rate: ObservedRate { raw_bits: 0x80000000 },
    }
}
fn write() -> AttackLeechRateWrite {
    AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::AttackRate, 0.01_f32.to_bits()).unwrap()
}

/// Independently specified prefix, authored as literal bytes rather than calling
/// the encoder or copying its field assembly; remaining240 reserved bytes are0.
fn golden() -> Vec<u8> {
    vec![
        0x35, 0x47, 0x46, 0x45, 24, 0, 0, 0, 85, 0, 0, 0, 0x80, 0x70, 0x60, 0x50, 0x40, 0x30, 0x20, 0x10, b'f', b'i', b'x', b't', b'u', b'r', b'e', b'-', b'p', b'r', b'o', b'o',
        b'f', b'-', b'1', b'6', 1, 0x0a, 0xd7, 0x23, 0x3c, 8, 7, 6, 5, 4, 3, 2, 1, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x28, 0x27, 0x26, 0x25, 0x24, 0x23, 0x22, 0x21,
        0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x48, 0x47, 0x46, 0x45, 0x44, 0x43, 0x42, 0x41, 0x58, 0x57, 0x56, 0x55, 0x54, 0x53, 0x52, 0x51, 0x45, 0x23, 0xc1, 0x7f, 0,
        0, 0, 0x80,
    ]
}
fn patch32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn invalid(bytes: &[u8]) {
    assert!(AttackLeechRateWrite::decode(bytes).is_err());
    assert!(Request::decode(bytes).is_err());
    if bytes.len() == REQUEST_HEADER_SIZE {
        assert!(Request::decode_header(bytes).is_err());
        assert!(RequestFrame::new(bytes).is_err());
    }
}

#[test]
fn independent_packed_golden_and_full_projection() {
    let write = write();
    let bytes = golden();
    assert_eq!(write.encode(), bytes);
    assert_eq!(AttackLeechRateWrite::decode(&bytes).unwrap(), write);
    let request = Request::SetAttackLeechRate(write.clone());
    assert_eq!(request.encode(), bytes);
    assert_eq!(Request::decode(&bytes).unwrap(), request);
    assert_eq!(write.expected_manager(), observation().manager.wrapper_token());
    assert_eq!(*write.expected(), observation());
    assert_eq!(write.target(), AttackLeechTarget::AttackRate);
    assert_eq!(write.desired_bits(), 0x3c23d70a);
    assert_eq!(write.desired_value(), 0.01_f32);
    let mut expected = observation();
    expected.attack_rate.raw_bits = 0x3c23d70a;
    assert_eq!(write.projected(), expected);
}

#[test]
fn declared_lengths_cannot_hide_extra_or_missing_data() {
    for length in [0, 23, 84, 86, u32::MAX] {
        let mut bytes = golden();
        patch32(&mut bytes, 8, length);
        assert!(Request::decode(&bytes).is_err());
    }
}

#[test]
fn unknown_target_zero_identities_and_wrong_operation_rejected() {
    for tag in [0, 3, 24, 255] {
        let mut bytes = golden();
        bytes[36] = tag;
        invalid(&bytes);
    }
    for at in [41, 49, 57, 65, 73, 81] {
        let mut bytes = golden();
        bytes[at..at + 8].fill(0);
        invalid(&bytes);
    }
    let mut bytes = golden();
    patch32(&mut bytes, 4, 25);
    invalid(&bytes);
    bytes = golden();
    bytes[0] = 0;
    assert_eq!(AttackLeechRateWrite::decode(&bytes), Err(ProtocolError::InvalidMagic));
}

#[test]
fn all_truncations_and_excess_rejected_and_frame_cannot_grow() {
    let bytes = golden();
    for end in 0..bytes.len() {
        assert!(Request::decode(&bytes[..end]).is_err());
    }
    let mut frame = RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE]).unwrap();
    assert!(frame.push(&bytes[REQUEST_HEADER_SIZE..]).is_ok());
    assert_eq!(frame.finish().unwrap(), Request::SetAttackLeechRate(write()));
    let mut frame = RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE]).unwrap();
    assert!(frame.push(&bytes[REQUEST_HEADER_SIZE..]).is_ok());
    assert!(frame.push(&[0]).is_err());
    assert!(frame.finish().is_err());
}

#[test]
fn desired_f32_domains_boundaries_and_nonfinite_patterns_are_strict() {
    for target in [AttackLeechTarget::AttackRate, AttackLeechTarget::IncreasedLeechRate] {
        for bits in [0x7f800000, 0xff800000, 0x7fc00001, 0x7f800001, 0xffc12345] {
            assert!(AttackLeechRateWrite::new(dispatcher(), observation(), target, bits).is_err());
            let mut bytes = golden();
            bytes[36] = if target == AttackLeechTarget::AttackRate { 1 } else { 2 };
            patch32(&mut bytes, 37, bits);
            invalid(&bytes);
        }
    }
    for value in [0.0_f32, -0.0, -1.0, f32::from_bits(0.01_f32.to_bits() - 1), f32::from_bits(20_f32.to_bits() + 1)] {
        assert!(AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::AttackRate, value.to_bits()).is_err());
    }
    for value in [0.01_f32, 20.0] {
        assert!(AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::AttackRate, value.to_bits()).is_ok());
    }
    for value in [f32::from_bits((-1_f32).to_bits() + 1), f32::from_bits(100_f32.to_bits() + 1)] {
        assert!(AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::IncreasedLeechRate, value.to_bits()).is_err());
    }
    for value in [-1_f32, 100.0] {
        assert!(AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::IncreasedLeechRate, value.to_bits()).is_ok());
    }
}

#[test]
fn old_raw_patterns_are_never_normalized_and_noop_means_bit_identical() {
    for bits in [0, 0x80000000, 0x7f800000, 0xff800000, 0x7fc12345, 0x7f800001, u32::MAX] {
        for target in [AttackLeechTarget::AttackRate, AttackLeechTarget::IncreasedLeechRate] {
            let mut expected = observation();
            expected.attack_rate.raw_bits = bits;
            expected.increased_leech_rate.raw_bits = bits;
            let write = AttackLeechRateWrite::new(dispatcher(), expected, target, 1_f32.to_bits()).unwrap();
            let restored = AttackLeechRateWrite::decode(&write.encode()).unwrap();
            assert_eq!(*restored.expected(), expected);
            let projected = restored.projected();
            if target == AttackLeechTarget::AttackRate {
                assert_eq!(projected.increased_leech_rate.raw_bits, bits);
            } else {
                assert_eq!(projected.attack_rate.raw_bits, bits);
            }
        }
    }
    let mut expected = observation();
    expected.attack_rate.raw_bits = 1_f32.to_bits();
    assert!(AttackLeechRateWrite::new(dispatcher(), expected, AttackLeechTarget::AttackRate, 1_f32.to_bits()).is_err());
    let mut bytes = golden();
    patch32(&mut bytes, 89, 0x3c23d70a);
    invalid(&bytes);
    let signed = AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::IncreasedLeechRate, 0).unwrap();
    assert_eq!(signed.projected().increased_leech_rate.raw_bits, 0);
    assert_eq!(signed.expected().increased_leech_rate.raw_bits, 0x80000000);
    assert!(AttackLeechRateWrite::new(dispatcher(), signed.projected(), AttackLeechTarget::IncreasedLeechRate, 0).is_err());
    // No consumed request state: an identical constructor is valid again when
    // baseline bits are restored. Delivery certainty belongs to the outer actor.
    assert_eq!(
        AttackLeechRateWrite::new(dispatcher(), observation(), AttackLeechTarget::IncreasedLeechRate, 0).unwrap(),
        signed
    );
}

#[test]
fn dispatcher_is_exact_caller_proof_data_not_an_admission_assertion() {
    let request = Request::decode(&golden()).unwrap();
    assert_eq!(request.dispatcher(), Some(dispatcher()));
    assert_eq!(Request::Discover.dispatcher(), None);
    let mut bytes = golden();
    bytes[12..36].fill(0);
    let request = Request::decode(&bytes).unwrap();
    assert_eq!(request.dispatcher(), Some(DispatcherSpec { rva: 0, bytes: [0; 16] }));
    // Structurally valid zero/mismatched proof is left to the source-bound host
    // and helper admission checks; wire decoding must not claim runtime authority.
    for offset in 12..36 {
        let mut bytes = golden();
        bytes[offset] ^= 0x80;
        assert_eq!(Request::decode(&bytes).unwrap().encode(), bytes);
    }
}

#[test]
fn read_messages_keep_typed_observations_with_a_compact_request() {
    let request = Request::ReadAttackLeech {
        dispatcher: dispatcher(),
        expected_manager: 7,
    };
    let bytes = request.encode();
    assert_eq!(bytes.len(), 44);
    assert_eq!(&bytes[4..8], &23_u32.to_le_bytes());
    assert_eq!(&bytes[36..44], &7u64.to_le_bytes());
    assert_eq!(Request::decode(&bytes).unwrap(), request);
    let bytes = fields::encode(&AttackLeechRead::Available(observation()));
    assert_eq!(bytes.len(), 64);
    assert_eq!(&bytes[4..6], &1_u16.to_le_bytes());
    assert_eq!(fields::decode(&bytes).unwrap(), AttackLeechRead::Available(observation()));
}

#[test]
fn status_bounded_tail_header_cannot_submit_without_complete_expected_item() {
    let mut forge = ForgeSnapshot {
        item: 1,
        individual_id: 2,
        fingerprint: 0,
        name: "owned".into(),
        item_type: 3,
        sub_type: 4,
        rarity: 0,
        class_requirement: 0,
        affixes: vec![],
        implicit_rolls: vec![],
        identity: None,
        status: ItemStatus {
            forging_potential: 1,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 1,
        },
    };
    forge.fingerprint = forge.legacy_fingerprint();
    let request = Request::EditForge(ForgeWrite::level_requirement(dispatcher(), 7, forge, 2).unwrap());
    let bytes = request.encode();
    let header = Request::decode_header(&bytes[..REQUEST_HEADER_SIZE]).unwrap();
    assert!(header.body_len() > 0);
    assert!(Request::decode(&bytes[..REQUEST_HEADER_SIZE]).is_err());
    assert!(RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE]).unwrap().finish().is_err());
    let mut frame = RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE]).unwrap();
    frame.push(&bytes[REQUEST_HEADER_SIZE..]).unwrap();
    assert_eq!(frame.finish().unwrap(), request);
}
