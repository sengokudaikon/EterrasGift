use eg_trainer::{
    definitions::catalog::{CatalogIdentity, EXTRACTION_VERSION},
    item::edit::*,
    item::values::*,
};
use eg_trainer_wire::{
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    locale::GameLocale,
    mapping::*,
    process::ProcessIdentity,
};
use std::path::PathBuf;
fn identity() -> CatalogIdentity {
    CatalogIdentity {
        binary_hash: "A".repeat(64),
        metadata_hash: "B".repeat(64),
        locale: GameLocale {
            language: "en".into(),
            game_version: "1.5".into(),
        },
        extraction_version: EXTRACTION_VERSION,
    }
}
fn item() -> ForgeSnapshot {
    let mut item = ForgeSnapshot {
        item: 0x1234,
        individual_id: 44,
        fingerprint: 0,
        name: "Item".into(),
        item_type: 3,
        sub_type: 2,
        rarity: 7,
        class_requirement: 0,
        affixes: vec![AffixSnapshot {
            slot: 0,
            id: 12,
            tier: 8,
            roll: 55,
            kind: 0,
            seal: 0,
            name: "prefix".into(),
            special: 0,
        }],
        implicit_rolls: vec![9, 10],
        identity: Some(UniqueIdentity { id: 911, rolls: vec![77, 88] }),
        status: ItemStatus {
            forging_potential: 5,
            forging_potential_type: 0,
            legendary_potential: 1,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 20,
        },
    };
    item.fingerprint = item.legacy_fingerprint();
    item
}
fn observation() -> ExpectedObservation {
    ExpectedObservation::new(
        1,
        ProcessIdentity {
            pid: 42,
            created: 1234567,
            executable: PathBuf::from("C:/Authored/Game.exe"),
        },
        identity(),
        item(),
    )
    .unwrap()
}
fn altered(expected: &ExpectedObservation, f: impl FnOnce(&mut ForgeSnapshot)) -> ExpectedObservation {
    let mut item = expected.snapshot().clone();
    f(&mut item);
    item.fingerprint = item.legacy_fingerprint();
    ExpectedObservation::new(expected.generation(), expected.process().clone(), expected.catalog_identity().clone(), item).unwrap()
}
fn request() -> MappingRequest {
    MappingRequest {
        address: MappingAddress {
            target: MappingTarget::Affix,
            index: 0,
            line: None,
        },
        draft: None,
    }
}
fn context() -> RawMappingPage {
    RawMappingPage {
        language: "en".into(),
        context: vec![1, 2, 255],
        address: request().address,
        start: MappingStart::ContextOnly,
        samples: vec![],
    }
}
fn page(start: u8, values: impl Fn(usize) -> f64) -> RawMappingPage {
    RawMappingPage {
        start: MappingStart::Samples(start),
        samples: (usize::from(start)..usize::from(start) + 16)
            .map(|i| RawValueSample {
                raw_quantity: values(i) / 100.,
                display: format!("{}%", values(i)),
                unit: "%".into(),
                effects: vec![format!("effect at roll {i}")],
            })
            .collect(),
        ..context()
    }
}
#[derive(Default)]
struct Parser {
    seen: Vec<(String, String, String)>,
}
impl DisplayScalarParser for Parser {
    fn parse(&mut self, language: &str, display: &str, unit: &str) -> Result<f64, String> {
        self.seen.push((language.into(), display.into(), unit.into()));
        if language != "en" {
            return Err("No authored parser for this language".into());
        }
        display.strip_suffix(unit).ok_or("suffix")?.parse().map_err(|_| "number".into())
    }
}
fn builder() -> MappingBuilder {
    MappingBuilder::new(observation(), request(), context(), None).unwrap()
}
fn table(values: impl Fn(usize) -> f64) -> Result<ValueTable, MappingError> {
    let mut b = builder();
    let mut parser = Parser::default();
    for start in (0..=240).step_by(16) {
        b.push(&observation(), &request(), page(start, &values), &mut parser)?;
    }
    b.finish()
}
fn modifier(index: u32, ids: Vec<u8>, editable: bool) -> ModifierSnapshot {
    ModifierSnapshot {
        index,
        text: format!("line{index}"),
        range: "1-2".into(),
        roll_ids: ids,
        editable,
    }
}
fn tooltip(kind: TooltipKind) -> BoundTooltip {
    BoundTooltip::new(&observation(), kind, vec![modifier(5, vec![0], true), modifier(7, vec![1], false)]).unwrap()
}
fn rolls(target: RollTarget, changes: &[(i32, i32)]) -> RollEdit {
    RollEdit {
        target,
        changes: changes.iter().map(|(index, value)| RollChange { index: *index, value: *value }).collect(),
        allow_incompatible: false,
    }
}

#[test]
fn full_table_uses_all_actual_display_samples_and_preserves_diagnostic_raw() {
    let mut b = builder();
    let mut parser = Parser::default();
    for start in (0..=240).step_by(16) {
        assert_eq!(b.next_start(), Some(start));
        b.push(&observation(), &request(), page(start, |i| (i * i) as f64), &mut parser).unwrap();
    }
    assert_eq!(b.next_start(), None);
    let t = b.finish().unwrap();
    assert_eq!(t.samples().len(), 256);
    assert_eq!(t.sample(128).value, 16384.);
    assert_eq!(t.sample(128).raw_quantity, 163.84);
    assert_eq!(t.sample(128).display, "16384%");
    assert_eq!(t.sample(128).effects, vec!["effect at roll 128"]);
    assert_eq!(t.minimum(), 0.);
    assert_eq!(t.maximum(), 65025.);
    assert_eq!(t.unit(), "%");
    assert!(t.context_key().starts_with("en:0:0:"));
    assert_eq!(parser.seen.len(), 256);
    assert_eq!(parser.seen[128], ("en".into(), "16384%".into(), "%".into()));
    assert_eq!(t.nearest(16384., 0).unwrap(), 128);
}
#[test]
fn endpoints_never_substitute_for_complete_sampling() {
    let mut b = builder();
    let mut parser = Parser::default();
    b.push(&observation(), &request(), page(0, |i| i as f64), &mut parser).unwrap();
    assert_eq!(b.finish(), Err(MappingError::Incomplete));
    assert_eq!(builder().finish(), Err(MappingError::Incomplete));
}
#[test]
fn increasing_decreasing_plateaus_fixed_and_nonmonotonic() {
    assert!(table(|i| (i / 3) as f64).is_ok());
    let t = table(|i| 255. - i as f64).unwrap();
    assert_eq!(t.minimum(), 0.);
    assert_eq!(t.maximum(), 255.);
    assert_eq!(t.nearest(20., 0).unwrap(), 235);
    assert!(table(|_| 3.).unwrap().is_fixed());
    assert!(table(|i| if i % 2 == 0 { 0.0 } else { -0.0 }).unwrap().is_fixed());
    assert!(!t.is_fixed());
    assert_eq!(table(|i| if i == 100 { 200. } else { i as f64 }), Err(MappingError::NonMonotonic));
}
#[test]
fn nearest_matches_source_sort_priority_for_plateaus_and_midpoints() {
    for reverse in [false, true] {
        let t = table(|i| {
            let v = (i / 3) as f64;
            if reverse { 85. - v } else { v }
        })
        .unwrap();
        for current in 0..=255u8 {
            for value in [0., 0.5, 1., 9.5, 27., 52.5, 84., 85.] {
                let expected = (0..256usize)
                    .min_by(|a, b| {
                        (t.samples()[*a].value - value)
                            .abs()
                            .total_cmp(&(t.samples()[*b].value - value).abs())
                            .then_with(|| a.abs_diff(current.into()).cmp(&b.abs_diff(current.into())))
                            .then_with(|| a.cmp(b))
                    })
                    .unwrap();
                assert_eq!(t.nearest(value, current).unwrap(), expected as u8);
            }
        }
        for value in [-1., 86., f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(t.nearest(value, 0), Err(MappingError::OutOfRange));
        }
    }
}
#[test]
fn any_page_failure_poisoned_and_cannot_retry() {
    let mut b = builder();
    let mut parser = Parser::default();
    assert_eq!(b.push(&observation(), &request(), page(16, |i| i as f64), &mut parser), Err(MappingError::OutOfOrder));
    assert_eq!(b.next_start(), None);
    assert_eq!(b.push(&observation(), &request(), page(0, |i| i as f64), &mut parser), Err(MappingError::Poisoned));
    assert_eq!(b.finish(), Err(MappingError::Poisoned));
    let mut b = builder();
    b.push(&observation(), &request(), page(0, |i| i as f64), &mut parser).unwrap();
    assert_eq!(b.push(&observation(), &request(), page(0, |i| i as f64), &mut parser), Err(MappingError::OutOfOrder));
}
#[test]
fn every_context_component_and_unit_remains_pinned() {
    for field in 0..5 {
        let mut b = builder();
        let mut parser = Parser::default();
        b.push(&observation(), &request(), page(0, |i| i as f64), &mut parser).unwrap();
        let mut p = page(16, |i| i as f64);
        match field {
            0 => p.language = "fr".into(),
            1 => p.context.push(1),
            2 => p.address.index = 1,
            3 => {
                for s in &mut p.samples {
                    s.unit = " points".into();
                    s.display.push_str(" points");
                }
            }
            _ => p.address.line = Some(7),
        }
        assert_eq!(b.push(&observation(), &request(), p, &mut parser), Err(MappingError::WrongContext));
        assert_eq!(b.finish(), Err(MappingError::Poisoned));
    }
}
#[test]
fn query_draft_exact_equality_required_for_all_pages_and_publication() {
    let draft = ItemEdit::Affix(AffixEdit {
        slot: 0,
        id: 99,
        tier: 7,
        roll: 128,
        seal: SealState::Normal,
        allow_incompatible: false,
    });
    let req = MappingRequest { draft: Some(draft), ..request() };
    let mut b = MappingBuilder::new(observation(), req.clone(), context(), None).unwrap();
    let mut parser = Parser::default();
    for start in (0..=240).step_by(16) {
        b.push(&observation(), &req, page(start, |i| i as f64), &mut parser).unwrap();
    }
    let table = b.finish().unwrap();
    assert!(table.recheck(&observation(), &req).is_ok());
    assert_eq!(table.recheck(&observation(), &request()), Err(MappingError::WrongContext));
    let mut b = MappingBuilder::new(observation(), req, context(), None).unwrap();
    assert_eq!(b.push(&observation(), &request(), page(0, |i| i as f64), &mut parser), Err(MappingError::WrongContext));
}
#[test]
fn parser_unavailable_nonfinite_or_wrong_suffix_never_uses_raw_quantity() {
    struct Failing;
    impl DisplayScalarParser for Failing {
        fn parse(&mut self, _: &str, _: &str, _: &str) -> Result<f64, String> {
            Err("exact culture unavailable".into())
        }
    }
    let mut b = builder();
    assert_eq!(
        b.push(&observation(), &request(), page(0, |i| i as f64), &mut Failing),
        Err(MappingError::DisplayUnavailable("exact culture unavailable".into()))
    );
    assert_eq!(b.finish(), Err(MappingError::Poisoned));
    struct Nonfinite;
    impl DisplayScalarParser for Nonfinite {
        fn parse(&mut self, _: &str, _: &str, _: &str) -> Result<f64, String> {
            Ok(f64::NAN)
        }
    }
    let mut b = builder();
    assert_eq!(b.push(&observation(), &request(), page(0, |i| i as f64), &mut Nonfinite), Err(MappingError::InvalidData));
    let mut b = builder();
    let mut p = page(0, |i| i as f64);
    p.samples[0].display = "0 % extra".into();
    let mut parser = Parser::default();
    assert!(matches!(b.push(&observation(), &request(), p, &mut parser), Err(MappingError::DisplayUnavailable(_))));
    assert!(parser.seen.is_empty());
}
#[test]
fn captured_language_passed_exactly_to_injected_parser() {
    let obs = observation();
    let mut identity = identity();
    identity.locale.language = "de-DE".into();
    let obs = ExpectedObservation::new(1, obs.process().clone(), identity, item()).unwrap();
    let mut c = context();
    c.language = "de-DE".into();
    let mut b = MappingBuilder::new(obs.clone(), request(), c, None).unwrap();
    let mut p = page(0, |i| i as f64);
    p.language = "de-DE".into();
    for s in &mut p.samples {
        s.display = "1,5 %".into();
        s.unit = " %".into();
    }
    struct Exact;
    impl DisplayScalarParser for Exact {
        fn parse(&mut self, l: &str, d: &str, u: &str) -> Result<f64, String> {
            assert_eq!((l, d, u), ("de-DE", "1,5 %", " %"));
            Ok(1.5)
        }
    }
    b.push(&obs, &request(), p, &mut Exact).unwrap();
    // Authored fake parser verifies delegation; this does not claim host/game locale parity.
}
#[test]
fn builder_requires_valid_current_catalog_and_context_only_initial_reply() {
    let obs = observation();
    for field in 0..2 {
        let mut identity = identity();
        if field == 0 {
            identity.binary_hash = "broken".into();
        } else {
            identity.metadata_hash = "broken".into();
        }
        let bad = ExpectedObservation::new(1, obs.process().clone(), identity, item()).unwrap();
        assert!(matches!(MappingBuilder::new(bad, request(), context(), None), Err(MappingError::InvalidData)));
    }
    let mut identity = identity();
    identity.extraction_version = 0;
    let bad = ExpectedObservation::new(1, obs.process().clone(), identity, item()).unwrap();
    assert!(matches!(MappingBuilder::new(bad, request(), context(), None), Err(MappingError::InvalidData)));
    assert!(matches!(
        MappingBuilder::new(obs.clone(), request(), page(0, |i| i as f64), None),
        Err(MappingError::WrongContext)
    ));
    let mut c = context();
    c.language = "fr".into();
    assert!(matches!(MappingBuilder::new(obs, request(), c, None), Err(MappingError::WrongContext)));
}
#[test]
fn tooltip_provenance_roll_bounds_and_readonly_lines() {
    let obs = observation();
    let t = tooltip(TooltipKind::Unique);
    assert!(t.recheck(&obs).is_ok());
    assert_eq!(t.recheck(&altered(&obs, |i| i.name.push('!'))), Err(MappingError::WrongObservation));
    assert!(BoundTooltip::new(&obs, TooltipKind::Unique, vec![modifier(1, vec![2], true)]).is_err());
    assert!(BoundTooltip::new(&obs, TooltipKind::Implicit, vec![modifier(1, vec![0, 1], true)]).is_err());
    assert!(BoundTooltip::new(&obs, TooltipKind::Unique, vec![modifier(1, vec![0], true), modifier(3, vec![0], false)]).is_ok());
    // A set item's unique lines bind its rolls as a unique's do.
    let set = altered(&obs, |i| i.rarity = 8);
    assert!(BoundTooltip::new(&set, TooltipKind::Unique, vec![]).is_ok());
    let req = MappingRequest {
        address: MappingAddress {
            target: MappingTarget::Unique,
            index: 1,
            line: Some(7),
        },
        draft: None,
    };
    assert!(req.validate(&obs, Some(&t)).is_ok());
    assert_eq!(req.validate(&obs, None), Err(MappingError::MissingTooltipEvidence));
    let badline = MappingRequest {
        address: MappingAddress { line: Some(5), ..req.address },
        draft: None,
    };
    assert!(badline.validate(&obs, Some(&t)).is_err());
}
#[test]
fn implicit_query_requires_exact_editable_single_roll_evidence_and_no_draft() {
    let obs = observation();
    let t = tooltip(TooltipKind::Implicit);
    let mut req = MappingRequest {
        address: MappingAddress {
            target: MappingTarget::Implicit,
            index: 0,
            line: None,
        },
        draft: None,
    };
    assert!(req.validate(&obs, Some(&t)).is_ok());
    assert_eq!(req.validate(&obs, None), Err(MappingError::MissingTooltipEvidence));
    req.address.index = 1;
    assert!(req.validate(&obs, Some(&t)).is_err());
    req.address.index = 0;
    req.draft = Some(ItemEdit::Roll(rolls(RollTarget::Implicit, &[(0, 99)])));
    assert!(req.validate(&obs, Some(&t)).is_err());
}
#[test]
fn affix_mapping_draft_validation_and_unique_roll_evidence() {
    let obs = observation();
    let mut req = request();
    req.address.index = 1;
    assert!(req.validate(&obs, None).is_err());
    req = request();
    for (id, tier, roll, slot) in [(4096, 1, 0, 0), (-1, 1, 0, 0), (12, 0, 0, 0), (12, 9, 0, 0), (12, 1, -1, 0), (12, 1, 256, 0), (12, 1, 0, 1)] {
        req.draft = Some(ItemEdit::Affix(AffixEdit {
            slot,
            id,
            tier,
            roll,
            seal: SealState::Normal,
            allow_incompatible: true,
        }));
        assert!(req.validate(&obs, None).is_err());
    }
    for changes in [vec![], vec![(0, 12), (0, 13)], vec![(1, 12)], vec![(0, 256)]] {
        req.draft = Some(ItemEdit::Roll(rolls(RollTarget::Affix, &changes)));
        assert!(req.validate(&obs, None).is_err());
    }
    let t = tooltip(TooltipKind::Unique);
    let mut req = MappingRequest {
        address: MappingAddress {
            target: MappingTarget::Unique,
            index: 0,
            line: None,
        },
        draft: Some(ItemEdit::Roll(rolls(RollTarget::Unique, &[(0, 99)]))),
    };
    assert!(req.validate(&obs, Some(&t)).is_ok());
    req.draft = Some(ItemEdit::Roll(rolls(RollTarget::Unique, &[(1, 99)])));
    assert!(req.validate(&obs, Some(&t)).is_err());
}
#[test]
fn preview_is_bound_to_full_observation_and_exact_draft() {
    let obs = observation();
    let draft = rolls(RollTarget::Unique, &[(0, 99)]);
    let preview = UniqueRollPreview {
        modifiers: vec![modifier(5, vec![0], true)],
    };
    let bound = BoundPreview::new(&obs, draft.clone(), preview.clone(), &tooltip(TooltipKind::Unique)).unwrap();
    assert_eq!(bound.preview(), &preview);
    assert!(bound.recheck(&obs, &draft).is_ok());
    assert_eq!(bound.recheck(&altered(&obs, |i| i.name.push('!')), &draft), Err(MappingError::WrongObservation));
    assert_eq!(bound.recheck(&obs, &rolls(RollTarget::Unique, &[(0, 98)])), Err(MappingError::WrongContext));
    assert!(BoundPreview::new(&obs, rolls(RollTarget::Affix, &[(0, 99)]), preview, &tooltip(TooltipKind::Unique)).is_err());
}
