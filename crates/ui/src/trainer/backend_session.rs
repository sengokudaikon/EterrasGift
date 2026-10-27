//! The trainer's pages on the trainer's worker: the session is connected while the worker is Ready, and the one field set
//! this version serves (Combat › Attack and leech rates) is read and written through its commands, each answer arriving
//! with a later snapshot. The hooks are the worker's profiles: the shown one's switches and values come from its snapshot
//! (or, for a character in play without one, this connection's), and each change is a command. Hook availability
//! comes from the connected worker's capabilities. No game, pipe or process work runs here; commands are queued
//! and their receipts polled.

use super::profiles::Profile;
use super::session::{FieldError, FieldRequest, FieldResult, FieldWrite, GameField, HookState, NOT_AVAILABLE, PendingFields, TrainerSession, not_connected};
use eg_trainer::backend::{
    AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, BackendError, BackendHandle, BackendSnapshot, Command, Delivery, HookRun, ObservedRate, Pending, Phase,
};
use eg_trainer::features::{FeatureIntents, HookId};
use eg_trainer::field_writes::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, FieldInputError, FieldPlanError, RateWriteRecord};
use eg_trainer::profiles::ProfileKey;
use eg_trainer::writes::WriteOutcome;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

/// The catalog's id of the field set the worker serves.
pub const ATTACK_LEECH: &str = "attack-leech";

const BUSY: &str = "The trainer cannot accept another command.";

pub struct BackendSession {
    backend: BackendHandle,
    /// Switches flipped and not yet taken by the worker: until they are, the row shows them as flipped.
    switching: RefCell<BTreeMap<HookId, (Pending, bool)>>,
    /// A profile chosen and not yet shown by the worker.
    selecting: RefCell<Option<(Pending, ProfileKey)>>,
    /// Other changes on their way, held only to know when they are taken.
    changing: RefCell<Vec<Pending>>,
}

impl BackendSession {
    pub fn new(backend: BackendHandle) -> BackendSession {
        BackendSession {
            backend,
            switching: RefCell::default(),
            selecting: RefCell::default(),
            changing: RefCell::default(),
        }
    }

    /// Queues a change whose answer the next snapshot shows; a full queue drops it, and the row keeps what the profile holds.
    fn submit(&self, command: Command) {
        if let Ok(receipt) = self.backend.try_submit(command) {
            self.changing.borrow_mut().push(receipt);
        }
    }

    /// Whose hooks the pages show, and the hooks: the profile chosen, until the worker has taken the choice, else the
    /// worker's (a profile, or the hooks of the character in play without one).
    fn shown<'a>(&self, snapshot: &'a BackendSnapshot) -> Option<(ProfileKey, &'a FeatureIntents)> {
        self.settle();
        if let Some((_, key)) = self.selecting.borrow().as_ref() {
            return snapshot.profiles.get(key).map(|profile| (key.clone(), &profile.hooks));
        }
        Some((snapshot.editing.clone()?, snapshot.shown_hooks()?))
    }

    /// Lets go of the changes the worker has taken: from then on the snapshot says what they did.
    fn settle(&self) {
        self.switching.borrow_mut().retain(|_, (receipt, _)| receipt.try_result().is_none());
        let mut selecting = self.selecting.borrow_mut();
        if selecting.as_mut().is_some_and(|(receipt, _)| receipt.try_result().is_some()) {
            *selecting = None;
        }
        self.changing.borrow_mut().retain_mut(|receipt| receipt.try_result().is_none());
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
        let served = self
            .with(|snapshot| {
                use eg_trainer::features::{FeatureKey, NativeSupport};
                if id == ATTACK_LEECH {
                    return snapshot.capabilities.read_attack_leech;
                }
                match FeatureKey::parse(id).map(|key| key.descriptor().native_support) {
                    Some(NativeSupport::BooleanOverride(hook)) => snapshot.capabilities.boolean_hooks[hook as usize],
                    _ => false,
                }
            })
            .unwrap_or(false);
        (!served).then_some(NOT_AVAILABLE)
    }

    fn profiles(&self) -> Vec<Profile> {
        self.with(|snapshot| snapshot.profiles.profiles().iter().map(|profile| profile_of(&profile.key)).collect())
            .unwrap_or_default()
    }

    fn shown_profile(&self) -> Option<Profile> {
        self.with(|snapshot| self.shown(snapshot).map(|(key, _)| profile_of(&key))).flatten()
    }

    fn is_profile_kept(&self) -> bool {
        self.with(|snapshot| self.shown(snapshot).is_none_or(|(key, _)| snapshot.profiles.get(&key).is_some()))
            .unwrap_or(true)
    }

    fn keep_profile(&self) {
        self.submit(Command::CreateProfile);
    }

    fn select_profile(&self, profile: &Profile) {
        let key = key_of(profile);
        if let Ok(receipt) = self.backend.try_submit(Command::SelectProfile(key.clone())) {
            *self.selecting.borrow_mut() = Some((receipt, key));
        }
    }

    fn delete_profile(&self, profile: &Profile) {
        self.submit(Command::DeleteProfile(key_of(profile)));
    }

    fn is_hook_wanted(&self, id: &str) -> bool {
        let Some(id) = HookId::parse(id) else {
            return false;
        };
        self.settle();
        if let Some((_, on)) = self.switching.borrow().get(&id) {
            return *on;
        }
        self.with(|snapshot| self.shown(snapshot).is_some_and(|(_, hooks)| hooks.is_wanted(id))).unwrap_or(false)
    }

    fn set_hook(&self, id: &str, on: bool) {
        let Some(id) = HookId::parse(id) else {
            return;
        };
        if let Ok(receipt) = self.backend.try_submit(Command::SetHookWanted { id, on }) {
            self.switching.borrow_mut().insert(id, (receipt, on));
        }
    }

    fn hook_values(&self, id: &str) -> Vec<f64> {
        let Some(id) = HookId::parse(id) else {
            return Vec::new();
        };
        self.with(|snapshot| self.shown(snapshot).map(|(_, hooks)| hooks.values(id).to_vec()))
            .flatten()
            .unwrap_or_default()
    }

    fn set_hook_values(&self, id: &str, values: &[f64]) {
        if let Some(id) = HookId::parse(id) {
            let command = Command::SetHookValues { id, values: values.to_vec() };
            if eg_trainer::features::FeatureKey::Hook(id).descriptor().kind == eg_trainer::features::FeatureKind::Choice {
                if let Ok(receipt) = self.backend.try_submit(command) {
                    self.switching.borrow_mut().insert(id, (receipt, values != [0.0]));
                }
            } else {
                self.submit(command);
            }
        }
    }

    fn hook_state(&self, id: &str) -> Option<HookState> {
        let id = HookId::parse(id)?;
        self.settle();
        if self.switching.borrow().contains_key(&id) {
            return Some(HookState::Applying);
        }
        self.with(|snapshot| snapshot.hooks.get(&id).map(hook_state)).flatten()
    }

    fn is_busy(&self) -> bool {
        self.settle();
        !self.switching.borrow().is_empty() || self.selecting.borrow().is_some() || !self.changing.borrow().is_empty()
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

fn profile_of(key: &ProfileKey) -> Profile {
    Profile {
        name: key.name.clone(),
        class: key.class.clone(),
        cycle: key.cycle.clone(),
    }
}

fn key_of(profile: &Profile) -> ProfileKey {
    ProfileKey {
        name: profile.name.clone(),
        class: profile.class.clone(),
        cycle: profile.cycle.clone(),
    }
}

fn hook_state(run: &HookRun) -> HookState {
    match run {
        HookRun::Off => HookState::Off,
        HookRun::Queued => HookState::Queued,
        HookRun::Applying => HookState::Applying,
        HookRun::On => HookState::On,
        HookRun::Failed { reason, on } => HookState::Failed { reason: reason.clone(), on: *on },
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
