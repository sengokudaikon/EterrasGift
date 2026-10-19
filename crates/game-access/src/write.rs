//! Owned protocol26 level-requirement writes and strict full readback classification.
//! These codecs grant no process access or write permission. Host generation,
//! profile, policy and main-thread runtime admission remain independent checks.

use crate::{
    forge::{self, ForgeSnapshot},
    protocol::{DispatcherSpec, MAGIC, MAX_RESPONSE_SIZE, ProtocolError, REQUEST_SIZE, Response, ResponseCode},
};

pub const SET_LEVEL_REQUIREMENT_OPERATION: u32 = 22;
pub const MAX_ERROR_TEXT: usize = 4096;
/// Exact maximum of the existing strict owned forge schema: fixed48, main name,
/// 32 affixes each20+name bytes,3 implicit rolls,optional identity3+8roll bytes.
/// This is a versioned schema limit, not an arbitrary caller-provided allocation.
pub const MAX_EXPECTED_BYTES: usize = 48 + forge::MAX_NAME_BYTES + forge::MAX_AFFIXES * (20 + forge::MAX_NAME_BYTES) + forge::MAX_IMPLICIT_ROLLS + 3 + forge::MAX_UNIQUE_ROLLS;
pub const MIN_EXPECTED_BYTES: usize = 48;
pub const MAX_REQUEST_BYTES: usize = REQUEST_SIZE + MAX_EXPECTED_BYTES;
fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelRequirementWrite {
    dispatcher: DispatcherSpec,
    expected_manager: u64,
    expected: Box<ForgeSnapshot>,
    value: u8,
    canonical_expected: Vec<u8>,
}
impl LevelRequirementWrite {
    /// Validates/copies the complete expected item before any transport operation.
    /// Manager and item addresses are opaque identity tokens, never dereferenced.
    /// Eligibility and the current game's literal maxLevel remain native checks.
    pub fn new(dispatcher: DispatcherSpec, expected_manager: u64, expected: ForgeSnapshot, value: i32) -> Result<Self, ProtocolError> {
        let value = u8::try_from(value).map_err(|_| ProtocolError::InvalidData)?;
        if expected_manager == 0 {
            return invalid();
        }
        let canonical_expected = forge::encode(Some(&expected))?;
        if !(MIN_EXPECTED_BYTES..=MAX_EXPECTED_BYTES).contains(&canonical_expected.len()) {
            return invalid();
        }
        Ok(Self {
            dispatcher,
            expected_manager,
            expected: Box::new(expected),
            value,
            canonical_expected,
        })
    }
    pub fn expected(&self) -> &ForgeSnapshot {
        &self.expected
    }
    pub fn expected_manager(&self) -> u64 {
        self.expected_manager
    }
    pub fn value(&self) -> u8 {
        self.value
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
        self.dispatcher
    }
    pub fn projected(&self) -> ForgeSnapshot {
        let mut projected = (*self.expected).clone();
        projected.status.level_requirement = i32::from(self.value);
        projected.fingerprint = projected.legacy_fingerprint();
        projected
    }
    /// Infallible apart from allocation: all fields and canonical tail are owned,
    /// immutable and validated by construction. No encoder unwrap of caller data.
    pub fn encode(&self) -> Vec<u8> {
        let mut frame = vec![0; REQUEST_SIZE];
        frame[..4].copy_from_slice(&MAGIC.to_le_bytes());
        frame[4..8].copy_from_slice(&SET_LEVEL_REQUIREMENT_OPERATION.to_le_bytes());
        frame[8..12].copy_from_slice(&i32::from(self.value).to_le_bytes());
        frame[12..16].copy_from_slice(&(self.canonical_expected.len() as u32).to_le_bytes());
        frame[16..24].copy_from_slice(&self.expected_manager.to_le_bytes());
        frame[48..56].copy_from_slice(&self.dispatcher.rva.to_le_bytes());
        frame[56..72].copy_from_slice(&self.dispatcher.bytes);
        frame.extend_from_slice(&self.canonical_expected);
        frame
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteReply {
    /// Exact projected full readback. This classifies bytes, not delivery evidence;
    /// the host still rechecks current generation/identity before publication.
    Confirmed(ForgeSnapshot),
    /// Trusted helper contract raises these codes only before the store boundary.
    /// This is never permission for an automatic write retry.
    NotApplied { code: ResponseCode, message: String },
    /// Store may have happened; retain diagnostics and require explicit fresh read.
    Uncertain { reason: String },
}
fn uncertain(reason: impl Into<String>) -> WriteReply {
    WriteReply::Uncertain { reason: reason.into() }
}
pub fn classify_response(response: &Response, request: &LevelRequirementWrite) -> WriteReply {
    if response.payload.len() > MAX_RESPONSE_SIZE {
        return uncertain("response exceeds frame bound");
    }
    if response.code == ResponseCode::Done {
        return match forge::decode(&response.payload) {
            Ok(Some(actual)) if actual == request.projected() => WriteReply::Confirmed(actual),
            Ok(_) => uncertain("write readback differs from full projected item"),
            Err(_) => uncertain("invalid write readback payload"),
        };
    }
    if response.payload.len() > MAX_ERROR_TEXT {
        return uncertain("failure text exceeds bound");
    }
    let Ok(message) = std::str::from_utf8(&response.payload) else {
        return uncertain("invalid UTF-8 failure text");
    };
    if response.code == ResponseCode::Failed {
        return uncertain(message);
    }
    WriteReply::NotApplied {
        code: response.code,
        message: message.into(),
    }
}
