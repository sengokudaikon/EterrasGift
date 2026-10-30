//! Cheat profiles in the worker (`crate::features::profiles`): which one the pages show and change, and the lock that
//! holds them still while a game runs without an offline character in play. A character without a profile has hooks
//! for the connection only, until the user keeps them as its profile. Applying them is `hooks.rs`'s.
use super::hooks::{HookRun, hook_ids};
use super::write::invalid_edit;
use super::*;
use crate::features::profiles::ProfileKey;
use crate::features::{FeatureError, FeatureIntents, FeatureKey, HookId};

/// Whether the profiles can change, and which one is live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileLock {
    /// No game runs: any profile can be shown, changed or deleted.
    Editable,
    /// A game runs without an offline character in play (the connection's phase says why): the profiles wait.
    Locked,
    /// The character is in play: it is the one shown, and each change is applied as it is made.
    Live(ProfileKey),
}

fn locked() -> BackendError {
    BackendError::blocked(ErrorKind::NotReady, "Profiles change while no game runs, or for the offline character in play.")
}

fn not_in_play() -> BackendError {
    BackendError::blocked(ErrorKind::NotReady, "A profile is kept for the offline character in play.")
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) fn profile_lock(&self) -> ProfileLock {
        match &self.live {
            Some(key) if self.state.phase == Phase::Ready => ProfileLock::Live(key.clone()),
            _ if self.state.enabled && (self.games_running || self.connection.is_some()) => ProfileLock::Locked,
            _ => ProfileLock::Editable,
        }
    }

    pub(super) fn select_profile(&mut self, key: ProfileKey) -> Result<(), BackendError> {
        if self.profile_lock() != ProfileLock::Editable {
            return Err(locked());
        }
        if self.state.profiles.get(&key).is_none() {
            return Err(invalid_edit("no such profile"));
        }
        self.state.editing = Some(key);
        self.publish();
        Ok(())
    }

    /// Keeps the hooks of the character in play as its profile, saved from now on.
    pub(super) fn create_profile(&mut self) -> Result<(), BackendError> {
        let ProfileLock::Live(key) = self.profile_lock() else {
            return Err(not_in_play());
        };
        let Some(hooks) = self.state.unsaved.take() else {
            return Err(invalid_edit("the character in play has a profile"));
        };
        Arc::make_mut(&mut self.state.profiles).create(key, Arc::unwrap_or_clone(hooks), self.clock.now());
        self.save_profiles();
        self.publish();
        Ok(())
    }

    pub(super) fn delete_profile(&mut self, key: ProfileKey) -> Result<(), BackendError> {
        let live = match self.profile_lock() {
            ProfileLock::Editable => false,
            ProfileLock::Live(live) if live == key => true,
            _ => return Err(locked()),
        };
        let profile = Arc::make_mut(&mut self.state.profiles).remove(&key).ok_or_else(|| invalid_edit("no such profile"))?;
        if live {
            // Nothing changes in the game: the character goes on with its hooks, for this connection only.
            self.state.unsaved = Some(Arc::new(profile.hooks));
        } else if self.state.editing.as_ref() == Some(&key) {
            self.state.editing = self.state.profiles.profiles().first().map(|profile| profile.key.clone());
        }
        self.save_profiles();
        self.publish();
        Ok(())
    }

    pub(super) async fn set_hook_wanted(&mut self, id: HookId, on: bool) -> Result<(), BackendError> {
        let (changed, live) = self.change_profile(|hooks| Ok(hooks.set_wanted(id, on)))?;
        if changed && live {
            self.apply_hook(self.command_epoch, id).await?;
        }
        Ok(())
    }

    /// Validated strictly: values of the wrong count or out of range are refused whole.
    pub(super) async fn set_hook_values(&mut self, id: HookId, values: Vec<f64>) -> Result<(), BackendError> {
        let (changed, live) = self.change_profile(|hooks| hooks.set_values(id, &values))?;
        let applied =
            live && (FeatureKey::Hook(id).descriptor().kind == crate::features::FeatureKind::Choice || self.wanted(id) || self.state.hooks.get(&id).is_some_and(HookRun::holds));
        if changed && applied {
            self.apply_hook(self.command_epoch, id).await?;
        }
        Ok(())
    }

    /// Changes the shown hooks, saving them when they are a profile's. Whether they changed, and whether their
    /// character is in play.
    fn change_profile(&mut self, change: impl FnOnce(&mut FeatureIntents) -> Result<bool, FeatureError>) -> Result<(bool, bool), BackendError> {
        let lock = self.profile_lock();
        let (hooks, saved) = match &lock {
            ProfileLock::Locked => return Err(locked()),
            ProfileLock::Editable => {
                let key = self.state.editing.as_ref().ok_or_else(|| {
                    BackendError::blocked(
                        ErrorKind::NotReady,
                        "No profiles yet: keep one for an offline character while it is in play with the trainer connected.",
                    )
                })?;
                (Arc::make_mut(&mut self.state.profiles).hooks_mut(key), true)
            }
            ProfileLock::Live(key) => match self.state.unsaved.as_mut() {
                Some(unsaved) => (Some(Arc::make_mut(unsaved)), false),
                None => (Arc::make_mut(&mut self.state.profiles).hooks_mut(key), true),
            },
        };
        let changed = change(hooks.ok_or_else(|| invalid_edit("no such profile"))?).map_err(invalid_edit)?;
        if changed {
            if saved {
                self.save_profiles();
            }
            self.publish();
        }
        Ok((changed, matches!(lock, ProfileLock::Live(_))))
    }

    /// The hooks of the character in play: its profile's, or this connection's.
    pub(super) fn live_hooks(&self) -> Option<&FeatureIntents> {
        let key = self.live.as_ref()?;
        self.state.profiles.get(key).map(|profile| &profile.hooks).or(self.state.unsaved.as_deref())
    }

    pub(super) fn wanted(&self, id: HookId) -> bool {
        self.live_hooks().is_some_and(|hooks| hooks.is_wanted(id))
    }

    fn save_profiles(&self) {
        if let Some(saver) = &self.options.save_profiles {
            saver.save(&self.state.profiles);
        }
    }

    /// The admitted offline character entered play: it is the one shown, its profile becomes the last played, and its
    /// wanted hooks wait their turn (`apply_queued`). Without a profile it wants nothing, with the template's values,
    /// and nothing is saved.
    pub(super) fn enter_play(&mut self) {
        let Some(key) = self.character.as_ref().map(ProfileKey::of) else {
            return;
        };
        if self.state.profiles.get(&key).is_some() {
            Arc::make_mut(&mut self.state.profiles).played(&key, self.clock.now());
            self.save_profiles();
            self.state.unsaved = None;
        } else {
            self.state.unsaved = Some(Arc::new(self.state.profiles.template().clone()));
        }
        self.live = Some(key.clone());
        self.state.editing = Some(key);
        self.state.hooks = hook_ids().map(|id| (id, if self.wanted(id) { HookRun::Queued } else { HookRun::Off })).collect();
    }
}
