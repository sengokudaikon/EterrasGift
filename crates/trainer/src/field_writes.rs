//! Pure planning for one attack/leech field, following `native/hooks/combat.h` and the immutable
//! `attack-leech` editor descriptors. This module performs no I/O and grants no native write authority.
//!
//! Bounds are the source editor's doubles, checked before its float projection. UI steps never
//! quantize an input. Nonfinite inputs are rejected as in the original runFields entrypoint.
//! The source's `shortestDecimal(float)` is a display conversion: widening an f32 to f64
//! is not the same decimal that the user typed (e.g. 1.1 or the 0.01 lower bound). Neither display text
//! nor widened decimal equality replaces raw-bit observation comparisons.
//!
//! The backend and helper independently admit the exact process/helper and offline main thread,
//! re-resolve and root the manager/actor/stats graph, recheck every wrapper/native token and both
//! raw rates immediately before one validated f32 store, and perform a fresh read for confirmation.
//! This plan is not an Undo token. Uncertain writes are never retried.
//! Equality detects observed drift, not ABA between reads: wrappers can die and addresses can be
//! reused. Process/generation/sequence and token equality are not durable selection or lifetime
//! authority across a UI wait. This layer creates no retained-root ticket.

use crate::{
    backend::{AttackLeechObservation, AttackLeechRead, AttackLeechSnapshot, AttackLeechUnavailable},
    features::{FeatureField, FeatureKey, FieldSetId},
};

/// The only two fields this pure planner understands. It adds no endpoint or capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackLeechTarget {
    AttackRate,
    IncreasedLeechRate,
}

impl AttackLeechTarget {
    /// Translate an existing descriptor field selector without accepting any other field set.
    pub fn from_field(set: FieldSetId, index: usize) -> Result<Self, FieldPlanError> {
        match (set.as_str(), index) {
            ("attack-leech", 0) => Ok(Self::AttackRate),
            ("attack-leech", 1) => Ok(Self::IncreasedLeechRate),
            _ => Err(FieldPlanError::UnsupportedField),
        }
    }

    pub fn descriptor(self) -> &'static FeatureField {
        let index = match self {
            Self::AttackRate => 0,
            Self::IncreasedLeechRate => 1,
        };
        &FeatureKey::parse("attack-leech").expect("immutable attack-leech descriptor").descriptor().fields[index]
    }

    fn bits(self, observation: &AttackLeechObservation) -> u32 {
        match self {
            Self::AttackRate => observation.attack_rate.raw_bits,
            Self::IncreasedLeechRate => observation.increased_leech_rate.raw_bits,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldInputError {
    NonFinite,
    OutsideEditorRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldPlanError {
    UnsupportedField,
    Unavailable(AttackLeechUnavailable),
    InvalidObservation,
    InvalidInput(FieldInputError),
    StaleObservation,
}

impl std::fmt::Display for FieldPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "attack/leech plan: {self:?}")
    }
}
impl std::error::Error for FieldPlanError {}

/// Equal projected target bits are explicitly a no-op. Numerically equal signed zeroes with
/// different bits are a change; two different f64 inputs that project to identical f32 bits are not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttackLeechDecision {
    NoOp,
    Change(AttackLeechPlan),
}

/// An immutable local projection, not permission to dereference tokens or execute a native store.
/// A publicly constructible snapshot is input data, not runtime admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackLeechPlan {
    before: Box<AttackLeechSnapshot>,
    target: AttackLeechTarget,
    requested_bits: u64,
    projected: AttackLeechObservation,
}

impl AttackLeechPlan {
    pub fn prepare(expected: &AttackLeechSnapshot, target: AttackLeechTarget, value: f64) -> Result<AttackLeechDecision, FieldPlanError> {
        if expected.process.pid == 0
            || expected.process.created == 0
            || !expected.process.executable.is_absolute()
            || expected.generation == 0
            || expected.sequence == 0
            || expected.manager == 0
        {
            return Err(FieldPlanError::InvalidObservation);
        }
        let before = match expected.read {
            AttackLeechRead::Available(before) => before,
            AttackLeechRead::Unavailable(reason) => return Err(FieldPlanError::Unavailable(reason)),
        };
        if before.manager.wrapper_token() != expected.manager {
            return Err(FieldPlanError::InvalidObservation);
        }
        if !value.is_finite() {
            return Err(FieldPlanError::InvalidInput(FieldInputError::NonFinite));
        }
        let descriptor = target.descriptor();
        if value < descriptor.minimum || value > descriptor.maximum {
            return Err(FieldPlanError::InvalidInput(FieldInputError::OutsideEditorRange));
        }
        // Bounds above are intentionally f64. In particular, 0.01_f64 is accepted even though
        // its projected f32 widens to a double slightly below the editor's original lower bound.
        let projected_bits = (value as f32).to_bits();
        if target.bits(&before) == projected_bits {
            return Ok(AttackLeechDecision::NoOp);
        }
        let mut projected = before;
        match target {
            AttackLeechTarget::AttackRate => projected.attack_rate.raw_bits = projected_bits,
            AttackLeechTarget::IncreasedLeechRate => projected.increased_leech_rate.raw_bits = projected_bits,
        }
        Ok(AttackLeechDecision::Change(Self {
            before: Box::new(expected.clone()),
            target,
            requested_bits: value.to_bits(),
            projected,
        }))
    }

    pub fn before(&self) -> &AttackLeechSnapshot {
        &self.before
    }
    pub fn target(&self) -> AttackLeechTarget {
        self.target
    }
    pub fn requested_value(&self) -> f64 {
        f64::from_bits(self.requested_bits)
    }
    pub fn projected(&self) -> &AttackLeechObservation {
        &self.projected
    }
    pub fn projected_bits(&self) -> u32 {
        self.target.bits(&self.projected)
    }
    pub fn projected_value(&self) -> f32 {
        f32::from_bits(self.projected_bits())
    }

    /// Recheck the entire observation, including its sequence even if the native tokens/bits
    /// happen to match again. An unavailable or malformed replacement is also stale. Success
    /// proves only owned-data equality; it cannot establish current native liveness or admission.
    pub fn recheck(&self, current: &AttackLeechSnapshot) -> Result<(), FieldPlanError> {
        if current == self.before.as_ref() {
            Ok(())
        } else {
            Err(FieldPlanError::StaleObservation)
        }
    }
}

pub type RateWriteStatus = crate::writes::WriteOutcome<AttackLeechObservation>;
/// Owned historical outcome. No roots, native pointers, Undo or replay authority.
pub type RateWriteRecord = crate::writes::WriteRecord<AttackLeechPlan, AttackLeechObservation>;
