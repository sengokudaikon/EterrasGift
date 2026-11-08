use eg_trainer_wire::protocol::RequestFrame as FrameAssembler;
use eg_trainer_wire::write::*;
use eg_trainer_wire::{
    forge::{self, AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    protocol::{DispatcherSpec, MAGIC, ProtocolError, REQUEST_HEADER_SIZE, Request, Response, ResponseCode},
};
fn decode_header(bytes: &[u8]) -> Result<eg_trainer_wire::protocol::RequestHeader, ProtocolError> {
    Request::decode_header(bytes)
}
fn decode_frame(bytes: &[u8]) -> Result<Request, ProtocolError> {
    Request::decode(bytes)
}
fn frame(write: &ForgeWrite) -> Vec<u8> {
    Request::EditForge(write.clone()).encode()
}
fn spec() -> DispatcherSpec {
    DispatcherSpec { rva: 0x1000, bytes: [0xcc; 16] }
}
fn snapshot() -> ForgeSnapshot {
    ForgeSnapshot {
        item: 0x1122334455667788,
        individual_id: 0xaabbccdd,
        fingerprint: 0x351c5afa7067984c,
        name: "A".into(),
        item_type: 4,
        sub_type: 257,
        rarity: 2,
        class_requirement: -7,
        affixes: vec![],
        implicit_rolls: vec![],
        identity: None,
        status: ItemStatus {
            forging_potential: 3,
            forging_potential_type: -1,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 42,
        },
    }
}
fn write() -> ForgeWrite {
    ForgeWrite::level_requirement(spec(), 0x1111222233334444, snapshot(), 80).unwrap()
}
fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn tail_literal() -> Vec<u8> {
    hex(concat!(
        "018877665544332211ddccbbaa4c986770fa5a1c35010041",
        "04010102f9ffffff00000003ffffffff00000000002a000000"
    ))
}
fn literal() -> Vec<u8> {
    let mut bytes = hex("354746450a000000580000000010000000000000cccccccccccccccccccccccccccccccc444433332222111105500031000000");
    bytes.extend(tail_literal());
    bytes
}
fn set32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
#[test]
fn independent_literal_full_snapshot_wire_and_infallible_owned_encoder() {
    assert_eq!(forge::encode(Some(&snapshot())).unwrap(), tail_literal());
    assert_eq!(frame(&write()), literal());
    assert_eq!(decode_frame(&literal()), Ok(Request::EditForge(write())));
    let header = decode_header(&literal()[..REQUEST_HEADER_SIZE]).unwrap();
    assert_eq!(header.body_len(), 88);
    assert_eq!(header.total_len(), 100);
}

#[test]
fn envelope_and_typed_write_fields_are_bounded() {
    for (at, value) in [(0, 0u32), (4, 3), (8, u32::MAX)] {
        let mut bytes = literal();
        set32(&mut bytes, at, value);
        assert!(decode_header(&bytes[..REQUEST_HEADER_SIZE]).is_err());
    }
    for (at, value) in [(44, 255), (46, 2)] {
        let mut bytes = literal();
        bytes[at] = value;
        assert!(decode_frame(&bytes).is_err());
    }
    let mut bytes = literal();
    bytes[36..44].fill(0);
    assert!(decode_frame(&bytes).is_err());
    assert!(ForgeWrite::level_requirement(spec(), 0, snapshot(), 80).is_err());
    for value in [-1, 256, i32::MIN, i32::MAX] {
        assert!(ForgeWrite::level_requirement(spec(), 1, snapshot(), value).is_err());
    }
}

#[test]
fn assembler_accepts_arbitrary_fragmentation_only_after_complete_decode() {
    let frame = literal();
    for chunk in [1, 7, 49] {
        let mut assembler = FrameAssembler::new(&frame[..REQUEST_HEADER_SIZE]).unwrap();
        for bytes in frame[REQUEST_HEADER_SIZE..].chunks(chunk) {
            assembler.push(bytes).unwrap();
        }
        assert_eq!(assembler.remaining(), 0);
        assert_eq!(assembler.finish().unwrap(), Request::EditForge(write()));
    }
}
#[test]
fn seen_excess_poisoned() {
    let mut frame = literal();
    frame.push(0);
    let mut assembler = FrameAssembler::new(&frame[..REQUEST_HEADER_SIZE]).unwrap();
    assert!(assembler.push(&frame[REQUEST_HEADER_SIZE..]).is_err());
    assert!(assembler.push(&tail_literal()).is_err());
    assert!(assembler.finish().is_err());
}
#[test]
fn strict_expected_presence_utf8_bools_and_fingerprint() {
    for offset in [0, 13, 23, 43, 44] {
        let mut bytes = literal();
        bytes[51 + offset] = if offset == 23 { 0xff } else { 2 };
        assert!(decode_frame(&bytes).is_err());
    }
    let mut malformed = snapshot();
    malformed.fingerprint ^= 1;
    assert!(ForgeWrite::level_requirement(spec(), 1, malformed, 80).is_err());
    let mut bytes = literal();
    set32(&mut bytes, 47, u32::MAX);
    assert!(decode_frame(&bytes).is_err());
}

#[test]
fn maximum_schema_bound_is_exact_and_allows_large_inherited_ids_without_rebuild() {
    let mut maximum = snapshot();
    maximum.name = "x".repeat(forge::MAX_NAME_BYTES);
    maximum.affixes = (0..forge::MAX_AFFIXES)
        .map(|slot| AffixSnapshot {
            slot: slot as u8,
            id: u16::MAX,
            tier: 256,
            roll: 255,
            kind: i32::MIN,
            seal: i32::MAX,
            name: "x".repeat(forge::MAX_NAME_BYTES),
            special: i32::MIN,
        })
        .collect();
    maximum.implicit_rolls = vec![255; 3];
    maximum.rarity = 9;
    maximum.identity = Some(UniqueIdentity {
        id: u16::MAX,
        rolls: vec![255; 8],
    });
    maximum.fingerprint = maximum.legacy_fingerprint();
    assert_eq!(forge::encode(Some(&maximum)).unwrap().len(), MAX_EXPECTED_BYTES);
    assert_eq!(MAX_EXPECTED_BYTES, 135870);
    // Wire decoding is not eligibility: native must refuse unique-backed items before store.
    let request = ForgeWrite::level_requirement(spec(), 1, maximum.clone(), 0).unwrap();
    assert_eq!(frame(&request).len(), MAX_EXPECTED_BYTES + 51);
    assert_eq!(decode_frame(&frame(&request)).unwrap(), Request::EditForge(request));
    maximum.name.push('x');
    assert!(ForgeWrite::level_requirement(spec(), 1, maximum, 0).is_err());
    let mut oversized = vec![0; MAX_REQUEST_BYTES + 1];
    oversized[..4].copy_from_slice(&MAGIC.to_le_bytes());
    assert!(decode_frame(&oversized).is_err());
}
#[test]
fn minimal_schema_bound_is_exact() {
    let mut item = snapshot();
    item.name.clear();
    assert_eq!(forge::encode(Some(&item)).unwrap().len(), MIN_EXPECTED_BYTES);
    let request = ForgeWrite::level_requirement(spec(), 1, item, 0).unwrap();
    assert!(decode_frame(&frame(&request)).is_ok());
    assert_eq!(REQUEST_HEADER_SIZE, 12);
}
#[test]
fn projection_changes_only_requested_level_and_its_legacy_fingerprint() {
    let request = write();
    let mut expected = snapshot();
    expected.status.level_requirement = 80;
    expected.fingerprint = expected.legacy_fingerprint();
    assert_eq!(request.projected(), expected);
    assert_ne!(request.projected().fingerprint, snapshot().fingerprint);
    assert_eq!(request.expected(), &snapshot());
}

#[test]
fn status_fields_have_one_strict_frame_and_boolean_values_are_canonical() {
    for (field, value) in [
        (StatusField::ForgingPotential, 211),
        (StatusField::Corrupted, 1),
        (StatusField::Ruined, 0),
        (StatusField::LevelRequirement, 17),
        (StatusField::LegendaryPotential, 6),
        (StatusField::WeaversWill, 49),
        (StatusField::WeaversTouch, 77),
    ] {
        let request = ForgeWrite::new(spec(), 1, snapshot(), field, value).unwrap();
        let bytes = frame(&request);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 10);
        assert_eq!(bytes[44], field as u8);
        assert_eq!(Request::decode(&bytes).unwrap(), Request::EditForge(request.clone()));
        assert_eq!(field.value(&request.projected()), value);
    }
    for field in [StatusField::Corrupted, StatusField::Ruined] {
        assert!(ForgeWrite::new(spec(), 1, snapshot(), field, 2).is_err());
        let mut invalid = frame(&write());
        invalid[44] = field as u8;
        assert!(Request::decode(&invalid).is_err());
    }
    for field in [7, u32::MAX] {
        let mut invalid = frame(&write());
        invalid[44] = field as u8;
        assert!(Request::decode(&invalid).is_err());
    }
}

#[test]
fn override_is_one_explicit_bit_and_all_other_flag_bits_are_rejected() {
    let request = ForgeWrite::with_override(spec(), 1, snapshot(), StatusField::WeaversTouch, 77, true).unwrap();
    let bytes = frame(&request);
    assert_eq!(bytes[46], 1);
    assert_eq!(Request::decode(&bytes).unwrap(), Request::EditForge(request));
    assert!(!write().unrestricted());
    for flags in [2u8, 3, 128, 255] {
        let mut invalid = bytes.clone();
        invalid[46] = flags;
        assert!(Request::decode(&invalid).is_err());
    }
}

#[test]
fn potential_and_weaver_rules_keep_literal_bounds_identity_and_override_distinct() {
    use eg_trainer_wire::definitions::{ItemLimits, ValueRange};
    let limits = ItemLimits {
        forging_potential: ValueRange { minimum: 3, maximum: 211 },
        legendary_potential: ValueRange { minimum: 1, maximum: 6 },
        weavers_will: ValueRange { minimum: 2, maximum: 49 },
        weavers_touch: ValueRange { minimum: 4, maximum: 77 },
        level_requirement: ValueRange { minimum: 0, maximum: 137 },
    };
    let mut item = snapshot();
    item.rarity = 7;
    for (field, value, flag, definition, expected) in [
        (StatusField::LegendaryPotential, 6, false, Some(0), true),
        (StatusField::LegendaryPotential, 0, false, Some(0), false),
        (StatusField::LegendaryPotential, 7, true, Some(0), false),
        (StatusField::LegendaryPotential, 1, true, Some(1), false),
        (StatusField::WeaversWill, 49, false, Some(1), true),
        (StatusField::WeaversWill, 0, false, Some(1), true),
        (StatusField::WeaversWill, 1, false, Some(1), true),
        (StatusField::WeaversWill, 50, false, Some(1), false),
        (StatusField::WeaversWill, 255, true, Some(1), true),
        (StatusField::WeaversWill, 10, false, Some(0), false),
        (StatusField::WeaversWill, 10, true, Some(0), true),
        (StatusField::WeaversWill, 0, true, Some(0), true),
        (StatusField::WeaversWill, 255, true, Some(0), true),
        (StatusField::WeaversWill, -1, true, Some(0), false),
        (StatusField::WeaversWill, 256, true, Some(0), false),
        (StatusField::WeaversWill, 10, true, None, false),
        (StatusField::WeaversWill, 10, true, Some(99), false),
    ] {
        assert_eq!(field.allows(&item, &limits, value, flag, definition), expected, "{field:?}/{value}/{flag}/{definition:?}");
    }
    item.status.legendary_potential = 1;
    assert!(!StatusField::WeaversWill.allows(&item, &limits, 10, true, Some(0)));
    item.status.legendary_potential = 0;
    item.status.weavers_will = 2;
    assert!(!StatusField::LegendaryPotential.allows(&item, &limits, 1, true, Some(0)));
    item.rarity = 8;
    assert!(!StatusField::WeaversWill.allows(&item, &limits, 10, true, Some(1)));
    item.rarity = 9;
    item.status.weavers_will = 80;
    item.status.legendary_potential = 1;
    assert!(StatusField::WeaversWill.allows(&item, &limits, 79, true, None));
    assert!(StatusField::WeaversWill.allows(&item, &limits, 255, true, None));
    item.status.weavers_will = 0;
    assert!(!StatusField::WeaversWill.allows(&item, &limits, 10, true, None));
    item.item_type = 25;
    item.status.weavers_touch = 1;
    for (value, flag, expected) in [
        (0, true, true),
        (1, true, true),
        (4, true, true),
        (77, true, true),
        (78, true, false),
        (255, true, false),
        (-1, true, false),
        (256, true, false),
        (1, false, false),
    ] {
        assert_eq!(StatusField::WeaversTouch.allows(&item, &limits, value, flag, None), expected, "{value}/{flag}");
    }
    item.status.weavers_touch = 200;
    for (value, expected) in [(0, true), (77, true), (150, true), (200, true), (201, false)] {
        assert_eq!(StatusField::WeaversTouch.allows(&item, &limits, value, true, None), expected, "held 200/{value}");
    }
    item.status.weavers_touch = 1;
    item.item_type = 24;
    assert!(!StatusField::WeaversTouch.allows(&item, &limits, 4, true, None));
}
fn done(item: Option<&ForgeSnapshot>) -> Response {
    Response {
        code: ResponseCode::Done,
        payload: forge::encode(item).unwrap(),
    }
}
#[test]
fn native_ack_is_provisional_but_replaced_target_is_unknown() {
    let request = write();
    assert_eq!(classify_response(&done(Some(&request.projected())), &request), WriteReply::Confirmed(request.projected()));
    assert_eq!(classify_response(&done(Some(&snapshot())), &request), WriteReply::Confirmed(snapshot()));
    assert!(matches!(classify_response(&done(None), &request), WriteReply::Uncertain { .. }));
    for field in 0..5 {
        let mut actual = request.projected();
        match field {
            0 => actual.name.push('!'),
            1 => actual.individual_id += 1,
            2 => actual.class_requirement += 1,
            3 => actual.item += 1,
            _ => actual.status.corrupted = true,
        }
        actual.fingerprint = actual.legacy_fingerprint();
        assert_eq!(
            matches!(classify_response(&done(Some(&actual)), &request), WriteReply::Uncertain { .. }),
            matches!(field, 1 | 3)
        );
    }
}
#[test]
fn damaged_or_oversized_done_is_uncertain_after_possible_store() {
    let request = write();
    let frame = done(Some(&request.projected()));
    for end in 0..frame.payload.len() {
        assert!(matches!(
            classify_response(
                &Response {
                    payload: frame.payload[..end].to_vec(),
                    ..frame.clone()
                },
                &request
            ),
            WriteReply::Uncertain { .. }
        ));
    }
    let mut extra = frame;
    extra.payload.push(0);
    assert!(matches!(classify_response(&extra, &request), WriteReply::Uncertain { .. }));
    assert!(matches!(
        classify_response(
            &Response {
                code: ResponseCode::Done,
                payload: vec![0; eg_trainer_wire::protocol::MAX_RESPONSE_SIZE + 1]
            },
            &request
        ),
        WriteReply::Uncertain { .. }
    ));
}
#[test]
fn trusted_prestore_codes_are_notapplied_but_never_authorize_replay() {
    let request = write();
    for code in [
        ResponseCode::NotReady,
        ResponseCode::StaleItem,
        ResponseCode::Refused,
        ResponseCode::OfflineRequired,
        ResponseCode::RestartRequired,
    ] {
        let response = Response {
            code,
            payload: b"nothing ran".to_vec(),
        };
        assert_eq!(
            classify_response(&response, &request),
            WriteReply::NotApplied {
                code,
                message: "nothing ran".into()
            }
        );
        for payload in [vec![0xff], vec![b'x'; MAX_ERROR_TEXT + 1]] {
            assert!(matches!(classify_response(&Response { code, payload }, &request), WriteReply::Uncertain { .. }));
        }
    }
    assert!(matches!(
        classify_response(
            &Response {
                code: ResponseCode::Failed,
                payload: b"post-store read failed".to_vec()
            },
            &request
        ),
        WriteReply::Uncertain { .. }
    ));
}

#[test]
fn async_reader_does_not_return_request_on_header_only_disconnect() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        use tokio::io::AsyncWriteExt;
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let task = tokio::spawn(async move { eg_trainer_wire::transport::read_request(&mut reader).await });
        writer.write_all(&frame(&write())[..REQUEST_HEADER_SIZE]).await.unwrap();
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        drop(writer);
        assert!(matches!(task.await.unwrap(), Err(eg_trainer_wire::transport::RequestReadError::Io(_))));
    });
}
#[test]
fn async_reader_rejects_unbounded_header_without_waiting_for_body() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        use tokio::io::AsyncWriteExt;
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let task = tokio::spawn(async move { eg_trainer_wire::transport::read_request(&mut reader).await });
        let mut header = literal()[..REQUEST_HEADER_SIZE].to_vec();
        set32(&mut header, 8, u32::MAX);
        writer.write_all(&header).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task).await.unwrap().unwrap();
        assert!(matches!(result, Err(eg_trainer_wire::transport::RequestReadError::Protocol(_))));
    });
}

#[test]
fn every_mutation_roundtrips_with_no_unused_slots() {
    let mutations = [
        ForgeMutation::Base {
            item_type: 255,
            sub_type: 65535,
            unrestricted: true,
        },
        ForgeMutation::Identity {
            id: 65535,
            sub_type: 255,
            rarity: 9,
            unrestricted: true,
        },
        ForgeMutation::Affix {
            slot: 31,
            id: 4095,
            tier: 8,
            roll: 255,
            seal: 3,
            unrestricted: true,
        },
        ForgeMutation::AddAffix {
            id: 4095,
            tier: 8,
            roll: 255,
            seal: 3,
            unrestricted: true,
        },
        ForgeMutation::RemoveAffix {
            slot: 31,
            id: 65535,
            restore_potential: None,
        },
    ];
    for mutation in mutations {
        let write = ForgeWrite::edit(spec(), 1, snapshot(), mutation).unwrap();
        let bytes = frame(&write);
        assert_eq!(Request::decode(&bytes), Ok(Request::EditForge(write)));
    }
    for mutation in [
        ForgeMutation::Identity {
            id: 1,
            sub_type: 0,
            rarity: 6,
            unrestricted: false,
        },
        ForgeMutation::Affix {
            slot: 32,
            id: 0,
            tier: 1,
            roll: 0,
            seal: 0,
            unrestricted: false,
        },
        ForgeMutation::AddAffix {
            id: 4096,
            tier: 0,
            roll: 0,
            seal: 4,
            unrestricted: false,
        },
        ForgeMutation::RemoveAffix {
            slot: 32,
            id: 0,
            restore_potential: None,
        },
    ] {
        assert!(ForgeWrite::edit(spec(), 1, snapshot(), mutation).is_err());
    }
}
#[test]
fn roll_batches_bound_count_and_reject_duplicate_or_out_of_range_slots() {
    for count in [1, 2, 32] {
        let changes = (0..count).map(|slot| AffixRollChange { slot, value: 255 }).collect::<Vec<_>>();
        let mutation = ForgeMutation::AffixRolls {
            rolls: AffixRolls::new(&changes).unwrap(),
            unrestricted: true,
        };
        let write = ForgeWrite::edit(spec(), 1, snapshot(), mutation).unwrap();
        let bytes = frame(&write);
        assert_eq!(Request::decode(&bytes), Ok(Request::EditForge(write)));
        for (at, value) in [(44, 2), (45, 0), (45, 33), (46, 32)] {
            let mut bad = bytes.clone();
            bad[at] = value;
            assert!(Request::decode(&bad).is_err());
        }
        if count > 1 {
            let mut bad = bytes;
            bad[48] = 0;
            assert!(Request::decode(&bad).is_err());
        }
    }
    assert!(AffixRolls::new(&[]).is_err());
    assert!(AffixRolls::new(&[AffixRollChange { slot: 0, value: 1 }; 33]).is_err());
    assert!(AffixRolls::new(&[AffixRollChange { slot: 0, value: 1 }; 2]).is_err());
}

#[test]
fn unique_and_implicit_rolls_round_trip_project_and_stay_inside_their_arrays() {
    let mut item = snapshot();
    item.rarity = 8;
    item.implicit_rolls = vec![10, 20];
    item.identity = Some(UniqueIdentity { id: 5, rolls: vec![1, 2, 3] });
    item.fingerprint = item.legacy_fingerprint();
    let rolls = |slot| AffixRolls::new(&[AffixRollChange { slot, value: 200 }]).unwrap();
    for (array, slot) in [(RollArray::Unique, 2), (RollArray::Implicit, 1)] {
        let mutation = ForgeMutation::ModifierRolls {
            array,
            rolls: rolls(slot),
            unrestricted: false,
        };
        let write = ForgeWrite::edit(spec(), 1, item.clone(), mutation).unwrap();
        assert_eq!(Request::decode(&frame(&write)), Ok(Request::EditForge(write.clone())));
        let projected = write.projected();
        match array {
            RollArray::Unique => assert_eq!(projected.identity.unwrap().rolls, [1, 2, 200]),
            RollArray::Implicit => assert_eq!(projected.implicit_rolls, [10, 200]),
        }
    }
    // An index past the game's array is refused before any transport.
    for (array, slot) in [(RollArray::Unique, 8), (RollArray::Implicit, 3)] {
        let mutation = ForgeMutation::ModifierRolls {
            array,
            rolls: rolls(slot),
            unrestricted: false,
        };
        assert!(ForgeWrite::edit(spec(), 1, item.clone(), mutation).is_err());
    }
    assert!(RollArray::try_from(0).is_err() && RollArray::try_from(3).is_err());
}
