//! Conditional CURRENT-player request; no write authority or replay ledger.
//! Expected identities are comparison tokens only. Never dereference wire tokens.

#![forbid(unsafe_code)]

use crate::{
    fields::{AttackLeechObservation, AttackLeechTarget, ObservedRate},
    protocol::{DispatcherSpec, ProtocolError},
};

pub const SET_ATTACK_LEECH_RATE_OPERATION: u32 = 24;

/// Validated, immutable one-field mutation intent for the currently admitted player.
/// This is neither a historical selection ticket nor proof that a request was not
/// sent before. Identical requests may become valid again when baseline bits return.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackLeechRateWrite {
    dispatcher: DispatcherSpec,
    expected: AttackLeechObservation,
    target: AttackLeechTarget,
    desired_bits: u32,
}

impl AttackLeechRateWrite {
    /// Wire-level f32 eligibility only. Host f64 editor validation must occur
    /// before projection; a finite cast alone does not validate original input.
    /// Nonzero identities are guaranteed by their private validated constructors.
    pub fn new(dispatcher: DispatcherSpec, expected: AttackLeechObservation, target: AttackLeechTarget, desired_bits: u32) -> Result<Self, ProtocolError> {
        let desired = ObservedRate { raw_bits: desired_bits };
        let old_bits = match target {
            AttackLeechTarget::AttackRate => {
                desired.attack_editor_value().map_err(|_| ProtocolError::InvalidData)?;
                expected.attack_rate.raw_bits
            }
            AttackLeechTarget::IncreasedLeechRate => {
                desired.leech_editor_value().map_err(|_| ProtocolError::InvalidData)?;
                expected.increased_leech_rate.raw_bits
            }
        };
        if old_bits == desired_bits {
            return Err(ProtocolError::InvalidData);
        }
        Ok(Self {
            dispatcher,
            expected,
            target,
            desired_bits,
        })
    }

    pub fn dispatcher(&self) -> DispatcherSpec {
        self.dispatcher
    }
    pub fn expected(&self) -> &AttackLeechObservation {
        &self.expected
    }
    pub fn expected_manager(&self) -> u64 {
        self.expected.manager.wrapper_token()
    }
    pub fn target(&self) -> AttackLeechTarget {
        self.target
    }
    pub fn desired_bits(&self) -> u32 {
        self.desired_bits
    }
    pub fn desired_value(&self) -> f32 {
        f32::from_bits(self.desired_bits)
    }
    pub fn projected(&self) -> AttackLeechObservation {
        let mut projected = self.expected;
        match self.target {
            AttackLeechTarget::AttackRate => projected.attack_rate.raw_bits = self.desired_bits,
            AttackLeechTarget::IncreasedLeechRate => projected.increased_leech_rate.raw_bits = self.desired_bits,
        }
        projected
    }

    /// Infallible apart from allocation: no unvalidated caller fields remain.
    pub fn encode(&self) -> Vec<u8> {
        crate::protocol::Request::SetAttackLeechRate(self.clone()).encode()
    }

    /// Full fixed frame validation must precede helper ownership/dispatcher/queue
    /// admission. This decoder performs no I/O, float arithmetic or pointer access.
    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        match crate::protocol::Request::decode(frame)? {
            crate::protocol::Request::SetAttackLeechRate(write) => Ok(write),
            _ => Err(ProtocolError::InvalidData),
        }
    }
}

/// Byte classification only: confirmation still requires host admission and an
/// independent fresh read. Trusted refusal codes precede the setter boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RateWriteReply {
    Confirmed(AttackLeechObservation),
    NotApplied { code: crate::protocol::ResponseCode, message: String },
    Uncertain { reason: String },
}
fn uncertain(reason: impl Into<String>) -> RateWriteReply {
    RateWriteReply::Uncertain { reason: reason.into() }
}
/// Reject malformed, unavailable or mismatched success payloads conservatively.
/// A failure to parse a refusal cannot prove nonexecution. No result authorizes retry.
pub fn classify_response(response: &crate::protocol::Response, request: &AttackLeechRateWrite) -> RateWriteReply {
    use crate::{
        fields::{self, AttackLeechRead},
        protocol::{MAX_RESPONSE_SIZE, ResponseCode},
        write::MAX_ERROR_TEXT,
    };
    if response.payload.len() > MAX_RESPONSE_SIZE {
        return uncertain("response exceeds frame bound");
    }
    if response.code == ResponseCode::Done {
        return match fields::decode(&response.payload) {
            Ok(AttackLeechRead::Available(actual)) if actual == request.projected() => RateWriteReply::Confirmed(actual),
            Ok(_) => uncertain("rate write readback differs from complete projected observation"),
            Err(_) => uncertain("invalid rate write readback payload"),
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
    RateWriteReply::NotApplied {
        code: response.code,
        message: message.into(),
    }
}
