//! The trainer's pages on the trainer's worker: the session is connected while the worker is Ready, and the one field set
//! this version serves (Combat › Attack and leech rates) is read and written through its commands, each answer arriving
//! with a later snapshot. Every other field set and every hook says, while connected, that this version cannot serve it
//! (`research/trainer-readiness.md`: the hook backend and the other six field sets are not ported). No game, pipe or
//! process work runs here; commands are queued and their receipts polled.

use super::session::{FieldError, FieldRequest, FieldResult, FieldWrite, GameField, HookMemory, HookState, Hooks, NOT_AVAILABLE, PendingFields, TrainerSession, not_connected};
use eg_trainer::backend::{
    AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, BackendError, BackendHandle, BackendSnapshot, Command, Delivery, ObservedRate, Pending, Phase,
};
use eg_trainer::field_writes::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, FieldInputError, FieldPlanError, RateWriteRecord};
use eg_trainer::writes::WriteOutcome;
use std::collections::VecDeque;

/// The catalog's id of the field set the worker serves.
pub const ATTACK_LEECH: &str = "attack-leech";

const BUSY: &str = "The trainer cannot accept another command.";

pub struct BackendSession {
    backend: BackendHandle,
    hooks: Hooks,
}

impl BackendSession {
    pub fn new(backend: BackendHandle, memory: HookMemory) -> BackendSession {
        BackendSession {
            backend,
            hooks: Hooks::new(memory),
        }
    }

    /// The worker's latest snapshot, while it runs.
    fn with<R>(&self, look: impl FnOnce(&BackendSnapshot) -> R) -> Option<R> {
        let updates = self.backend.subscribe();
        if updates.has_changed().is_err() {
            return None;
        }
        let snapshot = updates.borrow();
        Some(look(&snapshot))
    }
}

impl TrainerSession for BackendSession {
    fn is_connected(&self) -> bool {
        self.with(|snapshot| snapshot.phase == Phase::Ready).unwrap_or(false)
    }

    fn unavailable(&self, id: &str) -> Option<&'static str> {
        let served = id == ATTACK_LEECH && self.with(|snapshot| snapshot.capabilities.read_attack_leech).unwrap_or(false);
        (!served).then_some(NOT_AVAILABLE)
    }

    fn is_hook_wanted(&self, id: &str) -> bool {
        self.hooks.is_wanted(id)
    }

    fn set_hook(&self, id: &str, on: bool) {
        self.hooks.set(id, on);
    }

    fn hook_values(&self, id: &str) -> Vec<f64> {
        self.hooks.memory.get(id)
    }

    fn set_hook_values(&self, id: &str, values: &[f64]) {
        self.hooks.memory.set(id, values);
    }

    /// No hook is put in the game by this version (`unavailable` says so while connected).
    fn hook_state(&self, _id: &str) -> Option<HookState> {
        None
    }

    fn read_fields(&self, id: &str) -> FieldRequest {
        if !self.is_connected() {
            return not_connected();
        }
        if self.unavailable(id).is_some() {
            return refused(NOT_AVAILABLE);
        }
        match self.backend.try_submit(Command::ReadAttackLeech) {
            Ok(receipt) => FieldRequest::Pending(Box::new(PendingRead {
                backend: self.backend.clone(),
                receipt,
            })),
            Err(_) => refused(BUSY),
        }
    }

    fn write_fields(&self, id: &str, fields: &[FieldWrite]) -> FieldRequest {
        if !self.is_connected() {
            return not_connected();
        }
        if self.unavailable(id).is_some() || fields.len() != 2 {
            return refused(NOT_AVAILABLE);
        }
        // One field at a time, in the catalog's order; only the changed ones are sent.
        let steps = [AttackLeechTarget::AttackRate, AttackLeechTarget::IncreasedLeechRate]
            .into_iter()
            .zip(fields.iter().cloned())
            .filter(|(_, write)| write.changes())
            .collect();
        let mut write = PendingWrite {
            backend: self.backend.clone(),
            steps,
            current: None,
            sent: false,
        };
        match write.poll() {
            Some(result) => FieldRequest::Done(result),
            None => FieldRequest::Pending(Box::new(write)),
        }
    }
}

fn refused(details: &str) -> FieldRequest {
    FieldRequest::Done(Err(FieldError {
        details: details.to_owned(),
        nothing_sent: true,
    }))
}

/// A worker message as a sentence: capitalised, with its full stop.
pub(crate) fn sentence(message: &str) -> String {
    let mut chars = message.trim().chars();
    let mut text: String = chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default();
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

fn error_text(error: &BackendError) -> String {
    sentence(&error.message)
}

fn unavailable_text(reason: AttackLeechUnavailable) -> &'static str {
    match reason {
        AttackLeechUnavailable::OfflineRequired => "No offline character is connected.",
        AttackLeechUnavailable::ManagerAbsent | AttackLeechUnavailable::ManagerChanged => "The character changed in the game.",
        AttackLeechUnavailable::ActorCacheAbsent | AttackLeechUnavailable::StatsAbsent => "The character's stats are not loaded.",
        AttackLeechUnavailable::NativeObjectDestroyed | AttackLeechUnavailable::GraphChanged => "The character's stats changed while they were read.",
        AttackLeechUnavailable::UnsupportedStatsClass => "This character's stats are not supported.",
    }
}

fn plan_text(error: FieldPlanError) -> &'static str {
    match error {
        FieldPlanError::Unavailable(reason) => unavailable_text(reason),
        FieldPlanError::InvalidInput(FieldInputError::NonFinite) => "The value is not a number.",
        FieldPlanError::InvalidInput(FieldInputError::OutsideEditorRange) => "The value is outside the field's range.",
        FieldPlanError::StaleObservation => "The game changed since it was read. Read again first.",
        FieldPlanError::InvalidObservation | FieldPlanError::UnsupportedField => "The game's answer cannot be written to.",
    }
}

/// A rate as the editor shows it: the shortest decimal that is the game's float, so a value typed and kept reads back as
/// the same number.
fn decimal(rate: ObservedRate) -> Result<f64, FieldError> {
    let value = rate.finite_value().map_err(|_| FieldError {
        details: "The game holds a value that is not a number.".to_owned(),
        nothing_sent: true,
    })?;
    Ok(value.to_string().parse().unwrap_or(f64::from(value)))
}

fn fields(observation: &AttackLeechObservation) -> FieldResult {
    Ok(vec![
        GameField::new("", decimal(observation.attack_rate)?),
        GameField::new("", decimal(observation.increased_leech_rate)?),
    ])
}

/// The worker's current observation of the two rates, for this connection.
fn observed(snapshot: &BackendSnapshot) -> FieldResult {
    let current = snapshot
        .attack_leech
        .as_ref()
        .filter(|current| snapshot.phase == Phase::Ready && current.generation == snapshot.generation && snapshot.attached.as_ref() == Some(&current.process));
    let fail = |details: &str| {
        Err(FieldError {
            details: details.to_owned(),
            nothing_sent: true,
        })
    };
    match current.map(|current| &current.read) {
        Some(AttackLeechRead::Available(observation)) => fields(observation),
        Some(AttackLeechRead::Unavailable(reason)) => fail(unavailable_text(*reason)),
        None => fail("The game did not answer."),
    }
}

/// Exact: the field holds the very number the editor read.
#[allow(clippy::float_cmp)]
fn same(held: f64, read: f64) -> bool {
    held == read
}

struct PendingRead {
    backend: BackendHandle,
    receipt: Pending,
}

impl PendingFields for PendingRead {
    fn poll(&mut self) -> Option<FieldResult> {
        Some(match self.receipt.try_result()? {
            Ok(()) => observed(&self.backend.snapshot()),
            Err(error) => Err(FieldError {
                details: error_text(&error),
                nothing_sent: true,
            }),
        })
    }
}

/// A write of the changed rates, one conditional plan each, every plan made from the observation the previous one left.
/// It stops at the first that is refused, not kept or uncertain; nothing is ever sent again.
struct PendingWrite {
    backend: BackendHandle,
    steps: VecDeque<(AttackLeechTarget, FieldWrite)>,
    /// The plan on its way, and the worker's record from before it (an earlier identical plan's record is not its answer).
    current: Option<(Pending, AttackLeechPlan, Option<RateWriteRecord>)>,
    /// A plan has been confirmed: a later refusal no longer means nothing was sent.
    sent: bool,
}

impl PendingWrite {
    fn refuse(&self, details: impl Into<String>) -> FieldResult {
        Err(FieldError {
            details: details.into(),
            nothing_sent: !self.sent,
        })
    }

    fn uncertain(details: impl Into<String>) -> FieldResult {
        Err(FieldError {
            details: details.into(),
            nothing_sent: false,
        })
    }

    /// The answer to the plan on its way, once the worker has given it; None to go on to the next field.
    fn answer(&mut self, result: Result<(), BackendError>) -> Option<FieldResult> {
        let (_, plan, before) = self.current.take()?;
        let snapshot = self.backend.snapshot();
        let record = snapshot
            .last_rate_write
            .as_ref()
            .filter(|record| record.plan == plan && (result.is_ok() || before.as_ref() != Some(*record)));
        match (record.map(|record| &record.status), result) {
            (Some(WriteOutcome::Confirmed(_)), _) => {
                self.sent = true;
                None
            }
            (Some(WriteOutcome::NotKept(actual)), _) => Some(fields(actual).or_else(|error| Self::uncertain(error.details))),
            (Some(WriteOutcome::NotApplied { message, .. }), _) => Some(self.refuse(sentence(message))),
            (Some(WriteOutcome::Uncertain { message }), _) => Some(Self::uncertain(sentence(message))),
            (Some(WriteOutcome::Pending), _) | (None, Ok(())) => Some(Self::uncertain("The worker ended without the write's outcome.")),
            (None, Err(error)) if error.delivery == Some(Delivery::MayHaveRun) => Some(Self::uncertain(error_text(&error))),
            (None, Err(error)) => Some(self.refuse(error_text(&error))),
        }
    }
}

impl PendingFields for PendingWrite {
    fn poll(&mut self) -> Option<FieldResult> {
        loop {
            if let Some((receipt, _, _)) = self.current.as_mut() {
                let result = receipt.try_result()?;
                if let Some(answer) = self.answer(result) {
                    return Some(answer);
                }
            }
            let snapshot = self.backend.snapshot();
            let Some((target, write)) = self.steps.pop_front() else {
                return Some(observed(&snapshot).or_else(|error| Self::uncertain(error.details)));
            };
            if snapshot.phase != Phase::Ready {
                return Some(self.refuse("No offline character is connected."));
            }
            // Sent only while the field still holds what was read.
            let held = match observed(&snapshot) {
                Ok(read) => read[usize::from(target == AttackLeechTarget::IncreasedLeechRate)].value,
                Err(error) => return Some(self.refuse(error.details)),
            };
            if !same(held, write.read) {
                return Some(self.refuse(format!("{} changed in the game since it was read. Read again first.", write.label)));
            }
            match snapshot.attack_leech_plan(target, write.wanted) {
                Ok(AttackLeechDecision::NoOp) => {}
                Ok(AttackLeechDecision::Change(plan)) => match self.backend.try_submit(Command::SetAttackLeechRate(Box::new(plan.clone()))) {
                    Ok(receipt) => self.current = Some((receipt, plan, snapshot.last_rate_write.clone())),
                    Err(_) => return Some(self.refuse(BUSY)),
                },
                Err(error) => return Some(self.refuse(plan_text(error))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_rates_read_as_the_editor_shows_them() {
        assert_eq!(sentence("session is not ready"), "Session is not ready.");
        assert_eq!(sentence("Owned refusal."), "Owned refusal.");
        assert_eq!(decimal(ObservedRate { raw_bits: 1.1_f32.to_bits() }), Ok(1.1));
        assert_eq!(decimal(ObservedRate { raw_bits: 0.01_f32.to_bits() }), Ok(0.01));
        assert!(decimal(ObservedRate { raw_bits: f32::NAN.to_bits() }).is_err());
    }
}
