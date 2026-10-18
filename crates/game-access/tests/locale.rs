use eg_game_access::{
    locale::{self, GameLocale, MAX_TEXT_BYTES},
    protocol::{MAX_RESPONSE_SIZE, ProtocolError},
};

#[test]
fn locale_matches_independently_authored_wire_and_preserves_identity() {
    let golden = [5, 0, 0, 0, b'p', b't', b'-', b'B', b'R', 5, 0, 0, 0, b'1', b'.', b'4', b'.', b'2'];
    let expected = GameLocale {
        language: "pt-BR".into(),
        game_version: "1.4.2".into(),
    };
    assert_eq!(locale::decode(&golden), Ok(expected.clone()));
    assert_eq!(locale::encode(&expected), Ok(golden.to_vec()));
    for length in 0..golden.len() {
        assert!(locale::decode(&golden[..length]).is_err(), "truncated at {length}");
    }
    assert!(locale::decode(&[golden.as_slice(), &[0]].concat()).is_err());
}

#[test]
fn empty_version_is_valid_but_empty_or_blank_language_is_not() {
    let golden = [2, 0, 0, 0, b'e', b'n', 0, 0, 0, 0];
    let expected = GameLocale {
        language: "en".into(),
        game_version: String::new(),
    };
    assert_eq!(locale::decode(&golden), Ok(expected.clone()));
    assert_eq!(locale::encode(&expected), Ok(golden.to_vec()));
    for language in ["", " ", "\t\r\n", "\u{2003}\u{00a0}"] {
        let locale = GameLocale {
            language: language.into(),
            game_version: "1.0".into(),
        };
        assert_eq!(locale::encode(&locale), Err(ProtocolError::InvalidData));
        let mut bytes = (language.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(language.as_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(locale::decode(&bytes), Err(ProtocolError::InvalidData));
    }
}

#[test]
fn unknown_culture_case_whitespace_and_unicode_are_preserved_without_normalization() {
    for language in ["not-a-registered-culture", "EN-us", " en-Ω-🌿 ", "中文"] {
        let expected = GameLocale {
            language: language.into(),
            game_version: " build Ω\0🌿 ".into(),
        };
        assert_eq!(locale::decode(&locale::encode(&expected).unwrap()), Ok(expected));
    }
}

#[test]
fn each_text_is_bounded_by_utf8_bytes_including_exact_maximum() {
    let maximum = GameLocale {
        language: "é".repeat(MAX_TEXT_BYTES / 2),
        game_version: "x".repeat(MAX_TEXT_BYTES),
    };
    let bytes = locale::encode(&maximum).unwrap();
    assert_eq!(bytes.len(), 8 + MAX_TEXT_BYTES * 2);
    assert_eq!(locale::decode(&bytes), Ok(maximum));
    for locale in [
        GameLocale {
            language: "é".repeat(MAX_TEXT_BYTES / 2) + "x",
            game_version: String::new(),
        },
        GameLocale {
            language: "en".into(),
            game_version: "x".repeat(MAX_TEXT_BYTES + 1),
        },
    ] {
        assert!(locale::encode(&locale).is_err());
    }
    for length in [MAX_TEXT_BYTES as u32 + 1, u32::MAX] {
        let mut bytes = length.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        assert!(locale::decode(&bytes).is_err());
        let mut bytes = vec![2, 0, 0, 0, b'e', b'n'];
        bytes.extend_from_slice(&length.to_le_bytes());
        assert!(locale::decode(&bytes).is_err());
    }
    assert!(locale::decode(&vec![0; MAX_RESPONSE_SIZE + 1]).is_err());
}

#[test]
fn invalid_utf8_in_either_text_is_rejected_instead_of_replaced() {
    for bytes in [
        vec![1, 0, 0, 0, 0xff, 0, 0, 0, 0],
        vec![2, 0, 0, 0, b'e', b'n', 1, 0, 0, 0, 0xff],
        vec![2, 0, 0, 0, 0xc3, 0x28, 0, 0, 0, 0],
    ] {
        assert_eq!(locale::decode(&bytes), Err(ProtocolError::InvalidData));
    }
}
