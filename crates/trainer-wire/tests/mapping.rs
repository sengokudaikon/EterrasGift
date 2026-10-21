use eg_trainer_wire::{mapping::*, protocol::ProtocolError};

fn address() -> MappingAddress {
    MappingAddress {
        target: MappingTarget::Unique,
        index: 1,
        line: Some(4),
    }
}
fn literal() -> Vec<u8> {
    // Authored independently: language=en, context=[0,255,7], Unique/1/line4,
    // final sample at requested position255, raw=0.5, display=50%, unit=%, effect=bonus.
    vec![
        2, 0, 0, 0, b'e', b'n', 3, 0, 0, 0, 0, 255, 7, 1, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 224, 63, 3, 0, 0, 0, b'5', b'0', b'%', 1, 0, 0, 0, b'%',
        1, 0, 0, 0, 5, 0, 0, 0, b'b', b'o', b'n', b'u', b's',
    ]
}
fn expected() -> RawMappingPage {
    RawMappingPage {
        language: "en".into(),
        context: vec![0, 255, 7],
        address: address(),
        start: MappingStart::Samples(255),
        samples: vec![RawValueSample {
            raw_quantity: 0.5,
            display: "50%".into(),
            unit: "%".into(),
            effects: vec!["bonus".into()],
        }],
    }
}
fn rejects(bytes: &[u8]) {
    assert_eq!(decode_mapping(bytes, address(), MappingStart::Samples(255)), Err(ProtocolError::InvalidData));
}

#[test]
fn independent_mapping_wire_and_context_only() {
    assert_eq!(decode_mapping(&literal(), address(), MappingStart::Samples(255)).unwrap(), expected());
    assert_eq!(encode_mapping(&expected()).unwrap(), literal());
    let context = RawMappingPage {
        start: MappingStart::ContextOnly,
        samples: vec![],
        ..expected()
    };
    assert_eq!(encode_mapping(&context).unwrap(), literal()[..25]);
    assert_eq!(decode_mapping(&literal()[..25], address(), MappingStart::ContextOnly).unwrap(), context);
}
#[test]
fn every_truncation_tail_and_invalid_utf8_rejected() {
    let bytes = literal();
    for end in 0..bytes.len() {
        rejects(&bytes[..end]);
    }
    let mut tail = bytes.clone();
    tail.push(0);
    rejects(&tail);
    let mut invalid = bytes;
    invalid[4] = 0xff;
    rejects(&invalid);
}
#[test]
fn malformed_counts_and_address_are_rejected() {
    for (offset, value) in [
        (0, -1i32),
        (0, 4097),
        (6, -1),
        (6, 32769),
        (13, -1),
        (13, 3),
        (17, -1),
        (17, 8),
        (21, -2),
        (25, 0),
        (25, 2),
        (25, 17),
        (37, -1),
        (37, 32769),
        (44, -1),
        (49, 129),
    ] {
        let mut bytes = literal();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        rejects(&bytes);
    }
    let mut wrong = address();
    wrong.index = 0;
    assert!(decode_mapping(&literal(), wrong, MappingStart::Samples(255)).is_err());
    assert!(decode_mapping(&literal(), address(), MappingStart::ContextOnly).is_err());
    assert!(decode_mapping(&literal(), address(), MappingStart::Samples(240)).is_err());
}
#[test]
fn numbers_must_be_finite_and_language_nonblank() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut bytes = literal();
        bytes[29..37].copy_from_slice(&value.to_le_bytes());
        rejects(&bytes);
        let mut page = expected();
        page.samples[0].raw_quantity = value;
        assert!(encode_mapping(&page).is_err());
    }
    let mut page = expected();
    page.language = " \t".into();
    assert!(encode_mapping(&page).is_err());
    page.language = "é".repeat(MAX_LANGUAGE_BYTES / 2);
    assert!(encode_mapping(&page).is_ok());
    page.language.push('x');
    assert!(encode_mapping(&page).is_err());
}
#[test]
fn target_storage_caps_and_line_rules() {
    for (target, last) in [(MappingTarget::Affix, 31), (MappingTarget::Unique, 7), (MappingTarget::Implicit, 2)] {
        let mut a = MappingAddress { target, index: last, line: None };
        assert!(a.validate().is_ok());
        a.index += 1;
        assert!(a.validate().is_err());
        a.index = 0;
        a.line = Some(0);
        assert_eq!(a.validate().is_ok(), target == MappingTarget::Unique);
    }
    let mut a = address();
    a.line = Some(i32::MAX as u32);
    assert!(a.validate().is_ok());
    a.line = Some(i32::MAX as u32 + 1);
    assert!(a.validate().is_err());
    for value in [-1, 3, i32::MAX] {
        assert!(MappingTarget::try_from(value).is_err());
    }
}
#[test]
fn bounded_text_effects_context_and_whole_response() {
    let mut page = expected();
    page.context = vec![7; MAX_CONTEXT_BYTES];
    page.samples[0].display = "x".repeat(MAX_TEXT_BYTES);
    page.samples[0].unit.clear();
    page.samples[0].effects = vec![String::new(); MAX_EFFECTS];
    assert!(encode_mapping(&page).is_ok());
    page.context.push(0);
    assert!(encode_mapping(&page).is_err());
    page.context.pop();
    page.samples[0].display.push('x');
    assert!(encode_mapping(&page).is_err());
    page.samples[0].display.pop();
    page.samples[0].effects.push(String::new());
    assert!(encode_mapping(&page).is_err());
    page.samples[0].effects = vec!["x".repeat(MAX_TEXT_BYTES); MAX_EFFECTS];
    assert!(encode_mapping(&page).is_err());
    rejects(&vec![0; eg_trainer_wire::protocol::MAX_RESPONSE_SIZE + 1]);
}
#[test]
fn exact_count_each_start_and_unit_consistency() {
    for start in 0..=255u8 {
        let mut page = expected();
        page.start = MappingStart::Samples(start);
        page.samples = vec![page.samples[0].clone(); page.start.count()];
        let bytes = encode_mapping(&page).unwrap();
        assert_eq!(decode_mapping(&bytes, address(), page.start).unwrap(), page);
        page.samples.push(page.samples[0].clone());
        assert!(encode_mapping(&page).is_err());
    }
    let mut page = expected();
    page.start = MappingStart::Samples(0);
    page.samples = vec![page.samples[0].clone(); 16];
    page.samples[15].unit = "different".into();
    assert!(encode_mapping(&page).is_err());
}
fn modifier() -> ModifierSnapshot {
    ModifierSnapshot {
        index: 12,
        text: "damage".into(),
        range: "1-2".into(),
        roll_ids: vec![0, 7],
        editable: true,
    }
}
fn modifier_literal() -> Vec<u8> {
    vec![
        1, 0, 0, 0, 12, 0, 0, 0, 6, 0, 0, 0, b'd', b'a', b'm', b'a', b'g', b'e', 3, 0, 0, 0, b'1', b'-', b'2', 2, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0,
    ]
}
#[test]
fn independent_modifier_preview_wire_and_all_truncations() {
    let bytes = modifier_literal();
    assert_eq!(encode_modifiers(&[modifier()]).unwrap(), bytes);
    assert_eq!(decode_modifiers(&bytes).unwrap(), vec![modifier()]);
    let preview = UniqueRollPreview { modifiers: vec![modifier()] };
    assert_eq!(decode_preview(&bytes).unwrap(), preview);
    assert_eq!(encode_preview(&preview).unwrap(), bytes);
    for end in 0..bytes.len() {
        assert!(decode_modifiers(&bytes[..end]).is_err());
    }
    let mut bytes = bytes;
    bytes.push(0);
    assert!(decode_modifiers(&bytes).is_err());
}
#[test]
fn strict_modifier_flags_ids_counts_and_utf8() {
    for (offset, value) in [
        (0, -1i32),
        (0, 129),
        (4, -1),
        (8, -1),
        (8, 32769),
        (25, -1),
        (25, 9),
        (29, -1),
        (29, 8),
        (33, 0),
        (37, -1),
        (37, 2),
    ] {
        let mut bytes = modifier_literal();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(decode_modifiers(&bytes).is_err(), "offset {offset} value {value}");
    }
    let mut bytes = modifier_literal();
    bytes[12] = 0xff;
    assert!(decode_modifiers(&bytes).is_err());
    let mut m = modifier();
    m.roll_ids.push(0);
    assert!(encode_modifiers(&[m]).is_err());
    assert!(encode_modifiers(&[modifier(), modifier()]).is_err());
    // Different tooltip lines may share the same roll; source explicitly needs it.
    let mut second = modifier();
    second.index = 500;
    assert!(encode_modifiers(&[modifier(), second]).is_ok());
    assert_eq!(decode_modifiers(&[0, 0, 0, 0]).unwrap(), vec![]);
}
#[test]
fn modifier_empty_strings_false_and_caps() {
    let mut m = modifier();
    m.text.clear();
    m.range = "x".repeat(MAX_TEXT_BYTES);
    m.roll_ids = (0..8).collect();
    m.editable = false;
    assert_eq!(decode_modifiers(&encode_modifiers(&[m.clone()]).unwrap()).unwrap(), vec![m.clone()]);
    m.range.push('x');
    assert!(encode_modifiers(&[m]).is_err());
    let lines: Vec<_> = (0..MAX_MODIFIERS)
        .map(|i| ModifierSnapshot {
            index: i as u32,
            text: String::new(),
            range: String::new(),
            roll_ids: vec![],
            editable: false,
        })
        .collect();
    assert!(encode_modifiers(&lines).is_ok());
    let mut excess = lines;
    excess.push(ModifierSnapshot {
        index: 128,
        text: String::new(),
        range: String::new(),
        roll_ids: vec![],
        editable: false,
    });
    assert!(encode_modifiers(&excess).is_err());
}

#[test]
fn detached_mapping_query_rejects_reserved_data_invalid_drafts_and_partial_frames() {
    use eg_trainer_wire::{
        forge::{AffixSnapshot, ForgeSnapshot, ItemStatus},
        protocol::{DispatcherSpec, Request},
    };
    let mut expected = ForgeSnapshot {
        item: 17,
        individual_id: 3,
        fingerprint: 0,
        name: "Owned Ω".into(),
        item_type: 4,
        sub_type: 2,
        rarity: 1,
        class_requirement: 0,
        affixes: vec![AffixSnapshot {
            slot: 0,
            id: 2,
            tier: 4,
            roll: 37,
            kind: 0,
            seal: 0,
            name: String::new(),
            special: 0,
        }],
        implicit_rolls: vec![],
        identity: None,
        status: ItemStatus {
            forging_potential: 12,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 1,
        },
    };
    expected.fingerprint = expected.legacy_fingerprint();
    let dispatcher = DispatcherSpec { rva: 23, bytes: [7; 16] };
    let query = AffixMappingQuery::new(
        dispatcher,
        41,
        expected.clone(),
        0,
        MappingStart::Samples(240),
        Some(AffixMappingDraft {
            id: 1004,
            tier: 4,
            roll: 17,
            seal: 1,
        }),
    )
    .unwrap();
    let request = Request::MapRoll(query);
    let frame = request.encode();
    assert_eq!(Request::decode(&frame), Ok(request));
    for (at, value) in [(44, 32u8), (45, 2), (47, 2), (50, 0), (50, 9), (52, 4)] {
        let mut bad = frame.clone();
        bad[at] = value;
        assert!(Request::decode(&bad).is_err());
    }
    assert!(Request::decode(&frame[..frame.len() - 1]).is_err());
    let mut excess = frame.clone();
    excess.push(0);
    assert!(Request::decode(&excess).is_err());
    let query = AffixMappingQuery::new(dispatcher, 41, expected.clone(), 0, MappingStart::ContextOnly, None).unwrap();
    let mut frame = query.encode();
    assert_eq!(frame[45..47], [0, 0]);
    assert_eq!(Request::decode(&frame), Ok(Request::MapRoll(query)));
    frame[45] = 2;
    assert!(Request::decode(&frame).is_err());
    assert!(AffixMappingQuery::new(dispatcher, 0, expected.clone(), 0, MappingStart::ContextOnly, None).is_err());
    assert!(AffixMappingQuery::new(dispatcher, 41, expected, 1, MappingStart::ContextOnly, None).is_err());
}
