//! Owned locale/version identity, without culture lookup or fabricated defaults.
//!
//! Two texts in order: language, then application version. Each is a little-endian
//! u32 UTF-8 byte length followed by those exact bytes, at most 4096 per text.
//! Language must not be blank; an empty application version is permitted.
//! Whitespace/casing and unknown language codes are preserved. The reader's runtime admission/initialization policy is separate.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Writer, invalid},
    protocol::ProtocolError,
};

pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameLocale {
    pub language: String,
    pub game_version: String,
}

pub fn encode(locale: &GameLocale) -> Result<Vec<u8>, ProtocolError> {
    if locale.language.trim().is_empty() {
        return invalid();
    }
    let mut w = Writer::default();
    w.text_within(&locale.language, MAX_TEXT_BYTES)?;
    w.text_within(&locale.game_version, MAX_TEXT_BYTES)?;
    w.finish()
}

pub fn decode(payload: &[u8]) -> Result<GameLocale, ProtocolError> {
    let mut r = Reader::reply(payload)?;
    let language = r.text(MAX_TEXT_BYTES)?.to_owned();
    let game_version = r.text(MAX_TEXT_BYTES)?.to_owned();
    r.finish()?;
    if language.trim().is_empty() {
        return invalid();
    }
    Ok(GameLocale { language, game_version })
}
