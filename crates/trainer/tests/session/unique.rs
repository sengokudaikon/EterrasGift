use super::*;
use eg_trainer::{
    edits::{ExpectedObservation, RollChange, RollEdit, RollTarget},
    mapping::{BoundTooltip, TooltipKind},
};
use eg_trainer_wire::{
    forge::UniqueIdentity,
    mapping::{self, ForgeDescriptionRequest, ModifierSnapshot, UniquePreviewRequest, UniqueRollChange},
};
fn item() -> ForgeSnapshot {
    let mut item = forge_item();
    item.rarity = 9;
    item.identity = Some(UniqueIdentity { id: 911, rolls: vec![17, 81] });
    item.fingerprint = item.legacy_fingerprint();
    item
}
fn lines() -> Vec<ModifierSnapshot> {
    vec![
        ModifierSnapshot {
            index: 0,
            text: "Game unique Ω".into(),
            range: "Game range".into(),
            roll_ids: vec![0, 1],
            editable: true,
        },
        ModifierSnapshot {
            index: 2,
            text: "Shared roll".into(),
            range: "".into(),
            roll_ids: vec![0],
            editable: true,
        },
        ModifierSnapshot {
            index: 3,
            text: "Fixed value".into(),
            range: "".into(),
            roll_ids: vec![],
            editable: false,
        },
    ]
}
fn observation(f: &Fixture) -> ExpectedObservation {
    let profile = f.profile();
    ExpectedObservation::new(
        1,
        f.target.clone(),
        eg_trainer::catalog::CatalogIdentity {
            binary_hash: profile.binary_hash().into(),
            metadata_hash: profile.metadata_hash().into(),
            locale: GameLocale {
                language: "en".into(),
                game_version: "owned".into(),
            },
            extraction_version: eg_trainer::catalog::EXTRACTION_VERSION,
        },
        item(),
    )
    .unwrap()
}
fn draft() -> RollEdit {
    RollEdit {
        target: RollTarget::Unique,
        changes: vec![RollChange { index: 0, value: 223 }],
        allow_incompatible: false,
    }
}
#[test]
fn unique_text_and_preview_are_full_observation_bound_without_write_history() {
    let f = Fixture::new();
    let before = observation(&f);
    let mut preview = lines();
    preview[0].text = "Draft 223 Ω".into();
    let wire = FakeTransport::new(
        f.target.clone(),
        [
            f.discover(),
            status(false, 0x1234, 0),
            done(mapping::encode_modifiers(&lines()).unwrap()),
            done(mapping::encode_modifiers(&preview).unwrap()),
        ],
    );
    let mut s = f.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    s.enable();
    run(s.connect(now())).unwrap();
    let actual = run(s.describe_unique(now(), before.snapshot())).unwrap().unwrap();
    assert_eq!(actual, lines());
    let evidence = BoundTooltip::new(&before, TooltipKind::Unique, actual).unwrap();
    let result = run(s.preview_unique(now(), &before, &draft(), &evidence)).unwrap();
    assert_eq!(result.preview().modifiers, preview);
    assert!(s.last_write().is_none());
    assert_eq!(s.state(), SessionState::Ready);
    assert!(matches!(&wire.requests()[2],Request::DescribeUnique(q) if q.expected()==before.snapshot() && q.manager()==0x1234));
    assert!(matches!(&wire.requests()[3],Request::PreviewUnique(q) if q.expected()==before.snapshot() && q.changes()==[UniqueRollChange{index:0,value:223}]));
}
#[test]
fn optional_unique_failures_keep_ready_but_bad_frames_and_global_failures_revoke() {
    for code in [
        ResponseCode::Refused,
        ResponseCode::Failed,
        ResponseCode::NotReady,
        ResponseCode::StaleItem,
        ResponseCode::OfflineRequired,
    ] {
        let f = Fixture::new();
        let wire = FakeTransport::new(
            f.target.clone(),
            [
                f.discover(),
                status(false, 0x1234, 0),
                Step::Reply(Ok(Response {
                    code,
                    payload: b"unavailable".to_vec(),
                })),
            ],
        );
        let mut s = f.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        s.enable();
        run(s.connect(now())).unwrap();
        let result = run(s.describe_unique(now(), &item()));
        if code == ResponseCode::OfflineRequired {
            assert!(result.is_err());
            assert_ne!(s.state(), SessionState::Ready);
        } else {
            assert_eq!(result.unwrap(), None);
            assert_eq!(s.state(), SessionState::Ready);
        }
    }
    for bad in [
        vec![1],
        mapping::encode_modifiers(&[ModifierSnapshot {
            roll_ids: vec![7],
            ..lines()[0].clone()
        }])
        .unwrap(),
    ] {
        let f = Fixture::new();
        let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0), done(bad)]);
        let mut s = f.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
        s.enable();
        run(s.connect(now())).unwrap();
        assert!(run(s.describe_unique(now(), &item())).is_err());
        assert_eq!(s.state(), SessionState::Disconnected);
    }
}
#[test]
fn preview_refusal_is_optional_and_invalid_drafts_never_reach_transport() {
    let f = Fixture::new();
    let before = observation(&f);
    let evidence = BoundTooltip::new(&before, TooltipKind::Unique, lines()).unwrap();
    let wire = FakeTransport::new(
        f.target.clone(),
        [
            f.discover(),
            status(false, 0x1234, 0),
            Step::Reply(Ok(Response {
                code: ResponseCode::Refused,
                payload: b"copy unavailable".to_vec(),
            })),
        ],
    );
    let mut s = f.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    s.enable();
    run(s.connect(now())).unwrap();
    let mut bad = draft();
    bad.changes[0].index = 7;
    assert!(run(s.preview_unique(now(), &before, &bad, &evidence)).is_err());
    assert_eq!(wire.requests().len(), 2);
    assert!(matches!(run(s.preview_unique(now(), &before, &draft(), &evidence)), Err(SessionError::Mapping(_))));
    assert_eq!(s.state(), SessionState::Ready);
    assert_eq!(wire.requests().len(), 3);
}

#[test]
fn obsolete_catalog_previews_are_refused_before_transport_without_releasing_a_healthy_session() {
    let f = Fixture::new();
    let current = observation(&f);
    let mut identity = current.catalog_identity().clone();
    identity.extraction_version += 1;
    let before = ExpectedObservation::new(current.generation(), f.target.clone(), identity, item()).unwrap();
    let evidence = BoundTooltip::new(&before, TooltipKind::Unique, lines()).unwrap();
    let wire = FakeTransport::new(f.target.clone(), [f.discover(), status(false, 0x1234, 0)]);
    let mut s = f.session(&wire, &FakeLog::new([offline()]), GatePolicy::Release);
    s.enable();
    run(s.connect(now())).unwrap();
    assert!(matches!(run(s.preview_unique(now(), &before, &draft(), &evidence)), Err(SessionError::Mapping(_))));
    assert_eq!(wire.requests().len(), 2);
    assert_eq!(s.state(), SessionState::Ready);
}
#[test]
fn conditional_unique_frames_reject_duplicate_rolls_reserved_data_and_truncation() {
    let f = Fixture::new();
    for request in [
        Request::DescribeUnique(ForgeDescriptionRequest::new(f.dispatcher, 41, item()).unwrap()),
        Request::PreviewUnique(
            UniquePreviewRequest::new(
                f.dispatcher,
                41,
                item(),
                vec![UniqueRollChange { index: 0, value: 17 }, UniqueRollChange { index: 1, value: 255 }],
            )
            .unwrap(),
        ),
    ] {
        let frame = request.encode();
        assert_eq!(Request::decode(&frame), Ok(request.clone()));
        for offset in [0, 4, 8] {
            let mut bad = frame.clone();
            bad[offset] ^= 0xff;
            assert!(Request::decode(&bad).is_err(), "{offset}");
        }
        assert!(Request::decode(&frame[..frame.len() - 1]).is_err());
        let mut excess = frame.clone();
        excess.push(0);
        assert!(Request::decode(&excess).is_err());
        if matches!(request, Request::PreviewUnique(_)) {
            // Count and indices are consecutive bytes after the manager token.
            for (offset, value) in [(44, 0), (44, 9), (45, 8), (47, 0)] {
                let mut bad = frame.clone();
                bad[offset] = value;
                assert!(Request::decode(&bad).is_err(), "{offset}");
            }
        }
    }
}
