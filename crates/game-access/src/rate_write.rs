//! Protocol28 conditional CURRENT-player request; no write authority or replay ledger.
//! Expected identities are comparison tokens only. Never dereference wire tokens.

use crate::{
    fields::{AttackLeechObservation, AttackLeechTarget, ObservedObjectIdentity, ObservedRate},
    protocol::{DispatcherSpec, MAGIC, ProtocolError, REQUEST_SIZE},
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
        let mut frame = vec![0; REQUEST_SIZE];
        frame[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        frame[4..8].copy_from_slice(&SET_ATTACK_LEECH_RATE_OPERATION.to_le_bytes());
        let target: u32 = match self.target {
            AttackLeechTarget::AttackRate => 1,
            AttackLeechTarget::IncreasedLeechRate => 2,
        };
        frame[8..12].copy_from_slice(&target.to_le_bytes());
        frame[12..16].copy_from_slice(&self.desired_bits.to_le_bytes());
        for (offset, identity) in [(16, self.expected.manager), (32, self.expected.actor), (72, self.expected.stats)] {
            frame[offset..offset + 8].copy_from_slice(&identity.wrapper_token().to_le_bytes());
            frame[offset + 8..offset + 16].copy_from_slice(&identity.native_token().to_le_bytes());
        }
        frame[48..56].copy_from_slice(&self.dispatcher.rva.to_le_bytes());
        frame[56..72].copy_from_slice(&self.dispatcher.bytes);
        frame[88..92].copy_from_slice(&self.expected.attack_rate.raw_bits.to_le_bytes());
        frame[92..96].copy_from_slice(&self.expected.increased_leech_rate.raw_bits.to_le_bytes());
        frame
    }

    /// Full fixed frame validation must precede helper ownership/dispatcher/queue
    /// admission. This decoder performs no I/O, float arithmetic or pointer access.
    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        if frame.len() != REQUEST_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let u32_at = |offset| u32::from_le_bytes(frame[offset..offset + 4].try_into().expect("fixed checked frame"));
        let u64_at = |offset| u64::from_le_bytes(frame[offset..offset + 8].try_into().expect("fixed checked frame"));
        if u32_at(0) != MAGIC {
            return Err(ProtocolError::IncompatibleVersion);
        }
        if u32_at(4) != SET_ATTACK_LEECH_RATE_OPERATION || frame[96..].iter().any(|byte| *byte != 0) {
            return Err(ProtocolError::InvalidData);
        }
        let target = match u32_at(8) {
            1 => AttackLeechTarget::AttackRate,
            2 => AttackLeechTarget::IncreasedLeechRate,
            _ => return Err(ProtocolError::InvalidData),
        };
        let expected = AttackLeechObservation {
            manager: ObservedObjectIdentity::new(u64_at(16), u64_at(24))?,
            actor: ObservedObjectIdentity::new(u64_at(32), u64_at(40))?,
            stats: ObservedObjectIdentity::new(u64_at(72), u64_at(80))?,
            attack_rate: ObservedRate { raw_bits: u32_at(88) },
            increased_leech_rate: ObservedRate { raw_bits: u32_at(92) },
        };
        let dispatcher = DispatcherSpec {
            rva: u64_at(48),
            bytes: frame[56..72].try_into().expect("fixed checked dispatcher"),
        };
        Self::new(dispatcher, expected, target, u32_at(12))
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
