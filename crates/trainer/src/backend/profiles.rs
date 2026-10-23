//! Cheat profiles in the worker (`crate::profiles`): which one the pages show and change, the lock that holds them
//! still while a game runs without an offline character in play, and the hooks of the character in play applied one at
//! a time, each settled by the game's own report. A character without a profile has hooks for the connection only,
//! until the user keeps them as its profile.
use super::write::{CurrentIntent, invalid_edit, refusal, write_error};
use super::*;
use crate::features::{DESCRIPTORS, FeatureError, FeatureIntents, FeatureKey, HookId};
use crate::profiles::ProfileKey;

/// What a wanted hook says while this version cannot apply it.
pub const HOOK_NOT_AVAILABLE: &str = "Not available in this version yet.";

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

/// A hook of the character in play, in this connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookRun {
    /// Off in the game: not wanted, or turned off as asked.
    Off,
    /// Wanted, waiting its turn.
    Queued,
    Applying,
    /// On in the game, as its own report confirmed.
    On,
    /// The last change did not take, for `reason`. `on` is what the game holds: its report, or what it held before
    /// when nothing reached it.
    Failed {
        reason: String,
        on: bool,
    },
}

impl HookRun {
    /// Whether the game may hold the hook on.
    fn holds(&self) -> bool {
        matches!(self, Self::On | Self::Failed { on: true, .. })
    }
}

/// One hook as the game should have it.
#[derive(Clone, Debug, PartialEq)]
pub struct HookRequest {
    pub id: HookId,
    pub on: bool,
    pub values: Vec<f64>,
}

fn hook_ids() -> impl Iterator<Item = HookId> {
    DESCRIPTORS.iter().filter_map(|descriptor| match descriptor.key {
        FeatureKey::Hook(id) => Some(id),
        _ => None,
    })
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
        let applied = live && (self.wanted(id) || self.state.hooks.get(&id).is_some_and(HookRun::holds));
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
    fn live_hooks(&self) -> Option<&FeatureIntents> {
        let key = self.live.as_ref()?;
        self.state.profiles.get(key).map(|profile| &profile.hooks).or(self.state.unsaved.as_deref())
    }

    fn wanted(&self, id: HookId) -> bool {
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

    /// Applies the hooks still waiting their turn, in the pages' order. Stops at the first that leaves the session
    /// unusable or its outcome unknown; one the game refuses says why, and the rest go on.
    pub(super) async fn apply_queued(&mut self, epoch: u64) -> Result<(), BackendError> {
        for id in hook_ids() {
            if self.state.hooks.get(&id) == Some(&HookRun::Queued) {
                self.superseded(epoch)?;
                self.apply_hook(epoch, id).await?;
            }
        }
        Ok(())
    }

    async fn apply_hook(&mut self, epoch: u64, id: HookId) -> Result<(), BackendError> {
        let (Some(hooks), Some(target)) = (self.live_hooks(), self.state.attached.clone()) else {
            return Err(BackendError::blocked(ErrorKind::NotConnected, "no offline character in play"));
        };
        let request = HookRequest {
            id,
            on: hooks.is_wanted(id),
            values: hooks.values(id).to_vec(),
        };
        let holds = self.state.hooks.get(&id).is_some_and(HookRun::holds);
        // Nothing to take back from the game, or nothing this version can do there.
        let settled = if !request.on && !holds {
            Some(HookRun::Off)
        } else if !self.connection.as_ref().is_some_and(|connection| connection.supports_hook(id)) {
            Some(HookRun::Failed {
                reason: HOOK_NOT_AVAILABLE.into(),
                on: holds,
            })
        } else {
            None
        };
        if let Some(run) = settled {
            return self.commit(epoch, |this| {
                this.state.hooks.insert(id, run);
            });
        }
        self.admit_using_policy(&target, GatePolicy::Release)?;
        // Superseded before it is sent, a change waits its turn again: a queued cleanup usually ends the connection,
        // and one that does not (automatic connection turned off) lets the monitor go on with it.
        if let Err(error) = self.commit(epoch, |this| {
            this.state.hooks.insert(id, HookRun::Applying);
        }) {
            self.state.hooks.insert(id, HookRun::Queued);
            return Err(error);
        }
        let mut control = CurrentIntent {
            intent: self.intent.clone(),
            epoch,
        };
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?;
        let (outcome, mut failure) = match connection.set_hook(self.clock.now(), &request, &mut control).await {
            Ok(outcome) => (outcome, None),
            Err(error) if error.kind == ErrorKind::Superseded && error.delivery != Some(Delivery::MayHaveRun) => {
                self.state.hooks.insert(id, HookRun::Queued);
                return Err(error);
            }
            Err(error) => (
                if error.delivery == Some(Delivery::MayHaveRun) {
                    WriteOutcome::uncertain(error.message.clone())
                } else {
                    WriteOutcome::refused(error.message.clone())
                },
                Some(error),
            ),
        };
        let run = match outcome {
            WriteOutcome::Confirmed(on) if on == request.on => {
                if on {
                    HookRun::On
                } else {
                    HookRun::Off
                }
            }
            WriteOutcome::Confirmed(on) | WriteOutcome::NotKept(on) => HookRun::Failed {
                reason: format!("The game's report has it {} after the change.", if on { "on" } else { "off" }),
                on,
            },
            WriteOutcome::NotApplied { code, message } => {
                failure = failure.or_else(|| refusal(code, &message));
                HookRun::Failed { reason: message, on: holds }
            }
            WriteOutcome::Uncertain { message } => return Err(self.hook_uncertain(id, &message, target)),
            WriteOutcome::Pending => return Err(self.hook_uncertain(id, "the helper gave no outcome", target)),
        };
        // Refused while the character loads: nothing reached the game, and the hook waits for it, as a read does.
        let run = match &failure {
            Some(error) if error.kind == ErrorKind::CharacterLoading => HookRun::Queued,
            _ => run,
        };
        // What the game answered stands even when cleanup is queued; it is published with the next snapshot then.
        self.state.hooks.insert(id, run);
        self.publish_if_current(epoch)?;
        match failure {
            None => Ok(()),
            Some(error) if matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading) => {
                self.handle_connection_gate(&error);
                Err(error)
            }
            Some(error)
                if matches!(
                    error.kind,
                    ErrorKind::Transport | ErrorKind::Protocol | ErrorKind::Identity | ErrorKind::RestartRequired | ErrorKind::Fault
                ) =>
            {
                self.fail(error.clone(), Some(target));
                Err(error)
            }
            // Refused before anything reached the game: the hook says why, and the session goes on.
            Some(_) => Ok(()),
        }
    }

    /// A change that may have reached the game ends the session, as an uncertain write does: nothing more is applied,
    /// and the helper leaves, until Retry.
    fn hook_uncertain(&mut self, id: HookId, message: &str, target: ProcessIdentity) -> BackendError {
        let error = write_error(
            ErrorKind::WriteUncertain,
            format!("{}: {message}", FeatureKey::Hook(id).descriptor().title),
            Delivery::MayHaveRun,
        );
        self.fail(error.clone(), Some(target));
        error
    }
}
