use eg_trainer_wire::{
    fields::{AttackLeechObservation, AttackLeechTarget, ObservedObjectIdentity, ObservedRate},
    protocol::{DispatcherSpec, ProtocolError, Request},
    rate_write::AttackLeechRateWrite,
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

/// The whole frame, independently specified as literal bytes rather than calling
/// the encoder or copying its field assembly.
fn golden() -> Vec<u8> {
    vec![
        0x35, 0x47, 0x46, 0x45, 24, 0, 0, 0, 85, 0, 0, 0, 0x80, 0x70, 0x60, 0x50, 0x40, 0x30, 0x20, 0x10, b'f', b'i', b'x', b't', b'u', b'r', b'e', b'-', b'p', b'r', b'o', b'o',
        b'f', b'-', b'1', b'6', 1, 0x0a, 0xd7, 0x23, 0x3c, 8, 7, 6, 5, 4, 3, 2, 1, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x28, 0x27, 0x26, 0x25, 0x24, 0x23, 0x22, 0x21,
        0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x48, 0x47, 0x46, 0x45, 0x44, 0x43, 0x42, 0x41, 0x58, 0x57, 0x56, 0x55, 0x54, 0x53, 0x52, 0x51, 0x45, 0x23, 0xc1, 0x7f, 0,
        0, 0, 0x80,
    ]
}
fn encode(write: &AttackLeechRateWrite) -> Vec<u8> {
    Request::SetAttackLeechRate(write.clone()).encode()
}
fn decode(bytes: &[u8]) -> Result<AttackLeechRateWrite, ProtocolError> {
    match Request::decode(bytes)? {
        Request::SetAttackLeechRate(write) => Ok(write),
        _ => Err(ProtocolError::InvalidData),
    }
}
fn patch32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn invalid(bytes: &[u8]) {
    assert!(Request::decode(bytes).is_err());
}

#[test]
fn independent_packed_golden_and_full_projection() {
    let write = write();
    let bytes = golden();
    assert_eq!(encode(&write), bytes);
    assert_eq!(decode(&bytes).unwrap(), write);
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
    assert_eq!(decode(&bytes), Err(ProtocolError::InvalidMagic));
}

#[test]
fn desired_value_is_refused_outside_its_own_target_domain_on_both_sides() {
    // The domains themselves are `ObservedRate`'s (fields.rs). Attack refuses 0, which leech accepts (signed zero below).
    for (target, tag, outside) in [(AttackLeechTarget::AttackRate, 1, 0.0_f32), (AttackLeechTarget::IncreasedLeechRate, 2, f32::NAN)] {
        assert!(AttackLeechRateWrite::new(dispatcher(), observation(), target, outside.to_bits()).is_err());
        let mut bytes = golden();
        bytes[36] = tag;
        patch32(&mut bytes, 37, outside.to_bits());
        invalid(&bytes);
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
            let restored = decode(&encode(&write)).unwrap();
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
