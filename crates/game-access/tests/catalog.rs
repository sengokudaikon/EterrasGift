use eg_game_access::{catalog::*, locale::GameLocale, protocol::MAX_RESPONSE_SIZE};

fn name(text: &str) -> DefinitionName {
    DefinitionName {
        text: text.into(),
        source: DefinitionNameSource::StoredDisplay,
    }
}
fn missing<T>() -> Availability<T> {
    Availability::Unavailable {
        reason: "formatter not audited".into(),
    }
}
fn page(entries: CatalogEntries) -> CatalogPage {
    CatalogPage {
        locale: GameLocale {
            language: "en".into(),
            game_version: String::new(),
        },
        kind: entries.kind(),
        cursor: 0,
        total: entries.len() as u32,
        entries,
    }
}
fn property() -> NumericProperty {
    NumericProperty {
        property: 255,
        tags: i32::MIN,
        special_tag: 254,
        extra_tag: -3,
        modifier_type: 88,
        set_property: true,
        minimum: 19.25,
        maximum: -2.5,
        roll_id: 7,
        rounding: -9,
        display_added_as_percentage: true,
        display_added_as_tenth: false,
        less_is_better: true,
    }
}
fn implicit() -> ImplicitDefinition {
    ImplicitDefinition {
        property: 255,
        tags: -2,
        special_tag: 254,
        extra_tag: i32::MIN,
        modifier_type: 91,
        minimum: -7.5,
        maximum: 19.25,
        minimum_text: missing(),
        maximum_text: Availability::Available("+19.25".into()),
    }
}
fn subtype() -> SubTypeDefinition {
    SubTypeDefinition {
        sub_type: 65535,
        name: name("Gold Ring"),
        level_requirement: -4,
        class_requirement: i32::MIN,
        cannot_drop: true,
        is_corrupted: false,
        implicits: Availability::Available(vec![implicit()]),
    }
}
fn base() -> ItemTypeDefinition {
    ItemTypeDefinition {
        item_type: 255,
        name: name("Ring"),
        sub_types: vec![subtype()],
    }
}
fn affix() -> AffixDefinition {
    AffixDefinition {
        id: 65535,
        raw_name: "internal".into(),
        raw_display_name: "stored".into(),
        game_display_name: missing(),
        kind: -1,
        special: 999,
        class_requirement: i32::MIN,
        item_types: None,
        tier_count: 1,
        standard_effect: -1.25,
        numeric: Availability::Available(vec![AffixTier { properties: vec![property()] }]),
    }
}
fn unique() -> UniqueDefinition {
    UniqueDefinition {
        id: 65535,
        name: DefinitionName {
            text: "内部".into(),
            source: DefinitionNameSource::InternalName,
        },
        item_type: 255,
        sub_types: vec![0, 255],
        is_set_item: true,
        set_id: 255,
        legendary_type: -8,
        can_drop_as_legendary: true,
        is_pre_corrupted: false,
        numeric: Availability::Available(UniqueNumericData {
            properties: vec![property()],
            descriptions: vec!["".into(), "description".into()],
            display_order: vec![255, 0, -1],
        }),
    }
}

// Independent authoring helpers deliberately do not call any production encoder.
fn int(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn text(out: &mut Vec<u8>, s: &str) {
    int(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}
fn header(kind: u32, total: u32, count: u32) -> Vec<u8> {
    let mut out = Vec::new();
    int(&mut out, kind);
    int(&mut out, 0);
    int(&mut out, total);
    text(&mut out, "en");
    text(&mut out, "");
    int(&mut out, count);
    out
}
fn authored_rarity(id: u8, source: u32) -> Vec<u8> {
    let mut out = header(3, 1, 1);
    out.extend_from_slice(&[id, 1]);
    text(&mut out, "Unique");
    int(&mut out, source);
    out
}

#[test]
fn independent_literal_rarity_wire_and_name_provenance() {
    let literal = vec![
        3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, b'e', b'n', 0, 0, 0, 0, 1, 0, 0, 0, 7, 1, 6, 0, 0, 0, b'U', b'n', b'i', b'q', b'u', b'e', 1, 0, 0, 0,
    ];
    let expected = page(CatalogEntries::Rarities(vec![RarityDefinition {
        id: 7,
        name: Availability::Available(name("Unique")),
    }]));
    assert_eq!(decode(&literal).unwrap(), expected);
    assert_eq!(encode(&expected).unwrap(), literal);
    assert_eq!(authored_rarity(7, 1), literal);
}

#[test]
fn independently_authored_affix_preserves_null_empty_and_explicit_unavailability() {
    let mut bytes = header(1, 1, 1);
    bytes.extend_from_slice(&42u16.to_le_bytes());
    text(&mut bytes, "raw");
    text(&mut bytes, "");
    bytes.push(0);
    text(&mut bytes, "unaudited display");
    for n in [-1i32, 99, i32::MIN] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    bytes.push(0);
    bytes.extend_from_slice(&128u16.to_le_bytes());
    bytes.extend_from_slice(&(-2.5f32).to_le_bytes());
    bytes.push(0);
    text(&mut bytes, "unaudited numeric");
    let expected = page(CatalogEntries::Affixes(vec![AffixDefinition {
        id: 42,
        raw_name: "raw".into(),
        raw_display_name: String::new(),
        game_display_name: Availability::Unavailable {
            reason: "unaudited display".into(),
        },
        kind: -1,
        special: 99,
        class_requirement: i32::MIN,
        item_types: None,
        tier_count: 128,
        standard_effect: -2.5,
        numeric: Availability::Unavailable {
            reason: "unaudited numeric".into(),
        },
    }]));
    assert_eq!(decode(&bytes).unwrap(), expected);
    assert_eq!(encode(&expected).unwrap(), bytes);
    let mut empty = expected.clone();
    if let CatalogEntries::Affixes(v) = &mut empty.entries {
        v[0].item_types = Some(vec![]);
    }
    assert_ne!(encode(&empty).unwrap(), bytes);
    assert_eq!(decode(&encode(&empty).unwrap()).unwrap(), empty);
}

#[test]
fn complete_owned_shapes_roundtrip_without_interpreting_unknown_enums_or_range_order() {
    for entries in [
        CatalogEntries::Bases(vec![base()]),
        CatalogEntries::Affixes(vec![affix()]),
        CatalogEntries::Uniques(vec![unique()]),
        CatalogEntries::Rarities(vec![RarityDefinition { id: 9, name: missing() }]),
    ] {
        let mut p = page(entries);
        p.locale.language = " mod_Locale ".into();
        p.locale.game_version = "version-β".into();
        assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    }
    let mut p = page(CatalogEntries::Bases(vec![base()]));
    if let CatalogEntries::Bases(v) = &mut p.entries {
        v[0].name.text.clear();
        v[0].sub_types[0].implicits = missing();
    }
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
}

#[test]
fn canonical_pagination_and_kind_total_caps_fail_closed() {
    assert!(CatalogKind::try_from(4).is_err());
    for kind in [CatalogKind::Bases, CatalogKind::Affixes, CatalogKind::Uniques, CatalogKind::Rarities] {
        assert!(decode(&header(kind as u32, kind.maximum_total() + 1, 0)).is_err());
    }
    let mut p = page(CatalogEntries::Bases(
        (0..16)
            .map(|id| ItemTypeDefinition {
                item_type: id,
                name: name(""),
                sub_types: vec![],
            })
            .collect(),
    ));
    p.total = 17;
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    if let CatalogEntries::Bases(v) = &mut p.entries {
        v.pop();
    }
    assert!(encode(&p).is_err());
    p.cursor = 17;
    p.entries = CatalogEntries::Bases(vec![]);
    assert!(encode(&p).is_ok());
    p.cursor = 18;
    assert!(encode(&p).is_err());
    p.cursor = 0;
    p.total = 0;
    assert!(encode(&p).is_ok());
    p.kind = CatalogKind::Affixes;
    assert!(encode(&p).is_err());
    let mut bytes = header(3, 1, 0);
    assert!(decode(&bytes).is_err());
    bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bytes).is_err());
}

#[test]
fn duplicate_ids_and_subtype_ids_are_rejected_on_both_sides() {
    for entries in [
        CatalogEntries::Bases(vec![base(), base()]),
        CatalogEntries::Affixes(vec![affix(), affix()]),
        CatalogEntries::Uniques(vec![unique(), unique()]),
        CatalogEntries::Rarities(vec![RarityDefinition { id: 1, name: missing() }, RarityDefinition { id: 1, name: missing() }]),
    ] {
        assert!(encode(&page(entries)).is_err());
    }
    let mut p = page(CatalogEntries::Bases(vec![base()]));
    if let CatalogEntries::Bases(v) = &mut p.entries {
        v[0].sub_types.push(subtype());
    }
    assert!(encode(&p).is_err());
    let mut bytes = header(3, 2, 2);
    let entry = &authored_rarity(1, 0)[26..];
    bytes.extend_from_slice(entry);
    bytes.extend_from_slice(entry);
    assert!(decode(&bytes).is_err());
    assert!(decode(&authored_rarity(10, 0)).is_err());
}

#[test]
fn all_truncations_utf8_tags_sources_and_trailing_data_are_rejected() {
    let bytes = authored_rarity(7, 1);
    for len in 0..bytes.len() {
        assert!(decode(&bytes[..len]).is_err(), "prefix {len}");
    }
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(decode(&bad).is_err());
    let mut bad = bytes.clone();
    bad[16] = 0xff;
    assert!(decode(&bad).is_err());
    let mut bad = bytes.clone();
    bad[27] = 2;
    assert!(decode(&bad).is_err());
    assert!(decode(&authored_rarity(7, 3)).is_err());
    let mut bad = header(3, 1, 17);
    bad.extend_from_slice(&bytes[26..]);
    assert!(decode(&bad).is_err());
    let mut bad = bytes;
    bad[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bad).is_err());
}

#[test]
fn strict_boolean_and_finite_numeric_wire_values() {
    let p = page(CatalogEntries::Bases(vec![base()]));
    let bytes = encode(&p).unwrap();
    // Header(26), base id/name/source/count(17), subtype id/name/source/levels(27).
    let bool_at = 26 + 1 + 4 + 4 + 4 + 4 + 2 + 4 + 9 + 4 + 4 + 4;
    assert_eq!(bytes[bool_at], 1);
    let mut bad = bytes.clone();
    bad[bool_at] = 2;
    assert!(decode(&bad).is_err());
    let offset = bytes.windows(4).position(|w| w == (-7.5f32).to_le_bytes()).unwrap();
    for n in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bad = bytes.clone();
        bad[offset..offset + 4].copy_from_slice(&n.to_le_bytes());
        assert!(decode(&bad).is_err());
    }
    let mut p = page(CatalogEntries::Affixes(vec![affix()]));
    if let CatalogEntries::Affixes(v) = &mut p.entries {
        v[0].standard_effect = f32::NAN;
    }
    assert!(encode(&p).is_err());
    let mut p = page(CatalogEntries::Uniques(vec![unique()]));
    if let CatalogEntries::Uniques(v) = &mut p.entries
        && let Availability::Available(n) = &mut v[0].numeric
    {
        n.properties[0].maximum = f32::INFINITY;
    }
    assert!(encode(&p).is_err());
}

#[test]
fn exact_nested_count_caps_and_tier_shape_are_enforced() {
    let mut b = base();
    b.sub_types = vec![subtype(); MAX_SUB_TYPES + 1];
    assert!(encode(&page(CatalogEntries::Bases(vec![b]))).is_err());
    let mut b = base();
    b.sub_types[0].implicits = Availability::Available(vec![implicit(); MAX_IMPLICITS + 1]);
    assert!(encode(&page(CatalogEntries::Bases(vec![b]))).is_err());
    for size in [0, 2, MAX_AFFIX_TIERS + 1] {
        let mut a = affix();
        a.numeric = Availability::Available(vec![AffixTier { properties: vec![] }; size]);
        assert!(encode(&page(CatalogEntries::Affixes(vec![a]))).is_err());
    }
    let mut a = affix();
    a.item_types = Some(vec![0; MAX_EQUIPMENT_TYPES + 1]);
    assert!(encode(&page(CatalogEntries::Affixes(vec![a]))).is_err());
    let mut a = affix();
    a.numeric = Availability::Available(vec![AffixTier {
        properties: vec![property(); MAX_PROPERTIES + 1],
    }]);
    assert!(encode(&page(CatalogEntries::Affixes(vec![a]))).is_err());
    for roll in [-2, 8] {
        let mut p = property();
        p.roll_id = roll;
        let mut a = affix();
        a.numeric = Availability::Available(vec![AffixTier { properties: vec![p] }]);
        assert!(encode(&page(CatalogEntries::Affixes(vec![a]))).is_err());
    }
    let mut u = unique();
    u.sub_types = vec![0; MAX_UNIQUE_SUB_TYPES + 1];
    assert!(encode(&page(CatalogEntries::Uniques(vec![u]))).is_err());
    for field in 0..3 {
        let mut u = unique();
        if let Availability::Available(n) = &mut u.numeric {
            match field {
                0 => n.properties = vec![property(); MAX_PROPERTIES + 1],
                1 => n.descriptions = vec![String::new(); MAX_DESCRIPTIONS + 1],
                _ => n.display_order = vec![0; MAX_DISPLAY_ORDER + 1],
            }
        }
        assert!(encode(&page(CatalogEntries::Uniques(vec![u]))).is_err());
    }
}

#[test]
fn text_bounds_unavailable_reasons_and_whole_message_budget() {
    let mut p = page(CatalogEntries::Rarities(vec![RarityDefinition {
        id: 0,
        name: Availability::Available(name(&"é".repeat(MAX_TEXT_BYTES / 2))),
    }]));
    assert!(encode(&p).is_ok());
    if let CatalogEntries::Rarities(v) = &mut p.entries {
        v[0].name = Availability::Available(name(&"x".repeat(MAX_TEXT_BYTES + 1)));
    }
    assert!(encode(&p).is_err());
    for reason in ["", " \t\r\n"] {
        p.entries = CatalogEntries::Rarities(vec![RarityDefinition {
            id: 0,
            name: Availability::Unavailable { reason: reason.into() },
        }]);
        assert!(encode(&p).is_err());
    }
    p.entries = CatalogEntries::Rarities(vec![RarityDefinition { id: 0, name: missing() }]);
    p.locale.language = " \n".into();
    assert!(encode(&p).is_err());
    assert!(decode(&vec![0; MAX_RESPONSE_SIZE + 1]).is_err());
    let bases = (0..16)
        .map(|id| ItemTypeDefinition {
            item_type: id,
            name: name(""),
            sub_types: (0..17)
                .map(|sub| SubTypeDefinition {
                    sub_type: sub,
                    name: name(&"x".repeat(MAX_TEXT_BYTES)),
                    level_requirement: 0,
                    class_requirement: 0,
                    cannot_drop: false,
                    is_corrupted: false,
                    implicits: Availability::Available(vec![]),
                })
                .collect(),
        })
        .collect();
    assert!(encode(&page(CatalogEntries::Bases(bases))).is_err());
}
