use eg_trainer_wire::protocol::{Discovery, DispatcherSpec, MAX_RESPONSE_SIZE, ProtocolError, REQUEST_HEADER_SIZE, Request, Response, ResponseCode, RuntimeStatus};

fn dispatcher() -> DispatcherSpec {
    DispatcherSpec {
        rva: 0x0807_0605_0403_0201,
        bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
    }
}

fn envelope(operation: u32, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x35, 0x47, 0x46, 0x45];
    bytes.extend(operation.to_le_bytes());
    bytes.extend((body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}
fn proof() -> Vec<u8> {
    let mut body = vec![1, 2, 3, 4, 5, 6, 7, 8];
    body.extend(dispatcher().bytes);
    body
}
#[test]
fn compact_control_requests_have_only_their_sequential_body() {
    assert_eq!(Request::Discover.encode(), envelope(6, &[]));
    for (operation, request) in [
        (7, Request::Status { dispatcher: dispatcher() }),
        (15, Request::ClearHooks { dispatcher: dispatcher() }),
        (18, Request::ReadForge { dispatcher: dispatcher() }),
        (20, Request::ReadLocale { dispatcher: dispatcher() }),
    ] {
        let golden = envelope(operation, &proof());
        assert_eq!(request.encode(), golden);
        assert_eq!(Request::decode(&golden), Ok(request));
    }
}
#[test]
fn envelope_rejects_unknown_operations_and_unbounded_bodies_before_allocation() {
    for op in [0, 3, 8, 16, 22, u32::MAX] {
        assert!(Request::decode_header(&envelope(op, &[])).is_err());
    }
    let mut header = envelope(18, &proof());
    header.truncate(REQUEST_HEADER_SIZE);
    for length in [0u32, 23, 1_048_577, u32::MAX] {
        header[8..12].copy_from_slice(&length.to_le_bytes());
        assert!(Request::decode_header(&header).is_err());
    }
    let mut wrong = Request::Discover.encode();
    wrong[0] = 0;
    assert!(Request::decode(&wrong).is_err());
}
#[test]
fn attack_leech_request_compares_nonzero_manager() {
    let request = Request::ReadAttackLeech {
        dispatcher: dispatcher(),
        expected_manager: 0x1817161514131211,
    };
    let mut body = proof();
    body.extend([0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18]);
    let mut golden = envelope(23, &body);
    assert_eq!(request.encode(), golden);
    assert_eq!(Request::decode(&golden), Ok(request));
    golden[36..44].fill(0);
    assert!(Request::decode(&golden).is_err());
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
    for code in 0_u32..=7 {
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
    header[4] = 8;
    assert!(Response::decode_header(&header).is_err());
    header[4] = 0;
    header[0] = 0;
    assert_eq!(Response::decode_header(&header), Err(ProtocolError::InvalidMagic));
    assert!(
        Response {
            code: ResponseCode::Done,
            payload: vec![0; MAX_RESPONSE_SIZE + 1]
        }
        .encode()
        .is_err()
    );
}

#[test]
fn discovery_carries_only_build_process_and_dispatcher_identity() {
    let discovery = Discovery {
        build_id: "aB12".repeat(16),
        dispatcher: dispatcher(),
    };
    let mut golden = 64u32.to_le_bytes().to_vec();
    golden.extend_from_slice(discovery.build_id.as_bytes());
    golden.extend_from_slice(&678u32.to_le_bytes());
    golden.extend_from_slice(&dispatcher().rva.to_le_bytes());
    golden.extend_from_slice(&dispatcher().bytes);
    assert_eq!(discovery.encode(678).unwrap(), golden);
    assert_eq!(Discovery::decode(&golden, 678), Ok(discovery));
    assert!(Discovery::decode(&golden, 679).is_err());
    for value in [b'z', 0xff] {
        let mut malformed = golden.clone();
        malformed[4] = value;
        assert!(Discovery::decode(&malformed, 678).is_err());
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
fn unload_is_an_exact_empty_control_frame_without_dispatcher() {
    let frame = envelope(25, &[]);
    assert_eq!(Request::Unload.dispatcher(), None);
    assert_eq!(Request::Unload.encode(), frame);
    assert_eq!(Request::decode(&frame), Ok(Request::Unload));
    assert!(Request::decode_header(&envelope(25, &[0])).is_err());
    assert!(Request::decode(&[frame, vec![0]].concat()).is_err());
}
