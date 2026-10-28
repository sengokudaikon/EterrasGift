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
}

/// Byte classification only: confirmation still requires host admission and an
/// independent fresh read. Trusted refusal codes precede the setter boundary.
pub type RateWriteReply = crate::write::Reply<AttackLeechObservation>;

/// Reject malformed, unavailable or mismatched success payloads conservatively.
pub fn classify_response(response: &crate::protocol::Response, request: &AttackLeechRateWrite) -> RateWriteReply {
    use crate::fields::{self, AttackLeechRead};
    crate::write::classify(response, |payload| match fields::decode(payload) {
        Ok(AttackLeechRead::Available(actual)) if actual == request.projected() => Ok(actual),
        Ok(_) => Err("rate write readback differs from complete projected observation"),
        Err(_) => Err("invalid rate write readback payload"),
    })
}
