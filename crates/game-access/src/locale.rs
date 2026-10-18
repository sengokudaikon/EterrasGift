//! Owned locale/version identity, without culture lookup or fabricated defaults.
//!
//! Two texts in order: language, then application version. Each is a little-endian
//! u32 UTF-8 byte length followed by those exact bytes, at most 4096 per text.
//! Language must not be blank; an empty application version is permitted, as in
//! the original GameDataIdentity. Whitespace/casing and unknown language codes are
//! preserved. The reader's runtime admission/initialization policy is separate.

use crate::protocol::{MAX_RESPONSE_SIZE, ProtocolError};

pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameLocale {
    pub language: String,
    pub game_version: String,
}

pub fn encode(locale: &GameLocale) -> Result<Vec<u8>, ProtocolError> {
    if locale.language.trim().is_empty() || locale.language.len() > MAX_TEXT_BYTES || locale.game_version.len() > MAX_TEXT_BYTES {
        return Err(ProtocolError::InvalidData);
    }
    let mut out = Vec::with_capacity(8 + locale.language.len() + locale.game_version.len());
    for text in [&locale.language, &locale.game_version] {
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
    }
    if out.len() > MAX_RESPONSE_SIZE {
        return Err(ProtocolError::InvalidData);
    }
    Ok(out)
}

pub fn decode(payload: &[u8]) -> Result<GameLocale, ProtocolError> {
    if payload.len() > MAX_RESPONSE_SIZE {
        return Err(ProtocolError::InvalidData);
    }
    let mut rest = payload;
    let mut text = || -> Result<String, ProtocolError> {
        let (length, tail) = rest.split_at_checked(4).ok_or(ProtocolError::InvalidData)?;
        let length = u32::from_le_bytes(length.try_into().map_err(|_| ProtocolError::InvalidData)?) as usize;
        if length > MAX_TEXT_BYTES {
            return Err(ProtocolError::InvalidData);
        }
        let (bytes, tail) = tail.split_at_checked(length).ok_or(ProtocolError::InvalidData)?;
        rest = tail;
        std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| ProtocolError::InvalidData)
    };
    let language = text()?;
    let game_version = text()?;
    if !rest.is_empty() || language.trim().is_empty() {
        return Err(ProtocolError::InvalidData);
    }
    Ok(GameLocale { language, game_version })
}
