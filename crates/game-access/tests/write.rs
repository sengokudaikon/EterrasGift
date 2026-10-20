use eg_game_access::protocol::RequestFrame as FrameAssembler;
use eg_game_access::write::*;
use eg_game_access::{
    catalog::CatalogKind,
    forge::{self, AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    protocol::{DispatcherSpec, MAGIC, ProtocolError, REQUEST_SIZE, Request, Response, ResponseCode},
};
fn decode_header(bytes: &[u8]) -> Result<eg_game_access::protocol::RequestHeader, ProtocolError> {
    Request::decode_header(bytes)
}
fn decode_frame(bytes: &[u8]) -> Result<Request, ProtocolError> {
    Request::decode(bytes)
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
    let mut frame = vec![0; 336];
    frame[..24].copy_from_slice(&[
        0x35, 0x47, 0x46, 0x45, 10, 0, 0, 0, 80, 0, 0, 0, 49, 0, 0, 0, 0x44, 0x44, 0x33, 0x33, 0x22, 0x22, 0x11, 0x11,
    ]);
    frame[24..28].copy_from_slice(&5_u32.to_le_bytes());
    frame[48..56].copy_from_slice(&[0, 0x10, 0, 0, 0, 0, 0, 0]);
    frame[56..72].fill(0xcc);
    frame.extend_from_slice(&tail_literal());
    frame
}
fn set32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
#[test]
fn independent_literal_full_snapshot_wire_and_infallible_owned_encoder() {
    assert_eq!(snapshot().legacy_fingerprint(), 0x351c5afa7067984c);
    assert_eq!(tail_literal().len(), 49);
    assert_eq!(forge::encode(Some(&snapshot())).unwrap(), tail_literal());
    assert_eq!(write().encode(), literal());
    assert_eq!(decode_frame(&literal()).unwrap(), Request::EditForge(write()));
    let layout = decode_header(&literal()[..336]).unwrap();
    assert_eq!(layout.tail_len(), 49);
    assert_eq!(layout.total_len(), 385);
    let mut original = snapshot();
    let request = ForgeWrite::level_requirement(spec(), 4, original.clone(), 255).unwrap();
    original.name = "changed after construction".into();
    assert_eq!(request.expected().name, "A");
    assert_eq!(request.projected().status.level_requirement, 255);
    assert_eq!(request.expected_manager(), 4);
    assert_eq!(request.dispatcher(), spec());
    assert!(decode_frame(&request.clone().encode()).is_ok());
}
#[test]
fn every_header_reserved_byte_is_strict_and_known_fields_are_bounded() {
    let bytes = literal();
    for at in (32..48).chain(72..336) {
        let mut bad = bytes.clone();
        bad[at] = 1;
        assert!(decode_header(&bad[..336]).is_err(), "reserved {at}");
        assert!(decode_frame(&bad).is_err());
    }
    for (at, value) in [
        (0, 0),
        (4, 23),
        (8, u32::MAX),
        (8, 256),
        (12, 0),
        (12, 47),
        (12, MAX_EXPECTED_BYTES as u32 + 1),
        (12, u32::MAX),
    ] {
        let mut bad = bytes.clone();
        set32(&mut bad, at, value);
        assert!(decode_header(&bad[..336]).is_err(), "at {at} value {value}");
    }
    let mut bad = bytes;
    bad[16..24].fill(0);
    assert!(decode_header(&bad[..336]).is_err());
    assert!(ForgeWrite::level_requirement(spec(), 0, snapshot(), 80).is_err());
    for value in [-1, 256, i32::MIN, i32::MAX] {
        assert!(ForgeWrite::level_requirement(spec(), 1, snapshot(), value).is_err());
    }
    for value in [0, 255] {
        assert!(ForgeWrite::level_requirement(spec(), u64::MAX, snapshot(), value).is_ok());
    }
}
#[test]
fn no_header_only_or_partial_tail_can_be_a_request() {
    let frame = literal();
    for end in 0..frame.len() {
        assert!(decode_frame(&frame[..end]).is_err(), "prefix {end}");
    }
    let assembler = FrameAssembler::new(&frame[..336]).unwrap();
    assert_eq!(assembler.remaining(), 49);
    assert!(assembler.finish().is_err());
    for count in 0..49 {
        let mut assembler = FrameAssembler::new(&frame[..336]).unwrap();
        assembler.push(&frame[336..336 + count]).unwrap();
        assert_eq!(assembler.remaining(), 49 - count);
        assert!(assembler.finish().is_err());
    }
}
#[test]
fn assembler_accepts_arbitrary_fragmentation_only_after_complete_decode() {
    let frame = literal();
    for chunk in 1..=49 {
        let mut assembler = FrameAssembler::new(&frame[..336]).unwrap();
        for bytes in frame[336..].chunks(chunk) {
            assembler.push(bytes).unwrap();
        }
        assert_eq!(assembler.remaining(), 0);
        assert_eq!(assembler.finish().unwrap(), Request::EditForge(write()));
    }
}
#[test]
fn seen_excess_poisoned_and_control_requests_do_not_gain_a_tail() {
    let mut frame = literal();
    frame.push(0);
    assert!(decode_frame(&frame).is_err());
    let mut assembler = FrameAssembler::new(&frame[..336]).unwrap();
    assert!(assembler.push(&frame[336..]).is_err());
    assert!(assembler.push(&tail_literal()).is_err());
    assert!(assembler.finish().is_err());
    let controls = [
        Request::Discover,
        Request::Status { dispatcher: spec() },
        Request::ClearHooks { dispatcher: spec() },
        Request::ReadForge { dispatcher: spec() },
        Request::ReadItemLimits { dispatcher: spec() },
        Request::ReadLocale { dispatcher: spec() },
        Request::ReadCatalogPage {
            dispatcher: spec(),
            kind: CatalogKind::Affixes,
            cursor: 16,
        },
    ];
    for request in controls {
        let bytes = request.encode();
        assert_eq!(decode_header(&bytes).unwrap().tail_len(), 0);
        assert_eq!(FrameAssembler::new(&bytes).unwrap().finish().unwrap(), request.clone());
        let mut extra = bytes;
        extra.push(0);
        assert!(decode_frame(&extra).is_err());
    }
}
#[test]
fn strict_expected_presence_utf8_bools_and_fingerprint() {
    for at in [336, 336 + 13, 336 + 23, 336 + 43, 336 + 44] {
        let mut frame = literal();
        frame[at] = if at == 336 + 23 { 0xff } else { 2 };
        assert!(decode_frame(&frame).is_err(), "at {at}");
    }
    let mut frame = literal();
    frame[336..].fill(0);
    assert!(decode_frame(&frame).is_err());
    let mut invalid = snapshot();
    invalid.fingerprint ^= 1;
    assert_eq!(ForgeWrite::level_requirement(spec(), 1, invalid, 80), Err(ProtocolError::InvalidData));
    let mut frame = literal();
    set32(&mut frame, 12, 1);
    frame.truncate(337);
    frame[336] = 0;
    assert!(decode_frame(&frame).is_err());
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
    assert_eq!(request.encode().len(), MAX_REQUEST_BYTES);
    assert_eq!(decode_frame(&request.encode()).unwrap(), Request::EditForge(request));
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
    assert!(decode_frame(&request.encode()).is_ok());
    assert_eq!(REQUEST_SIZE, 336);
    assert_eq!(eg_game_access::protocol::VERSION, 37);
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
        let bytes = request.encode();
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 10);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), field as u32);
        assert_eq!(Request::decode(&bytes).unwrap(), Request::EditForge(request.clone()));
        assert_eq!(field.value(&request.projected()), value);
    }
    for field in [StatusField::Corrupted, StatusField::Ruined] {
        assert!(ForgeWrite::new(spec(), 1, snapshot(), field, 2).is_err());
        let mut invalid = write().encode();
        set32(&mut invalid, 24, field as u32);
        assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err());
    }
    for field in [7, u32::MAX] {
        let mut invalid = write().encode();
        set32(&mut invalid, 24, field);
        assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err());
    }
}

#[test]
fn override_is_one_explicit_bit_and_all_other_flag_bits_are_rejected() {
    let request = ForgeWrite::with_override(spec(), 1, snapshot(), StatusField::WeaversTouch, 77, true).unwrap();
    let bytes = request.encode();
    assert_eq!(&bytes[28..32], &1_u32.to_le_bytes());
    assert_eq!(Request::decode(&bytes).unwrap(), Request::EditForge(request));
    assert!(!write().unrestricted());
    for flags in [2, 3, 256, 65536, u32::MAX] {
        let mut invalid = bytes.clone();
        set32(&mut invalid, 28, flags);
        assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err());
    }
}

#[test]
fn potential_and_weaver_rules_keep_literal_bounds_identity_and_override_distinct() {
    use eg_game_access::definitions::{ItemLimits, ValueRange};
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
        (StatusField::WeaversWill, 0, false, Some(1), false),
        (StatusField::WeaversWill, 10, false, Some(0), false),
        (StatusField::WeaversWill, 10, true, Some(0), true),
        (StatusField::WeaversWill, 0, true, Some(0), true),
        (StatusField::WeaversWill, 50, true, Some(0), false),
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
    assert!(!StatusField::WeaversWill.allows(&item, &limits, 81, true, None));
    item.status.weavers_will = 0;
    assert!(!StatusField::WeaversWill.allows(&item, &limits, 10, true, None));
    item.item_type = 25;
    for (value, flag, expected) in [(0, true, true), (4, true, true), (77, true, true), (3, true, false), (78, true, false), (4, false, false)] {
        assert_eq!(StatusField::WeaversTouch.allows(&item, &limits, value, flag, None), expected);
    }
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
                payload: vec![0; eg_game_access::protocol::MAX_RESPONSE_SIZE + 1]
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
        let task = tokio::spawn(async move { eg_game_access::transport::read_request(&mut reader).await });
        writer.write_all(&write().encode()[..336]).await.unwrap();
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        drop(writer);
        assert!(matches!(task.await.unwrap(), Err(eg_game_access::transport::RequestReadError::Io(_))));
    });
}
#[test]
fn async_reader_rejects_unbounded_header_without_waiting_for_body() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        use tokio::io::AsyncWriteExt;
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let task = tokio::spawn(async move { eg_game_access::transport::read_request(&mut reader).await });
        let mut header = literal()[..336].to_vec();
        set32(&mut header, 12, u32::MAX);
        writer.write_all(&header).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task).await.unwrap().unwrap();
        assert!(matches!(result, Err(eg_game_access::transport::RequestReadError::Protocol(_))));
    });
}

#[test]
fn base_and_identity_share_bounded_frames_with_exact_scalar_widths() {
    for mutation in [
        ForgeMutation::Base {
            item_type: 255,
            sub_type: 65535,
            unrestricted: false,
        },
        ForgeMutation::Base {
            item_type: 26,
            sub_type: 513,
            unrestricted: true,
        },
        ForgeMutation::Identity {
            id: 65535,
            sub_type: 255,
            rarity: 9,
            unrestricted: true,
        },
        ForgeMutation::Identity {
            id: 912,
            sub_type: 2,
            rarity: 8,
            unrestricted: false,
        },
    ] {
        let request = ForgeWrite::edit(spec(), 1, snapshot(), mutation).unwrap();
        let frame = request.encode();
        let (operation, value, selector, extra) = match mutation {
            ForgeMutation::Base { item_type, sub_type, .. } => (11, u32::from(sub_type), u32::from(item_type), 0),
            ForgeMutation::Identity { id, sub_type, rarity, .. } => (17, u32::from(id), u32::from(sub_type), u32::from(rarity)),
            _ => unreachable!(),
        };
        for (at, expected) in [(4, operation), (8, value), (24, selector), (28, u32::from(mutation.unrestricted())), (32, extra)] {
            assert_eq!(u32::from_le_bytes(frame[at..at + 4].try_into().unwrap()), expected);
        }
        assert_eq!(Request::decode(&frame).unwrap(), Request::EditForge(request));
        for (at, bad) in [
            (8, 65536),
            (24, 256),
            (28, 2),
            (32, if operation == 11 { 1 } else { 6 }),
            (32, if operation == 11 { 255 } else { 10 }),
        ] {
            let mut invalid = frame.clone();
            set32(&mut invalid, at, bad);
            assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err());
        }
        for at in (36..48).chain(72..336) {
            let mut invalid = frame.clone();
            invalid[at] = 1;
            assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err());
        }
        for end in REQUEST_SIZE..frame.len() {
            assert!(Request::decode(&frame[..end]).is_err());
        }
        let mut excess = frame;
        excess.push(0);
        assert!(Request::decode(&excess).is_err());
    }
}

#[test]
fn affix_roll_batches_have_a_strict_fixed_header_and_complete_expected_tail() {
    for count in [1, 2, 32] {
        let changes = (0..count)
            .map(|slot| AffixRollChange {
                slot,
                value: if slot % 2 == 0 { 0 } else { 255 },
            })
            .collect::<Vec<_>>();
        let mutation = ForgeMutation::AffixRolls {
            rolls: AffixRolls::new(&changes).unwrap(),
            unrestricted: count == 32,
        };
        let request = ForgeWrite::edit(spec(), 4, snapshot(), mutation).unwrap();
        let bytes = request.encode();
        assert_eq!(&bytes[4..12], [2, 0, 0, 0, count, 0, 0, 0]);
        for (i, change) in changes.iter().enumerate() {
            assert_eq!(u32::from_le_bytes(bytes[80 + i * 8..84 + i * 8].try_into().unwrap()), u32::from(change.slot));
            assert_eq!(u32::from_le_bytes(bytes[84 + i * 8..88 + i * 8].try_into().unwrap()), u32::from(change.value));
        }
        assert_eq!(Request::decode(&bytes).unwrap(), Request::EditForge(request));
        for end in 0..bytes.len() {
            assert!(Request::decode(&bytes[..end]).is_err());
        }
        for (at, value) in [(8, 0), (8, 33), (24, 1), (24, 2), (28, 2), (32, 1), (80, 32), (84, 256)] {
            let mut invalid = bytes.clone();
            set32(&mut invalid, at, value);
            assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err(), "at {at}: {value}");
        }
        for at in (36..48).chain(72..80).chain(80 + usize::from(count) * 8..336) {
            let mut invalid = bytes.clone();
            invalid[at] = 1;
            assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err(), "reserved {at}");
        }
        if count > 1 {
            let mut invalid = bytes.clone();
            set32(&mut invalid, 88, 0);
            assert!(Request::decode_header(&invalid[..REQUEST_SIZE]).is_err());
        }
        let mut excess = bytes;
        excess.push(0);
        assert!(Request::decode(&excess).is_err());
    }
    assert!(AffixRolls::new(&[]).is_err());
    assert!(AffixRolls::new(&[AffixRollChange { slot: 32, value: 0 }]).is_err());
    assert!(AffixRolls::new(&[AffixRollChange { slot: 0, value: 0 }, AffixRollChange { slot: 0, value: 255 }]).is_err());
    assert!(AffixRolls::new(&[AffixRollChange { slot: 0, value: 0 }; 33]).is_err());
}

#[test]
fn replacement_frame_bounds_and_reserved_bits_are_checked_before_submission() {
    let mutation = ForgeMutation::Affix {
        slot: 31,
        id: 4095,
        tier: 8,
        roll: 255,
        seal: 3,
        unrestricted: true,
    };
    let write = ForgeWrite::edit(spec(), 12, snapshot(), mutation).unwrap();
    let bytes = write.encode();
    assert_eq!(decode_frame(&bytes).unwrap(), Request::EditForge(write));
    for (offset, value) in [(8, 4096), (24, 32), (28, 2), (32, 0), (32, 9), (32, 0x40001), (32, 0x1000001), (36, 1), (80, 1)] {
        let mut damaged = bytes.clone();
        set32(&mut damaged, offset, value);
        assert!(decode_header(&damaged[..REQUEST_SIZE]).is_err(), "{offset}/{value}");
    }
}

#[test]
fn affix_list_frames_reject_bad_bounds_flags_and_unused_fields() {
    for mutation in [
        ForgeMutation::AddAffix {
            id: 4095,
            tier: 8,
            roll: 255,
            seal: 2,
            unrestricted: true,
        },
        ForgeMutation::RemoveAffix { slot: 31, id: 65500 },
    ] {
        let request = ForgeWrite::edit(spec(), 12, snapshot(), mutation).unwrap();
        let bytes = request.encode();
        assert_eq!(decode_frame(&bytes).unwrap(), Request::EditForge(request));
        let damage = if matches!(mutation, ForgeMutation::AddAffix { .. }) {
            vec![(8, 4096), (24, 1), (28, 2), (32, 0), (32, 9), (32, 0x40001), (80, 1)]
        } else {
            vec![(8, 65536), (24, 32), (28, 1), (32, 1), (80, 1)]
        };
        for (offset, value) in damage {
            let mut damaged = bytes.clone();
            set32(&mut damaged, offset, value);
            assert!(decode_header(&damaged[..REQUEST_SIZE]).is_err(), "{mutation:?} {offset}/{value}");
        }
    }
}
