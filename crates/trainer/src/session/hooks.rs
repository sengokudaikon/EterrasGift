use super::*;
use crate::write::{WriteControl, WriteOutcome};
use eg_trainer_wire::{
    hooks::{GameplayHook, HookObservation},
    transport::Delivery,
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

    pub async fn set_gameplay_hook_with_value(&mut self, hook: GameplayHook, on: bool, value: u64, control: &mut dyn WriteControl) -> Result<WriteOutcome<bool>, SessionError> {
        hook.validate_value(value).map_err(SessionError::Protocol)?;
        self.admit()?;
        let dispatcher = self.dispatcher()?;
        let expected_manager = self.offline_manager()?;
        self.provenance(&dispatcher)?;
        if !control.allowed() {
            return Err(SessionError::Busy);
        }
        self.state = SessionState::Connecting;
        let request = Request::SetGameplayHook {
            dispatcher,
            expected_manager,
            hook,
            on,
            value,
        };
        let mut outcome = match self.transport.send(&request).await {
            Ok(response) => match response.code {
                ResponseCode::Done if response.payload.is_empty() => WriteOutcome::Confirmed(on),
                ResponseCode::NotReady | ResponseCode::Refused | ResponseCode::CharacterLoading | ResponseCode::OfflineRequired | ResponseCode::RestartRequired => {
                    WriteOutcome::NotApplied {
                        code: response.code,
                        message: String::from_utf8_lossy(&response.payload).into_owned(),
                    }
                }
                _ => WriteOutcome::uncertain("Hook change returned no valid acknowledgement"),
            },
            Err(error) if error.delivery == Delivery::NotSent => WriteOutcome::NotApplied {
                code: ResponseCode::NotReady,
                message: error.to_string(),
            },
            Err(error) => WriteOutcome::uncertain(error.to_string()),
        };
        let provenance = self.provenance(&dispatcher);
        if matches!(outcome, WriteOutcome::Confirmed(_)) {
            outcome = if let Err(error) = provenance.as_ref() {
                WriteOutcome::uncertain(format!("Hook reply lost confirmation provenance: {error}"))
            } else if !control.allowed() {
                WriteOutcome::uncertain("Hook acknowledged before queued cleanup; independent readback was not submitted")
            } else {
                match self
                    .transport
                    .send(&Request::ReadGameplayHook {
                        dispatcher,
                        expected_manager,
                        hook,
                    })
                    .await
                {
                    Ok(response) if response.code == ResponseCode::Done => match HookObservation::decode(&response.payload) {
                        Ok(actual) if actual.confirmed(actual.enabled) && hook.validate_value(actual.value).is_ok() => {
                            self.hooks[hook as usize] = Some(actual);
                            if actual.enabled == on && (!on || actual.value == value) {
                                WriteOutcome::Confirmed(on)
                            } else {
                                WriteOutcome::NotKept(actual.enabled)
                            }
                        }
                        _ => WriteOutcome::uncertain("Hook flag and entry bytes did not agree"),
                    },
                    Ok(response) => WriteOutcome::uncertain(format!("Hook readback refused: {:?}", response.code)),
                    Err(error) => WriteOutcome::uncertain(format!("Hook readback failed: {error}")),
                }
            };
            if matches!(outcome, WriteOutcome::Confirmed(_) | WriteOutcome::NotKept(_))
                && let Err(error) = self.provenance(&dispatcher)
            {
                outcome = WriteOutcome::uncertain(format!("Hook readback lost confirmation provenance: {error}"));
            }
        }
        let reusable = provenance.is_ok()
            && matches!(
                outcome,
                WriteOutcome::Confirmed(_)
                    | WriteOutcome::NotKept(_)
                    | WriteOutcome::NotApplied {
                        code: ResponseCode::NotReady | ResponseCode::Refused | ResponseCode::CharacterLoading,
                        ..
                    }
            );
        if reusable {
            self.state = SessionState::Ready;
            if matches!(
                outcome,
                WriteOutcome::NotApplied {
                    code: ResponseCode::CharacterLoading,
                    ..
                }
            ) {
                self.status = None;
            }
        } else {
            self.revoke();
        }
        Ok(outcome)
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
