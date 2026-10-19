use eg_game_access::protocol::{
    ADAPTER, Discovery, DispatcherSpec, HookState, MAGIC, MAX_RESPONSE_SIZE, ProtocolError, REQUEST_SIZE, REQUIRED_KEYS, Request, Response, ResponseCode, RuntimeStatus,
};

fn dispatcher() -> DispatcherSpec {
    DispatcherSpec {
        rva: 0x0807_0605_0403_0201,
        bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
    }
}

#[test]
fn control_requests_match_the_packed_native_layout() {
    let mut golden = vec![0; 336];
    golden[..8].copy_from_slice(&[0x35, 0x47, 0x46, 0x45, 6, 0, 0, 0]);
    assert_eq!(Request::Discover.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(Request::Discover));
    golden[4] = 7;
    golden[48..56].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    golden[56..72].copy_from_slice(&dispatcher().bytes);
    let status = Request::Status { dispatcher: dispatcher() };
    assert_eq!(status.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(status));
    golden[4] = 15;
    let clear = Request::ClearHooks { dispatcher: dispatcher() };
    // EncodeHooks([]) has a zero byte length at offset 76, not a tail count byte.
    assert_eq!(clear.encode(), golden);
    assert_eq!(clear.encode().len(), 336);
    assert_eq!(Request::decode(&golden), Ok(clear));
    golden.push(0);
    assert_eq!(Request::decode(&golden), Err(ProtocolError::InvalidData));
    golden.pop();
    golden[4] = 18;
    let read_forge = Request::ReadForge { dispatcher: dispatcher() };
    assert_eq!(read_forge.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(read_forge));
    golden[4] = 19;
    let limits = Request::ReadItemLimits { dispatcher: dispatcher() };
    assert_eq!(limits.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(limits));
    golden[4] = 20;
    let locale = Request::ReadLocale { dispatcher: dispatcher() };
    assert_eq!(locale.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(locale));
}

#[test]
fn requests_reject_reserved_fields_and_unsupported_operations() {
    let source = Request::Status { dispatcher: dispatcher() }.encode();
    for request in [
        Request::Status { dispatcher: dispatcher() },
        Request::ReadForge { dispatcher: dispatcher() },
        Request::ReadItemLimits { dispatcher: dispatcher() },
        Request::ReadLocale { dispatcher: dispatcher() },
    ] {
        for position in (8..48).chain(72..REQUEST_SIZE) {
            let mut bytes = request.encode();
            bytes[position] = 1;
            assert!(Request::decode(&bytes).is_err(), "reserved byte {position}");
        }
    }
    for operation in [0_u32, 1, 2, 3, 4, 5, 8, 14, 16, 17, 22, u32::MAX] {
        let mut bytes = source.clone();
        bytes[4..8].copy_from_slice(&operation.to_le_bytes());
        assert!(Request::decode(&bytes).is_err());
    }
    for length in 0..REQUEST_SIZE {
        assert!(Request::decode(&source[..length]).is_err());
    }
    let mut discover = Request::Discover.encode();
    discover[56] = 1;
    assert!(Request::decode(&discover).is_err());
}

#[test]
fn attack_leech_request_binds_nonzero_manager_and_has_no_tail() {
    let request = Request::ReadAttackLeech {
        dispatcher: dispatcher(),
        expected_manager: 0x1817_1615_1413_1211,
    };
    let mut golden = vec![0; 336];
    golden[..16].copy_from_slice(&[0x35, 0x47, 0x46, 0x45, 23, 0, 0, 0, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18]);
    golden[48..56].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    golden[56..72].copy_from_slice(&dispatcher().bytes);
    assert_eq!(request.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(request));
    assert_eq!(Request::decode_header(&golden).unwrap().tail_len(), 0);
    for offset in (16..48).chain(72..336) {
        let mut malformed = golden.clone();
        malformed[offset] = 1;
        assert!(Request::decode(&malformed).is_err(), "reserved byte {offset}");
        assert!(Request::decode_header(&malformed).is_err());
    }
    let mut zero = golden.clone();
    zero[8..16].fill(0);
    assert!(Request::decode(&zero).is_err());
    golden.push(0);
    assert!(Request::decode(&golden).is_err());
}

#[test]
fn catalog_query_has_two_bounded_fields_and_no_other_reserved_data() {
    use eg_game_access::catalog::CatalogKind;
    for kind in [CatalogKind::Bases, CatalogKind::Affixes, CatalogKind::Uniques, CatalogKind::Rarities] {
        for cursor in [0, 16.min(kind.maximum_total()), kind.maximum_total()] {
            let request = Request::ReadCatalogPage {
                dispatcher: dispatcher(),
                kind,
                cursor,
            };
            let bytes = request.encode();
            assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 21);
            assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), kind as u32);
            assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), cursor);
            assert_eq!(Request::decode(&bytes), Ok(request));
            for offset in (16..48).chain(72..REQUEST_SIZE) {
                let mut malformed = bytes.clone();
                malformed[offset] = 1;
                assert_eq!(Request::decode(&malformed), Err(ProtocolError::InvalidData));
            }
        }
        let bytes = Request::ReadCatalogPage {
            dispatcher: dispatcher(),
            kind,
            cursor: kind.maximum_total() + 1,
        }
        .encode();
        assert_eq!(Request::decode(&bytes), Err(ProtocolError::InvalidData));
    }
    let mut bytes = Request::ReadCatalogPage {
        dispatcher: dispatcher(),
        kind: CatalogKind::Bases,
        cursor: 0,
    }
    .encode();
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(Request::decode(&bytes), Err(ProtocolError::InvalidData));
}

#[test]
fn response_header_is_bounded_before_payload_allocation() {
    let golden = [0x35, 0x47, 0x46, 0x45, 5, 0, 0, 0, 3, 0, 0, 0, b'n', b'o', b'w'];
    let expected = Response {
        code: ResponseCode::OfflineRequired,
        payload: b"now".to_vec(),
    };
    assert_eq!(Response::decode(&golden), Ok(expected.clone()));
    assert_eq!(expected.encode().unwrap(), golden);
    assert_eq!(Response::decode_header(&golden[..12]), Ok((ResponseCode::OfflineRequired, 3)));
    for code in 0_u32..=6 {
        let mut header = golden[..12].to_vec();
        header[4..8].copy_from_slice(&code.to_le_bytes());
        assert_eq!(Response::decode_header(&header).unwrap().0 as u32, code);
    }
    for length in [MAX_RESPONSE_SIZE as u32 + 1, u32::MAX] {
        let mut header = golden[..12].to_vec();
        header[8..12].copy_from_slice(&length.to_le_bytes());
        assert!(Response::decode_header(&header).is_err());
    }
    let mut header = golden[..12].to_vec();
    header[4] = 7;
    assert!(Response::decode_header(&header).is_err());
    header[4] = 0;
    header[0] = 0;
    assert_eq!(Response::decode_header(&header), Err(ProtocolError::IncompatibleVersion));
    assert!(Response::decode(&golden[..14]).is_err());
    assert!(Response::decode(&[golden.as_slice(), &[0]].concat()).is_err());
    assert!(
        Response {
            code: ResponseCode::Done,
            payload: vec![0; MAX_RESPONSE_SIZE + 1]
        }
        .encode()
        .is_err()
    );
}

// These names are the C# RuntimeDiscovery.RequiredKeys/native requiredFields contract.
const KEYS: [&str; 37] = [
    "ItemContainersManager.CraftingOffset",
    "CraftingContainers.MainOffset",
    "OneSlotItemContainer.ContentOffset",
    "ItemContainerEntry.DataOffset",
    "ItemDataUnpacked.SerializationOffset",
    "ItemDataUnpacked.IndividualIdOffset",
    "ItemDataUnpacked.ItemTypeOffset",
    "ItemDataUnpacked.SubTypeOffset",
    "ItemDataUnpacked.RarityOffset",
    "ItemDataUnpacked.UniqueIdOffset",
    "ItemDataUnpacked.ImplicitRollsOffset",
    "ItemDataUnpacked.UniqueRollsOffset",
    "ItemDataUnpacked.AffixesOffset",
    "ItemDataUnpacked.ClassRequirementOffset",
    "ItemDataUnpacked.FullNameOffset",
    "ItemDataUnpacked.SealedRegularOffset",
    "ItemDataUnpacked.SealedPrimordialOffset",
    "ItemDataUnpacked.SealedCorruptionOffset",
    "ItemDataUnpacked.ForgingPotentialOffset",
    "ItemDataUnpacked.ForgingPotentialTypeOffset",
    "ItemDataUnpacked.LegendaryPotentialOffset",
    "ItemDataUnpacked.WeaversWillOffset",
    "ItemDataUnpacked.WeaversTouchOffset",
    "ItemDataUnpacked.CorruptedOffset",
    "ItemDataUnpacked.RuinedOffset",
    "ItemDataUnpacked.LevelRequirementOffset",
    "ItemAffix.IdOffset",
    "ItemAffix.TierOffset",
    "ItemAffix.RollOffset",
    "ItemAffix.TypeOffset",
    "ItemAffix.SpecialAffixTypeOffset",
    "ItemAffix.NameOffset",
    "ItemAffix.SealOffset",
    "AffixList.SingleAffixesOffset",
    "AffixList.MultiAffixesOffset",
    "AffixDefinition.IdOffset",
    "AffixDefinition.TiersOffset",
];

fn text(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as i32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// Independently authored native appendInt/appendText wire format.
fn discovery_payload(keys: &[&str]) -> Vec<u8> {
    let mut out = 30_i32.to_le_bytes().to_vec();
    text(&mut out, &"aB12".repeat(16));
    text(&mut out, "il2cpp-x64-v1");
    out.extend_from_slice(&678_i32.to_le_bytes());
    out.extend_from_slice(&0x0807_0605_0403_0201_u64.to_le_bytes());
    out.extend_from_slice(&dispatcher().bytes);
    out.extend_from_slice(&(keys.len() as i32).to_le_bytes());
    for key in keys {
        text(&mut out, key);
        out.extend_from_slice(&16_u64.to_le_bytes());
    }
    out
}

#[test]
fn discovery_matches_native_handshake_and_requires_all_fields() {
    assert_eq!(REQUIRED_KEYS, KEYS);
    assert_eq!(ADAPTER, "il2cpp-x64-v1");
    assert_eq!(MAGIC, 0x4546_4735);
    let golden = discovery_payload(&KEYS);
    let discovery = Discovery::decode(&golden, 678).unwrap();
    assert_eq!(discovery.build_id, "aB12".repeat(16));
    assert_eq!(discovery.dispatcher, dispatcher());
    assert_eq!(discovery.offsets.len(), 37);
    assert!(discovery.offsets.values().all(|offset| *offset == 16));
    assert_eq!(Discovery::decode(&discovery.encode(678).unwrap(), 678), Ok(discovery));
    assert!(Discovery::decode(&golden, 679).is_err());
    for (missing, missing_key) in KEYS.iter().enumerate() {
        let keys: Vec<_> = KEYS.iter().copied().enumerate().filter_map(|(index, key)| (index != missing).then_some(key)).collect();
        assert!(Discovery::decode(&discovery_payload(&keys), 678).is_err(), "missing {missing_key}");
    }
    let mut duplicate = KEYS.to_vec();
    duplicate.push(KEYS[0]);
    assert!(Discovery::decode(&discovery_payload(&duplicate), 678).is_err());
    assert!(Discovery::decode(&[golden.as_slice(), &[0]].concat(), 678).is_err());
    for length in 0..golden.len() {
        assert!(Discovery::decode(&golden[..length], 678).is_err(), "truncated at {length}");
    }
}

#[test]
fn discovery_rejects_incompatible_and_malformed_fields() {
    let golden = discovery_payload(&KEYS);
    let mut wire = golden.clone();
    for version in [21_i32, 22, 23, 24, 25] {
        wire[..4].copy_from_slice(&version.to_le_bytes());
        assert_eq!(Discovery::decode(&wire, 678), Err(ProtocolError::IncompatibleVersion));
    }
    wire = golden.clone();
    // version, build text length, 64 build bytes, adapter text length.
    wire[76] = b'x';
    assert_eq!(Discovery::decode(&wire, 678), Err(ProtocolError::IncompatibleVersion));
    for value in [b'z', 0xff] {
        wire = golden.clone();
        wire[8] = value;
        assert!(Discovery::decode(&wire, 678).is_err());
    }
    for length in [0_u32, 4097, u32::MAX] {
        wire = golden.clone();
        wire[4..8].copy_from_slice(&length.to_le_bytes());
        assert!(Discovery::decode(&wire, 678).is_err());
    }
    let count_position = 4 + 4 + 64 + 4 + 13 + 4 + 8 + 16;
    for count in [0_u32, 129, u32::MAX] {
        wire = golden.clone();
        wire[count_position..count_position + 4].copy_from_slice(&count.to_le_bytes());
        assert!(Discovery::decode(&wire, 678).is_err());
    }
    let first_offset_position = count_position + 4 + 4 + KEYS[0].len();
    for offset in [0_u64, 15, 16385, u64::MAX] {
        wire = golden.clone();
        wire[first_offset_position..first_offset_position + 8].copy_from_slice(&offset.to_le_bytes());
        assert!(Discovery::decode(&wire, 678).is_err());
    }
    for offset in [16_u64, 16384] {
        wire = golden.clone();
        wire[first_offset_position..first_offset_position + 8].copy_from_slice(&offset.to_le_bytes());
        assert!(Discovery::decode(&wire, 678).is_ok());
    }
}

#[test]
fn status_is_exact_and_preserves_unsigned_manager_address() {
    let golden = [1, 0, 0, 0, 0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 3, 0, 0, 0];
    let status = RuntimeStatus {
        online: true,
        manager: 0x8899_aabb_ccdd_eeff,
        features_on: 3,
    };
    assert_eq!(RuntimeStatus::decode(&golden), Ok(status));
    assert_eq!(status.encode().unwrap(), golden);
    for online in [2_u32, u32::MAX] {
        let mut wire = golden;
        wire[..4].copy_from_slice(&online.to_le_bytes());
        assert!(RuntimeStatus::decode(&wire).is_err());
    }
    let mut wire = golden;
    wire[12..].copy_from_slice(&0x8000_0000_u32.to_le_bytes());
    assert!(RuntimeStatus::decode(&wire).is_err());
    assert!(RuntimeStatus { features_on: u32::MAX, ..status }.encode().is_err());
    for length in 0..16 {
        assert!(RuntimeStatus::decode(&golden[..length]).is_err());
    }
    assert!(RuntimeStatus::decode(&[golden.as_slice(), &[0]].concat()).is_err());
}

#[test]
fn hook_response_is_strict_and_supports_empty_clear_result() {
    assert_eq!(HookState::decode_all(&[0; 4]), Ok(Vec::new()));
    let golden = [1, 0, 0, 0, 3, b'f', b'o', b'o', 0, 3, 0, b'b', b'a', b'd'];
    assert_eq!(
        HookState::decode_all(&golden),
        Ok(vec![HookState {
            id: "foo".into(),
            on: false,
            failure: Some("bad".into())
        }])
    );
    for length in 0..golden.len() {
        assert!(HookState::decode_all(&golden[..length]).is_err());
    }
    let mut duplicate = [2, 0, 0, 0].to_vec();
    duplicate.extend_from_slice(&golden[4..]);
    duplicate.extend_from_slice(&golden[4..]);
    assert!(HookState::decode_all(&duplicate).is_err());
    for (index, value) in [(4, 0), (5, 0xff), (8, 2), (11, 0xff)] {
        let mut wire = golden;
        wire[index] = value;
        assert!(HookState::decode_all(&wire).is_err());
    }
    assert!(HookState::decode_all(&1025_i32.to_le_bytes()).is_err());
    assert!(HookState::decode_all(&(-1_i32).to_le_bytes()).is_err());
    assert!(HookState::decode_all(&[golden.as_slice(), &[0]].concat()).is_err());
}
