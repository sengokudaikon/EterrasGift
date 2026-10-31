//! Every message decodes from its own encoding, and from nothing else of another length: each strict prefix and a
//! trailing byte are refused. Golden bytes and field-level rejections stay with each message's own tests.

use eg_trainer_wire::{
    definitions::{self, ItemLimits, ValueRange},
    forge::{self, AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    hooks::{GameplayHook, HookObservation, PatchState},
    locale::{self, GameLocale},
    mapping::{
        self, AffixMappingDraft, AffixMappingQuery, ForgeDescriptionRequest, MappingAddress, MappingStart, MappingTarget, ModifierMappingQuery, ModifierSnapshot, RawMappingPage,
        RawValueSample, UniquePreviewRequest, UniqueRollChange,
    },
    new_affixes::{self, NewAffix, StatLine},
    protocol::{Discovery, DispatcherSpec, ProtocolError, REQUEST_HEADER_SIZE, Request, RequestFrame, Response, ResponseCode},
    write::{AffixRollChange, AffixRolls, ForgeMutation, ForgeWrite, RollArray, StatusField},
};
use std::fmt::Debug;

fn exact<T: PartialEq + Debug>(name: &str, bytes: &[u8], value: &T, decode: impl Fn(&[u8]) -> Result<T, ProtocolError>) {
    assert_eq!(decode(bytes).as_ref(), Ok(value), "{name}");
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err(), "{name} truncated at {end}");
    }
    assert!(decode(&[bytes, &[0]].concat()).is_err(), "{name} with a trailing byte");
}

fn dispatcher() -> DispatcherSpec {
    DispatcherSpec { rva: 0x1000, bytes: [0xcc; 16] }
}

fn item() -> ForgeSnapshot {
    let mut item = ForgeSnapshot {
        item: 0x1122_3344_5566_7788,
        individual_id: 0xaabb_ccdd,
        fingerprint: 0,
        name: "剑".into(),
        item_type: 42,
        sub_type: 513,
        rarity: 9,
        class_requirement: -7,
        affixes: vec![AffixSnapshot {
            slot: 0,
            id: 513,
            tier: 8,
            roll: 250,
            kind: -2,
            seal: 3,
            name: "火".into(),
            special: -123,
        }],
        implicit_rolls: vec![1, 128, 255],
        identity: Some(UniqueIdentity { id: 258, rolls: vec![0, 255] }),
        status: ItemStatus {
            forging_potential: 77,
            forging_potential_type: -3,
            legendary_potential: 2,
            weavers_will: 16,
            weavers_touch: 4,
            corrupted: true,
            ruined: false,
            level_requirement: -5,
        },
    };
    item.fingerprint = item.legacy_fingerprint();
    item
}

fn requests() -> Vec<Request> {
    let dispatcher = dispatcher();
    let edit = |mutation| Request::EditForge(ForgeWrite::edit(dispatcher, 0x99, item(), mutation).unwrap());
    let description = || ForgeDescriptionRequest::new(dispatcher, 0x99, item()).unwrap();
    let mut requests = vec![
        Request::Discover,
        Request::Unload,
        Request::Status { dispatcher },
        Request::ClearHooks { dispatcher },
        Request::ReadForge { dispatcher },
        Request::ReadLocale { dispatcher },
        Request::DescribeImplicits(description()),
        Request::DescribeUnique(description()),
        Request::PreviewUnique(UniquePreviewRequest::new(dispatcher, 0x99, item(), vec![UniqueRollChange { index: 1, value: 17 }]).unwrap()),
        Request::MapRoll(AffixMappingQuery::new(dispatcher, 0x99, item(), 0, MappingStart::ContextOnly, None).unwrap()),
        Request::MapModifierRoll(
            ModifierMappingQuery::new(
                dispatcher,
                0x99,
                item(),
                MappingAddress {
                    target: MappingTarget::Unique,
                    index: 1,
                    line: Some(3),
                },
                MappingStart::Samples(16),
                vec![UniqueRollChange { index: 0, value: 9 }],
            )
            .unwrap(),
        ),
        Request::MapModifierRoll(
            ModifierMappingQuery::new(
                dispatcher,
                0x99,
                item(),
                MappingAddress {
                    target: MappingTarget::Implicit,
                    index: 2,
                    line: None,
                },
                MappingStart::ContextOnly,
                vec![],
            )
            .unwrap(),
        ),
        edit(ForgeMutation::ModifierRolls {
            array: RollArray::Unique,
            rolls: AffixRolls::new(&[AffixRollChange { slot: 1, value: 3 }]).unwrap(),
            unrestricted: false,
        }),
        Request::MapRoll(
            AffixMappingQuery::new(
                dispatcher,
                0x99,
                item(),
                0,
                MappingStart::Samples(240),
                Some(AffixMappingDraft {
                    id: 1004,
                    tier: 4,
                    roll: 17,
                    seal: 1,
                }),
            )
            .unwrap(),
        ),
        edit(ForgeMutation::AddAffix {
            id: 4095,
            tier: 8,
            roll: 255,
            seal: 3,
            unrestricted: true,
        }),
        edit(ForgeMutation::RemoveAffix {
            slot: 0,
            id: 513,
            restore_potential: Some(2),
        }),
        edit(ForgeMutation::Affix {
            slot: 0,
            id: 7,
            tier: 1,
            roll: 0,
            seal: 0,
            unrestricted: false,
        }),
        edit(ForgeMutation::AffixRolls {
            rolls: AffixRolls::new(&[AffixRollChange { slot: 0, value: 9 }, AffixRollChange { slot: 31, value: 255 }]).unwrap(),
            unrestricted: false,
        }),
        edit(ForgeMutation::Status {
            field: StatusField::WeaversTouch,
            value: 77,
            unrestricted: true,
        }),
        edit(ForgeMutation::Base {
            item_type: 255,
            sub_type: 65535,
            unrestricted: true,
        }),
        edit(ForgeMutation::Identity {
            id: 65535,
            sub_type: 255,
            rarity: 7,
            unrestricted: false,
        }),
    ];
    for hook in GameplayHook::ALL {
        requests.push(Request::SetGameplayHook {
            dispatcher,
            expected_manager: 1,
            hook,
            on: true,
            value: hook.default_value(),
        });
        requests.push(Request::ReadGameplayHook {
            dispatcher,
            expected_manager: 1,
            hook,
        });
    }
    requests
}

#[test]
fn every_request_is_exactly_its_own_frame() {
    for request in requests() {
        let bytes = request.encode();
        exact(&format!("{request:?}"), &bytes, &request, Request::decode);
        // The header announces the body it carries, and the frame assembler accepts no more and finishes on no less.
        assert_eq!(Request::decode_header(&bytes[..REQUEST_HEADER_SIZE]).unwrap().body_len(), bytes.len() - REQUEST_HEADER_SIZE);
        if let Some((_, short)) = bytes[REQUEST_HEADER_SIZE..].split_last() {
            let mut frame = RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE]).unwrap();
            frame.push(short).unwrap();
            assert!(frame.finish().is_err());
        }
        let mut frame = RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE]).unwrap();
        frame.push(&bytes[REQUEST_HEADER_SIZE..]).unwrap();
        assert!(
            RequestFrame::new(&bytes[..REQUEST_HEADER_SIZE])
                .unwrap()
                .push(&[&bytes[REQUEST_HEADER_SIZE..], &[0]].concat())
                .is_err()
        );
        assert_eq!(frame.finish(), Ok(request));
    }
}

#[test]
fn retired_operations_are_refused_at_the_header() {
    for operation in [19_u32, 21, 23, 24] {
        let mut header = Request::Status { dispatcher: dispatcher() }.encode();
        header[4..8].copy_from_slice(&operation.to_le_bytes());
        assert!(Request::decode_header(&header[..REQUEST_HEADER_SIZE]).is_err(), "operation {operation}");
    }
}

#[test]
fn every_payload_is_exactly_its_own_encoding() {
    exact("item", &forge::encode(Some(&item())).unwrap(), &Some(item()), forge::decode);

    let locale = GameLocale {
        language: "pt-BR".into(),
        game_version: "1.4.2".into(),
    };
    exact("locale", &locale::encode(&locale).unwrap(), &locale, locale::decode);

    let range = ValueRange { minimum: 3, maximum: 211 };
    let limits = ItemLimits {
        forging_potential: range,
        legendary_potential: range,
        weavers_will: range,
        weavers_touch: range,
        level_requirement: range,
    };
    exact("item limits", &definitions::encode(&limits).unwrap(), &limits, definitions::decode);

    let address = MappingAddress {
        target: MappingTarget::Unique,
        index: 1,
        line: Some(4),
    };
    let page = RawMappingPage {
        language: "en".into(),
        context: vec![0, 255, 7],
        address,
        start: MappingStart::Samples(255),
        samples: vec![RawValueSample {
            raw_quantity: 0.5,
            display: "50%".into(),
            unit: "%".into(),
            effects: vec!["bonus".into()],
        }],
    };
    exact("mapping page", &mapping::encode_mapping(&page).unwrap(), &page, |bytes| {
        mapping::decode_mapping(bytes, address, MappingStart::Samples(255))
    });
    let context = RawMappingPage {
        start: MappingStart::ContextOnly,
        samples: Vec::new(),
        ..page
    };
    exact("mapping context", &mapping::encode_mapping(&context).unwrap(), &context, |bytes| {
        mapping::decode_mapping(bytes, address, MappingStart::ContextOnly)
    });

    let modifiers = vec![ModifierSnapshot {
        index: 12,
        text: "damage".into(),
        range: "1-2".into(),
        roll_ids: vec![0, 7],
        editable: true,
    }];
    exact("modifiers", &mapping::encode_modifiers(&modifiers).unwrap(), &modifiers, mapping::decode_modifiers);

    let hook = HookObservation {
        patch: PatchState::Shared,
        enabled: true,
        value: 0x0102_0304_0506_0708,
    };
    exact("hook observation", &hook.encode(), &hook, HookObservation::decode);

    let discovery = Discovery {
        build_id: "aB12".repeat(16),
        dispatcher: dispatcher(),
    };
    exact("discovery", &discovery.encode(678).unwrap(), &discovery, |bytes| Discovery::decode(bytes, 678));

    let response = Response {
        code: ResponseCode::Refused,
        payload: b"no".to_vec(),
    };
    exact("response", &response.encode().unwrap(), &response, Response::decode);
}

fn new_affix() -> NewAffix {
    let line = StatLine {
        property: 98,
        special_tag: 2,
        tags: -471,
        extra_tag: 7,
        modifier_type: 1,
        minimum: -1.5,
        maximum: 0.25,
    };
    NewAffix {
        id: *new_affixes::ID_RANGES[0].start(),
        name: "EG Exsanguinous: Potions".into(),
        display_name: "15-30% increased speed".into(),
        properties: vec![line; 3],
    }
}

#[test]
fn registered_affixes_round_trip_with_three_or_more_lines() {
    let request = Request::RegisterAffixes {
        dispatcher: dispatcher(),
        expected_manager: 9,
        affixes: vec![
            new_affix(),
            NewAffix {
                display_name: String::new(),
                properties: new_affix().properties[..1].to_vec(),
                ..new_affix()
            },
        ],
    };
    exact("register affixes", &request.encode(), &request, Request::decode);
}

#[test]
fn a_registered_affix_is_refused_outside_its_contract() {
    let encoded = |change: fn(&mut NewAffix)| {
        let mut affix = new_affix();
        change(&mut affix);
        Request::RegisterAffixes {
            dispatcher: dispatcher(),
            expected_manager: 9,
            affixes: vec![affix],
        }
        .encode()
    };
    type Change = fn(&mut NewAffix);
    let cases: [(&str, Change); 7] = [
        ("a shipped ID", |a| a.id = 25),
        ("Loom's block", |a| a.id = 3000),
        ("past what a save holds", |a| a.id = 4096),
        ("no marker", |a| a.name = "Exsanguinous".into()),
        ("several lines without a name", |a| a.display_name.clear()),
        ("no line", |a| a.properties.clear()),
        ("a value that is no number", |a| a.properties[0].minimum = f32::NAN),
    ];
    for (name, change) in cases {
        assert!(Request::decode(&encoded(change)).is_err(), "{name}");
    }
    let none = Request::RegisterAffixes {
        dispatcher: dispatcher(),
        expected_manager: 9,
        affixes: vec![],
    };
    assert!(Request::decode(&none.encode()).is_err(), "an empty request");
    let too_many = Request::RegisterAffixes {
        dispatcher: dispatcher(),
        expected_manager: 9,
        affixes: vec![new_affix(); new_affixes::MAX_PER_REQUEST + 1],
    };
    assert!(Request::decode(&too_many.encode()).is_err(), "more than a request holds");
    let no_character = Request::RegisterAffixes {
        dispatcher: dispatcher(),
        expected_manager: 0,
        affixes: vec![new_affix()],
    };
    assert!(Request::decode(&no_character.encode()).is_err(), "no character to check");
}
