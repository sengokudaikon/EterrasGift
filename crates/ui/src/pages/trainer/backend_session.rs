//! Trainer pages on the worker's owned snapshots. Independent hook controls use the shared profile/change/readback
//! path. Unported direct field editors keep the common interface but advertise no operation support.

use super::profiles::Profile;
use super::session::{FieldError, FieldRequest, FieldWrite, HookState, NotAvailable, TrainerSession};
use eg_trainer::backend::{BackendHandle, BackendSnapshot, Command, Pending, Phase};
use eg_trainer::features::profiles::ProfileKey;
use eg_trainer::features::{FeatureIntents, HookId};
use std::cell::RefCell;
use std::collections::BTreeMap;

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
            let descriptor = eg_trainer::features::FeatureKey::Hook(id).descriptor();
            let choice = descriptor.kind == eg_trainer::features::FeatureKind::Choice;
            if choice || matches!(descriptor.native_support(), eg_trainer::features::NativeSupport::GameplayOverride(_)) {
                let on = if choice { values != [0.0] } else { self.is_hook_wanted(id.as_str()) };
                if let Ok(receipt) = self.backend.try_submit(command) {
                    self.switching.borrow_mut().insert(id, (receipt, on));
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
        self.with(|snapshot| snapshot.hooks.get(&id).cloned()).flatten()
    }

    fn is_busy(&self) -> bool {
        self.settle();
        !self.switching.borrow().is_empty() || self.selecting.borrow().is_some() || !self.changing.borrow().is_empty()
    }

    fn read_fields(&self, _: &str) -> FieldRequest {
        refused(NotAvailable.get())
    }
    fn write_fields(&self, _: &str, _: &[FieldWrite]) -> FieldRequest {
        refused(NotAvailable.get())
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
