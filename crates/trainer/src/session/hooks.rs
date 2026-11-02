use super::*;
use crate::write::{WriteControl, WriteOutcome};
use eg_trainer_wire::{
    hooks::{GameplayHook, HookObservation},
    protocol::Response,
};

impl<T: Transport> Session<T> {
    pub fn hook_observation(&self, hook: GameplayHook) -> Option<HookObservation> {
        self.hooks[hook as usize]
    }

    /// A submitted mutation is never retried. ACK and independent entry/flag
    /// readback both belong to the same admitted manager and artifact lease.
    pub async fn set_gameplay_hook(&mut self, hook: GameplayHook, on: bool, control: &mut dyn WriteControl) -> Result<WriteOutcome<bool>, SessionError> {
        self.set_gameplay_hook_with_value(hook, on, hook.default_value(), control).await
    }

    pub async fn set_gameplay_hook_with_value(
        &mut self,
        hook: GameplayHook,
        on: bool,
        value: eg_trainer_wire::hooks::HookValue,
        control: &mut dyn WriteControl,
    ) -> Result<WriteOutcome<bool>, SessionError> {
        hook.validate_value(value).map_err(SessionError::Protocol)?;
        self.admit()?;
        let dispatcher = self.dispatcher()?;
        let expected_manager = self.offline_manager()?;
        self.provenance(&dispatcher)?;
        if !control.allowed() {
            return Err(SessionError::Busy);
        }
        let request = Request::SetGameplayHook {
            dispatcher,
            expected_manager,
            hook,
            on,
            value,
        };
        let readback = Request::ReadGameplayHook {
            dispatcher,
            expected_manager,
            hook,
        };
        let acknowledged = |response: &Response| match response.code {
            ResponseCode::Done if response.payload.is_empty() => WriteOutcome::Confirmed(on),
            ResponseCode::NotReady | ResponseCode::Refused | ResponseCode::CharacterLoading | ResponseCode::OfflineRequired | ResponseCode::RestartRequired => {
                WriteOutcome::NotApplied {
                    code: response.code,
                    message: String::from_utf8_lossy(&response.payload).into_owned(),
                }
            }
            _ => WriteOutcome::uncertain("Hook change returned no valid acknowledgement"),
        };
        let observed = |session: &mut Self, payload: &[u8]| match HookObservation::decode(payload) {
            Ok(actual) if actual.confirmed(actual.enabled) && hook.validate_value(actual.value).is_ok() => {
                session.hooks[hook as usize] = Some(actual);
                if actual.enabled == on && (!on || actual.value == value) {
                    WriteOutcome::Confirmed(on)
                } else {
                    WriteOutcome::NotKept(actual.enabled)
                }
            }
            _ => WriteOutcome::uncertain("Hook flag and entry bytes did not agree"),
        };
        Ok(self.mutate(dispatcher, &request, &readback, control, acknowledged, observed).await)
    }

    /// Poll only hooks this connection has confirmed; unsupported features need no RPC.
    pub(crate) async fn refresh_gameplay_hooks(&mut self) -> Result<(), SessionError> {
        let dispatcher = self.dispatcher()?;
        let expected_manager = self.status.ok_or(SessionError::NotConnected)?.manager;
        self.state = SessionState::Connecting;
        let result = async {
            for hook in GameplayHook::ALL {
                if self.hooks[hook as usize].is_none() {
                    continue;
                }
                self.provenance(&dispatcher)?;
                let payload = self
                    .request(&Request::ReadGameplayHook {
                        dispatcher,
                        expected_manager,
                        hook,
                    })
                    .await?;
                let actual = HookObservation::decode(&payload).map_err(SessionError::Protocol)?;
                if !actual.confirmed(actual.enabled) || hook.validate_value(actual.value).is_err() {
                    return Err(SessionError::Identity("Hook flag and entry bytes no longer agree".into()));
                }
                if actual.enabled && self.hooks[hook as usize].is_some_and(|previous| previous.enabled && previous.value != actual.value) {
                    return Err(SessionError::Identity("Hook selection changed since confirmation".into()));
                }
                self.provenance(&dispatcher)?;
                self.hooks[hook as usize] = Some(actual);
            }
            Ok(())
        }
        .await;
        if result.is_ok() {
            self.state = SessionState::Ready;
        } else if !matches!(result, Err(SessionError::CharacterLoading)) {
            self.revoke();
        }
        result
    }
}
