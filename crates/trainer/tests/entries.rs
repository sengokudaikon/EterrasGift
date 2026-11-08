use eg_trainer::definitions::catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog, MAX_CATALOG_BYTES};
use eg_trainer::definitions::entries::*;
use eg_trainer_wire::{
    definitions::{ItemLimits, ValueRange},
    locale::GameLocale,
    protocol::ProtocolError,
};

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
fn identity() -> CatalogIdentity {
    CatalogIdentity {
        binary_hash: "A".repeat(64),
        metadata_hash: "B".repeat(64),
        locale: GameLocale {
            language: " mod_Locale ".into(),
            game_version: "version-β".into(),
        },
        extraction_version: EXTRACTION_VERSION,
    }
}
fn limits() -> ItemLimits {
    let range = ValueRange { minimum: 0, maximum: 255 };
    ItemLimits {
        forging_potential: range,
        legendary_potential: range,
        weavers_will: range,
        weavers_touch: range,
        level_requirement: range,
    }
}
fn catalog(entries: CatalogEntries) -> Result<GameCatalog, ProtocolError> {
    GameCatalog::new(identity(), limits(), entries)
}
fn bases(bases: Vec<ItemTypeDefinition>) -> CatalogEntries {
    CatalogEntries {
        bases,
        ..CatalogEntries::default()
    }
}
fn affixes(affixes: Vec<AffixDefinition>) -> CatalogEntries {
    CatalogEntries {
        affixes,
        ..CatalogEntries::default()
    }
}
fn uniques(uniques: Vec<UniqueDefinition>) -> CatalogEntries {
    CatalogEntries {
        uniques,
        ..CatalogEntries::default()
    }
}
fn rarities(rarities: Vec<RarityDefinition>) -> CatalogEntries {
    CatalogEntries {
        rarities,
        ..CatalogEntries::default()
    }
}
/// The catalog its form makes again.
fn read(bytes: &[u8]) -> Result<GameCatalog, ProtocolError> {
    catalog(decode(bytes)?)
}
fn roundtrips(entries: CatalogEntries) {
    let made = catalog(entries).unwrap();
    assert_eq!(read(&encode(&made).unwrap()).unwrap(), made);
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
        affix_effect_modifier: 0.0,
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
        effects: Vec::new(),
        named_by_title: false,
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
        level_requirement: -7,
        source: None,
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

// Hand-written bytes, to reach the decoder's checks with data the encoder refuses to write.
fn int(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn text(out: &mut Vec<u8>, s: &str) {
    int(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}
/// No bases, affixes or uniques, then `count` rarities.
fn rarity_list(count: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for count in [0, 0, 0, count] {
        int(&mut out, count);
    }
    out
}
/// One rarity: the four counts (16 bytes), its ID (at 16), availability (17), name length (18..22), name (22..28) and
/// source (28..32).
fn authored_rarity(id: u8, source: u32) -> Vec<u8> {
    let mut out = rarity_list(1);
    out.extend_from_slice(&[id, 1]);
    text(&mut out, "Unique");
    int(&mut out, source);
    out
}

#[test]
fn complete_owned_shapes_roundtrip_without_interpreting_unknown_enums_or_range_order() {
    roundtrips(bases(vec![base()]));
    roundtrips(affixes(vec![affix()]));
    // An empty equipment list is not the absent one.
    roundtrips(affixes(vec![AffixDefinition {
        item_types: Some(vec![]),
        ..affix()
    }]));
    // A unique modifier keeps its unique, its own words and the game's.
    roundtrips(affixes(vec![AffixDefinition {
        level_requirement: 87,
        source: Some(AffixSource {
            unique: 65535,
            line: "15–30% more 内部".into(),
        }),
        effects: vec![AffixEffect {
            heading: "Cold Penetration".into(),
            words: "increased Mana".into(),
            explanation: String::new(),
            hides_value: true,
            no_plus: false,
        }],
        named_by_title: true,
        ..affix()
    }]));
    roundtrips(uniques(vec![unique()]));
    roundtrips(rarities(vec![RarityDefinition { id: 9, name: missing() }]));
    let mut b = base();
    b.name.text.clear();
    b.sub_types[0].implicits = missing();
    roundtrips(bases(vec![b]));
    roundtrips(CatalogEntries {
        bases: (0..17)
            .map(|id| ItemTypeDefinition {
                item_type: id,
                name: name(""),
                sub_types: vec![],
            })
            .collect(),
        affixes: vec![affix()],
        uniques: vec![unique()],
        rarities: vec![RarityDefinition { id: 0, name: missing() }, RarityDefinition { id: 9, name: missing() }],
    });
}

#[test]
fn each_list_is_bounded_before_it_is_read_and_the_form_is_exact() {
    for kind in 0..4 {
        let mut counts = [0; 4];
        counts[kind] = [MAX_BASES, MAX_AFFIXES, MAX_UNIQUES, MAX_RARITIES][kind] as u32 + 1;
        let mut bytes = Vec::new();
        for count in counts {
            int(&mut bytes, count);
        }
        assert!(decode(&bytes).is_err(), "list {kind} over its cap");
    }
    assert!(read(&rarity_list(0)).is_ok());
    assert!(decode(&rarity_list(0)[..15]).is_err(), "truncated");
    assert!(decode(&[rarity_list(0), vec![0]].concat()).is_err(), "a tail");
    assert!(decode(&vec![0; MAX_CATALOG_BYTES + 1]).is_err());
}

#[test]
fn duplicate_ids_and_subtype_ids_are_rejected_on_both_sides() {
    for entries in [
        bases(vec![base(), base()]),
        affixes(vec![affix(), affix()]),
        uniques(vec![unique(), unique()]),
        rarities(vec![RarityDefinition { id: 1, name: missing() }, RarityDefinition { id: 1, name: missing() }]),
    ] {
        assert!(catalog(entries).is_err());
    }
    let mut b = base();
    b.sub_types.push(subtype());
    assert!(catalog(bases(vec![b])).is_err());
    let mut bytes = rarity_list(2);
    let entry = &authored_rarity(1, 0)[16..];
    bytes.extend_from_slice(entry);
    bytes.extend_from_slice(entry);
    assert!(decode(&bytes).is_ok() && read(&bytes).is_err());
    assert!(read(&authored_rarity(9, 0)).is_ok());
    assert!(read(&authored_rarity(10, 0)).is_err());
}

#[test]
fn utf8_tags_sources_and_counts_are_rejected() {
    let bytes = authored_rarity(7, 1);
    assert!(read(&bytes).is_ok());
    let mut bad = bytes.clone();
    bad[22] = 0xff;
    assert!(decode(&bad).is_err());
    let mut bad = bytes.clone();
    bad[17] = 2;
    assert!(decode(&bad).is_err());
    assert!(decode(&authored_rarity(7, 3)).is_err());
    let mut bad = rarity_list(MAX_RARITIES as u32 + 1);
    bad.extend_from_slice(&bytes[16..]);
    assert!(decode(&bad).is_err());
    let mut bad = bytes;
    bad[18..22].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bad).is_err());
}

#[test]
fn strict_boolean_and_finite_numeric_wire_values() {
    let bytes = encode(&catalog(bases(vec![base()])).unwrap()).unwrap();
    // The bases' count (4), base id/name/source/count (17), subtype id/name/source/levels (27).
    let bool_at = 4 + 1 + 4 + 4 + 4 + 4 + 2 + 4 + 9 + 4 + 4 + 4;
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
    let mut a = affix();
    a.standard_effect = f32::NAN;
    assert!(catalog(affixes(vec![a])).is_err());
    let mut u = unique();
    if let Availability::Available(n) = &mut u.numeric {
        n.properties[0].maximum = f32::INFINITY;
    }
    assert!(catalog(uniques(vec![u])).is_err());
}

#[test]
fn exact_nested_count_caps_and_tier_shape_are_enforced() {
    let mut b = base();
    b.sub_types = (0..=MAX_SUB_TYPES as u16).map(|sub_type| SubTypeDefinition { sub_type, ..subtype() }).collect();
    assert!(catalog(bases(vec![b])).is_err());
    let mut b = base();
    b.sub_types[0].implicits = Availability::Available(vec![implicit(); MAX_IMPLICITS + 1]);
    assert!(catalog(bases(vec![b])).is_err());
    for size in [0, 2, MAX_AFFIX_TIERS + 1] {
        let mut a = affix();
        a.numeric = Availability::Available(vec![AffixTier { properties: vec![] }; size]);
        assert!(catalog(affixes(vec![a])).is_err());
    }
    let mut a = affix();
    a.item_types = Some(vec![0; MAX_EQUIPMENT_TYPES + 1]);
    assert!(catalog(affixes(vec![a])).is_err());
    let mut a = affix();
    a.numeric = Availability::Available(vec![AffixTier {
        properties: vec![property(); MAX_PROPERTIES + 1],
    }]);
    assert!(catalog(affixes(vec![a])).is_err());
    for roll in [-2, 8] {
        let mut p = property();
        p.roll_id = roll;
        let mut a = affix();
        a.numeric = Availability::Available(vec![AffixTier { properties: vec![p] }]);
        assert!(catalog(affixes(vec![a])).is_err());
    }
    let mut u = unique();
    u.sub_types = vec![0; MAX_UNIQUE_SUB_TYPES + 1];
    assert!(catalog(uniques(vec![u])).is_err());
    for field in 0..3 {
        let mut u = unique();
        if let Availability::Available(n) = &mut u.numeric {
            match field {
                0 => n.properties = vec![property(); MAX_PROPERTIES + 1],
                1 => n.descriptions = vec![String::new(); MAX_DESCRIPTIONS + 1],
                _ => n.display_order = vec![0; MAX_DISPLAY_ORDER + 1],
            }
        }
        assert!(catalog(uniques(vec![u])).is_err());
    }
}

#[test]
fn text_bounds_unavailable_reasons_and_whole_catalog_budget() {
    let named = |text: &str| {
        rarities(vec![RarityDefinition {
            id: 0,
            name: Availability::Available(name(text)),
        }])
    };
    assert!(catalog(named(&"é".repeat(MAX_TEXT_BYTES / 2))).is_ok());
    assert!(catalog(named(&"x".repeat(MAX_TEXT_BYTES + 1))).is_err());
    for reason in ["", " \t\r\n"] {
        let entries = rarities(vec![RarityDefinition {
            id: 0,
            name: Availability::Unavailable { reason: reason.into() },
        }]);
        assert!(catalog(entries).is_err());
    }
    let mut blank = identity();
    blank.locale.language = " \n".into();
    assert!(GameCatalog::new(blank, limits(), rarities(vec![RarityDefinition { id: 0, name: missing() }])).is_err());
    // Every base, each with as many subtypes of the longest name: 60 fit in a catalog's bytes, 65 do not.
    let long_names = |count: u16| {
        bases(
            (0..MAX_BASES)
                .map(|id| ItemTypeDefinition {
                    item_type: id as u8,
                    name: name(""),
                    sub_types: (0..count)
                        .map(|sub_type| SubTypeDefinition {
                            sub_type,
                            name: name(&"x".repeat(MAX_TEXT_BYTES)),
                            implicits: Availability::Available(vec![]),
                            ..subtype()
                        })
                        .collect(),
                })
                .collect(),
        )
    };
    assert!(catalog(long_names(60)).is_ok());
    assert!(catalog(long_names(65)).is_err());
}
