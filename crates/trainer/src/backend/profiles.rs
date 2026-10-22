//! Cheat profiles in the worker (`crate::profiles`): which one the pages show and change, the lock that holds them
//! still while a game runs without an offline character in play, and the live profile's hooks applied one at a time,
//! each settled by the game's own report.
use super::write::{CurrentIntent, invalid_edit, refusal, write_error};
use super::*;
use crate::features::{DESCRIPTORS, FeatureError, FeatureIntents, FeatureKey, HookId};
use crate::profiles::ProfileKey;
use std::collections::BTreeMap;

/// What a wanted hook says while this version cannot apply it.
pub const HOOK_NOT_AVAILABLE: &str = "Not available in this version yet.";

/// Whether the profiles can change, and which one is live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileLock {
    /// No game runs: any profile can be shown, changed or deleted.
    Editable,
    /// A game runs without an offline character in play (the connection's phase says why): the profiles wait.
    Locked,
    /// The character is in play: its profile is the one shown, and each change is applied as it is made.
    Live(ProfileKey),
}

/// A hook of the live profile, in this connection.
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

    pub(super) fn delete_profile(&mut self, key: ProfileKey) -> Result<(), BackendError> {
        if self.profile_lock() != ProfileLock::Editable {
            return Err(locked());
        }
        if !Arc::make_mut(&mut self.state.profiles).remove(&key) {
            return Err(invalid_edit("no such profile"));
        }
        if self.state.editing.as_ref() == Some(&key) {
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

    /// Changes the shown profile, or the live one, and saves it. Whether it changed, and whether it is live.
    fn change_profile(&mut self, change: impl FnOnce(&mut FeatureIntents) -> Result<bool, FeatureError>) -> Result<(bool, bool), BackendError> {
        let (key, live) = match self.profile_lock() {
            ProfileLock::Editable => (
                self.state.editing.clone().ok_or_else(|| {
                    BackendError::blocked(
                        ErrorKind::NotReady,
                        "No character has a profile yet: one is made when an offline character enters play with the trainer connected.",
                    )
                })?,
                false,
            ),
            ProfileLock::Live(key) => (key, true),
            ProfileLock::Locked => return Err(locked()),
        };
        let hooks = Arc::make_mut(&mut self.state.profiles).hooks_mut(&key).ok_or_else(|| invalid_edit("no such profile"))?;
        let changed = change(hooks).map_err(invalid_edit)?;
        if changed {
            self.save_profiles();
            self.publish();
        }
        Ok((changed, live))
    }

    fn wanted(&self, id: HookId) -> bool {
        self.live
            .as_ref()
            .and_then(|key| self.state.profiles.get(key))
            .is_some_and(|profile| profile.hooks.is_wanted(id))
    }

    fn save_profiles(&self) {
        if let Some(saver) = &self.options.save_profiles {
            saver.save(&self.state.profiles);
        }
    }

    /// The admitted offline character entered play: its profile (a new one wants nothing) is the live intent and the
    /// last played, and its wanted hooks are applied in turn.
    pub(super) async fn enter_play(&mut self, epoch: u64) -> Result<(), BackendError> {
        let Some(key) = self.character.as_ref().map(ProfileKey::of) else {
            return Ok(());
        };
        Arc::make_mut(&mut self.state.profiles).played(&key, self.clock.now());
        self.save_profiles();
        self.live = Some(key.clone());
        let hooks: BTreeMap<_, _> = hook_ids().map(|id| (id, if self.wanted(id) { HookRun::Queued } else { HookRun::Off })).collect();
        self.commit(epoch, |this| {
            this.state.editing = Some(key);
            this.state.hooks = hooks;
        })?;
        self.apply_queued(epoch).await
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
        let (Some(key), Some(target)) = (self.live.clone(), self.state.attached.clone()) else {
            return Err(BackendError::blocked(ErrorKind::NotConnected, "no offline character in play"));
        };
        let hooks = &self.state.profiles.get(&key).ok_or_else(|| invalid_edit("the live profile is missing"))?.hooks;
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
