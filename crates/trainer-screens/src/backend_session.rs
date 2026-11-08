//! Trainer pages on the worker's owned snapshots. Independent hook controls use the shared profile/change/readback
//! path. Numeric editors share typed read/apply receipts and independent readback.

use super::profiles::Profile;
use super::session::{FieldError, FieldRequest, FieldWrite, GameField, HookState, NotAvailable, Settling, TrainerSession};
use eg_trainer::backend::{BackendHandle, BackendSnapshot, Command, Pending, Phase};
use eg_trainer::features::profiles::ProfileKey;
use eg_trainer::features::{FeatureIntents, HookId};
use phrases::phrases;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

phrases! {
    ReadFieldsFirst: "Read these fields before applying changes"
    FieldsChanged: "The field observation changed; read again"
    FieldRange: "A value must be a whole number from 0 to 2147483647"
    FieldsGone: "The field observation is no longer available; read again"
    WorkerStopped: "The trainer worker stopped."
    WorkerBusy: "Wait for the current trainer operation to finish."
}

pub struct BackendSession {
    backend: BackendHandle,
    changes: Rc<RefCell<Changes>>,
    /// Receipts' waits not yet handed to whoever follows the session.
    settling: RefCell<Vec<Settling>>,
}

/// The changes the worker hasn't taken yet. From when it has, the snapshot says what they did.
#[derive(Default)]
struct Changes {
    /// Switches flipped: until they are taken, the row shows them as flipped. A later flip of the hook replaces an earlier one.
    switching: BTreeMap<HookId, (u64, bool)>,
    /// A profile chosen and not yet shown by the worker. A later choice replaces an earlier one.
    selecting: Option<(u64, ProfileKey)>,
    /// Other changes on their way.
    changing: usize,
    last: u64,
}

struct FieldsPending(Rc<RefCell<Changes>>);
impl Drop for FieldsPending {
    fn drop(&mut self) {
        self.0.borrow_mut().changing -= 1;
    }
}

impl Changes {
    fn next(&mut self) -> u64 {
        self.last += 1;
        self.last
    }
}

impl BackendSession {
    pub fn new(backend: BackendHandle) -> BackendSession {
        BackendSession {
            backend,
            changes: Rc::default(),
            settling: RefCell::default(),
        }
    }
    fn field_request(&self, set: eg_trainer_wire::field_edits::FieldSet, command: Command) -> FieldRequest {
        let receipt = match self.backend.try_submit(command) {
            Ok(receipt) => receipt,
            Err(error) => {
                return refused(match error {
                    eg_trainer::backend::SubmitError::Full => WorkerBusy.get(),
                    eg_trainer::backend::SubmitError::Stopped => WorkerStopped.get(),
                });
            }
        };
        let backend = self.backend.clone();
        let changes = self.changes.clone();
        changes.borrow_mut().changing += 1;
        let pending = FieldsPending(changes);
        FieldRequest::Pending(Box::pin(async move {
            let _pending = pending;
            let result = receipt.wait().await;
            result.map_err(|error| FieldError {
                details: error.message,
                nothing_sent: error.delivery != Some(eg_trainer::backend::Delivery::MayHaveRun),
            })?;
            backend
                .peek(|snapshot| {
                    snapshot
                        .fields
                        .get(&set)
                        .map(|read| read.snapshot.fields.iter().map(|field| GameField::new(&field.label, f64::from(field.value))).collect())
                })
                .ok_or_else(|| FieldError {
                    details: FieldsGone.into(),
                    nothing_sent: false,
                })
        }))
    }

    /// Waits for `receipt`, whatever its answer: the snapshot says what the change did. `taken` then lets go of it.
    fn track(&self, receipt: Pending, taken: impl FnOnce(&mut Changes) + 'static) {
        let changes = self.changes.clone();
        self.settling.borrow_mut().push(Box::pin(async move {
            let _ = receipt.wait().await;
            taken(&mut changes.borrow_mut());
        }));
    }

    /// Queues a change whose answer the next snapshot shows; a full queue drops it, and the row keeps what the profile holds.
    fn submit(&self, command: Command) {
        if let Ok(receipt) = self.backend.try_submit(command) {
            self.changes.borrow_mut().changing += 1;
            self.track(receipt, |changes| changes.changing -= 1);
        }
    }

    /// Queues a change that flips the hook's switch; a full queue drops it, and the row keeps what the profile holds.
    fn switch(&self, id: HookId, on: bool, command: Command) {
        if let Ok(receipt) = self.backend.try_submit(command) {
            let serial = {
                let mut changes = self.changes.borrow_mut();
                let serial = changes.next();
                changes.switching.insert(id, (serial, on));
                serial
            };
            self.track(receipt, move |changes| {
                if changes.switching.get(&id).is_some_and(|(latest, _)| *latest == serial) {
                    changes.switching.remove(&id);
                }
            });
        }
    }

    /// Whose hooks the pages show, and the hooks: the profile chosen, until the worker has taken the choice, else the
    /// worker's (a profile, or the hooks of the character in play without one).
    fn shown<'a>(&self, snapshot: &'a BackendSnapshot) -> Option<(ProfileKey, &'a FeatureIntents)> {
        if let Some((_, key)) = self.changes.borrow().selecting.as_ref() {
            return snapshot.profiles.get(key).map(|profile| (key.clone(), &profile.hooks));
        }
        Some((snapshot.editing.clone()?, snapshot.shown_hooks()?))
    }

    /// The worker's latest snapshot, while it runs.
    fn with<R>(&self, look: impl FnOnce(&BackendSnapshot) -> R) -> Option<R> {
        self.backend.is_running().then(|| self.backend.peek(look))
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
                match FeatureKey::parse(id).map(|key| key.descriptor().native_support()) {
                    Some(NativeSupport::GameplayOverride(hook)) => snapshot.capabilities.gameplay_hooks[hook as usize],
                    Some(NativeSupport::FieldEditor(set)) => snapshot.capabilities.fields[set as usize],
                    _ => false,
                }
            })
            .unwrap_or(false);
        (!served).then_some(NotAvailable.get())
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
            let serial = {
                let mut changes = self.changes.borrow_mut();
                let serial = changes.next();
                changes.selecting = Some((serial, key));
                serial
            };
            self.track(receipt, move |changes| {
                if changes.selecting.as_ref().is_some_and(|(latest, _)| *latest == serial) {
                    changes.selecting = None;
                }
            });
        }
    }

    fn delete_profile(&self, profile: &Profile) {
        self.submit(Command::DeleteProfile(key_of(profile)));
    }

    fn is_hook_wanted(&self, id: &str) -> bool {
        let Some(id) = HookId::parse(id) else {
            return false;
        };
        if let Some((_, on)) = self.changes.borrow().switching.get(&id) {
            return *on;
        }
        self.with(|snapshot| self.shown(snapshot).is_some_and(|(_, hooks)| hooks.is_wanted(id))).unwrap_or(false)
    }

    fn set_hook(&self, id: &str, on: bool) {
        let Some(id) = HookId::parse(id) else {
            return;
        };
        self.switch(id, on, Command::SetHookWanted { id, on });
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
            let descriptor = eg_trainer::features::FeatureKey::Hook(id).descriptor();
            let choice = descriptor.kind == eg_trainer::features::FeatureKind::Choice;
            if choice || matches!(descriptor.native_support(), eg_trainer::features::NativeSupport::GameplayOverride(_)) {
                let on = if choice { values != [0.0] } else { self.is_hook_wanted(id.as_str()) };
                self.switch(id, on, command);
            } else {
                self.submit(command);
            }
        }
    }

    fn hook_state(&self, id: &str) -> Option<HookState> {
        let id = HookId::parse(id)?;
        if self.changes.borrow().switching.contains_key(&id) {
            return Some(HookState::Applying);
        }
        self.with(|snapshot| snapshot.hooks.get(&id).cloned()).flatten()
    }

    fn is_busy(&self) -> bool {
        let changes = self.changes.borrow();
        !changes.switching.is_empty() || changes.selecting.is_some() || changes.changing > 0
    }

    fn settling(&self) -> Vec<Settling> {
        self.settling.take()
    }

    fn read_fields(&self, id: &str) -> FieldRequest {
        let Some(set) = eg_trainer_wire::field_edits::FieldSet::from_id(id) else {
            return refused(NotAvailable.get());
        };
        self.field_request(set, Command::ReadFields(set))
    }
    fn write_fields(&self, id: &str, fields: &[FieldWrite]) -> FieldRequest {
        let Some(set) = eg_trainer_wire::field_edits::FieldSet::from_id(id) else {
            return refused(NotAvailable.get());
        };
        let Some(expected) = self.backend.peek(|snapshot| snapshot.fields.get(&set).cloned()) else {
            return refused(ReadFieldsFirst.get());
        };
        if fields.len() != expected.snapshot.fields.len()
            || fields
                .iter()
                .zip(&expected.snapshot.fields)
                .any(|(write, read)| write.read != f64::from(read.value) || (set.held_items() && write.label != read.label))
        {
            return refused(FieldsChanged.get());
        }
        let mut values = Vec::with_capacity(fields.len());
        for field in fields {
            if !field.wanted.is_finite() || field.wanted.fract() != 0.0 || !(0.0..=f64::from(i32::MAX)).contains(&field.wanted) {
                return refused(FieldRange.get());
            }
            values.push(field.changes().then_some(field.wanted as i32));
        }
        self.field_request(
            set,
            Command::WriteFields {
                expected: Box::new(expected),
                values,
            },
        )
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
