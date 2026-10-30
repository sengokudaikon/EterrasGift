//! The hooks of the character in play, applied one at a time, each settled by the game's own report.
use super::write::{CurrentIntent, refusal, write_error};
use super::*;
use crate::features::{DESCRIPTORS, FeatureKey, HookId};

/// What a wanted hook says while this version cannot apply it.
pub const HOOK_NOT_AVAILABLE: &str = "Not available in this version yet.";

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
    pub(super) fn holds(&self) -> bool {
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

pub(super) fn hook_ids() -> impl Iterator<Item = HookId> {
    DESCRIPTORS.iter().filter_map(|descriptor| match descriptor.key {
        FeatureKey::Hook(id) => Some(id),
        _ => None,
    })
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) fn reconcile_hooks(&mut self) {
        for id in hook_ids() {
            if let Some(false) = self.connection.as_ref().and_then(|connection| id.native().and_then(|hook| connection.observed_hook(hook)))
                && self.state.hooks.get(&id).is_some_and(HookRun::holds)
            {
                self.state.hooks.insert(id, if self.wanted(id) { HookRun::Queued } else { HookRun::Off });
            }
        }
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

    pub(super) async fn apply_hook(&mut self, epoch: u64, id: HookId) -> Result<(), BackendError> {
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
        } else if !self
            .connection
            .as_ref()
            .is_some_and(|connection| id.native().is_some_and(|hook| connection.capabilities().gameplay_hooks[hook as usize]))
        {
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
        self.admit(&target)?;
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
        let connection = self.connection.as_mut().ok_or_else(super::released)?;
        let (outcome, mut failure) = match connection.set_hook(&request, &mut control).await {
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
