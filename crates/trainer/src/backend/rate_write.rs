//! Narrow CURRENT-player writes. Expected tokens/sequence are guards, not lifetime tickets.
use super::*;
#[cfg(test)]
mod tests;

struct CurrentRateIntent {
    intent: Arc<Mutex<u64>>,
    epoch: u64,
}
impl WriteControl for CurrentRateIntent {
    fn allowed(&mut self) -> bool {
        *self.intent.lock().unwrap_or_else(|p| p.into_inner()) == self.epoch
    }
}
fn invalid_rate(error: impl fmt::Display) -> BackendError {
    BackendError::blocked(ErrorKind::InvalidEdit, error.to_string())
}
fn rate_error(kind: ErrorKind, message: impl Into<String>, delivery: Delivery) -> BackendError {
    BackendError {
        kind,
        message: message.into(),
        delivery: Some(delivery),
        retry_when_waiting: false,
    }
}
impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn set_attack_leech_rate(&mut self, plan: AttackLeechPlan) -> Result<(), BackendError> {
        let result = self.set_attack_leech_rate_inner(plan).await;
        if let Err(error) = &result
            && self.state.phase == Phase::Ready
            && matches!(
                error.kind,
                ErrorKind::Transport | ErrorKind::Protocol | ErrorKind::Identity | ErrorKind::RestartRequired | ErrorKind::Fault
            )
        {
            self.fail(error.clone(), self.state.attached.clone());
        }
        result
    }
    async fn set_attack_leech_rate_inner(&mut self, plan: AttackLeechPlan) -> Result<(), BackendError> {
        if !self.state.enabled {
            return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
        }
        if self.state.phase != Phase::Ready {
            return Err(BackendError::blocked(ErrorKind::NotConnected, "session is not ready"));
        }
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        if !self.connection.as_ref().is_some_and(BackendConnection::supports_attack_leech_rate) {
            return Err(BackendError::blocked(ErrorKind::NotReady, "attack/leech rate writes are unavailable"));
        }
        let expected = plan.before();
        let current = self.state.attack_leech.as_ref().ok_or_else(|| invalid_rate("current rate observation missing"))?;
        if self.state.attached.as_ref() != Some(&expected.process)
            || expected.generation != self.state.generation
            || self
                .state
                .runtime_status
                .as_ref()
                .is_none_or(|status| status.online || status.manager == 0 || status.manager != expected.manager)
        {
            return Err(invalid_rate("rate plan belongs to another process, generation or manager"));
        }
        plan.recheck(current).map_err(invalid_rate)?;
        // Reserve before any RPC: a confirmed store must never discover counter exhaustion.
        let sequence = self
            .attack_leech_sequence
            .checked_add(1)
            .ok_or_else(|| BackendError::blocked(ErrorKind::Fault, "attack/leech observation sequence exhausted"))?;
        let target = expected.process.clone();
        self.admit_using_policy(&target, GatePolicy::Release)?;
        let refreshed = self.connection.as_mut().ok_or_else(|| invalid_rate("connection released"))?.refresh(self.clock.now()).await;
        let status = match refreshed {
            Ok(status) => status,
            Err(error) => {
                self.handle_connection_gate(&error);
                if error.kind != ErrorKind::Gate {
                    self.fail(error.clone(), Some(target));
                }
                return Err(error);
            }
        };
        self.superseded(epoch)?;
        self.check_status(status)?;
        if status.manager != expected.manager {
            return Err(BackendError::blocked(ErrorKind::Identity, "character manager changed before rate write"));
        }
        plan.recheck(self.state.attack_leech.as_ref().ok_or_else(|| invalid_rate("current rate observation released"))?)
            .map_err(invalid_rate)?;
        self.admit_using_policy(&target, GatePolicy::Release)?;
        self.commit(epoch, |this| {
            this.state.activity = Some(Activity::WritingAttackLeech);
            this.state.error = None;
            this.state.last_rate_write = Some(RateWriteRecord {
                plan: plan.clone(),
                status: RateWriteStatus::Pending,
            });
        })?;
        let mut control = CurrentRateIntent {
            intent: self.intent.clone(),
            epoch,
        };
        let result = self.connection.as_mut().unwrap().set_attack_leech_rate(self.clock.now(), &plan, &mut control).await;
        let mut status = match result {
            Ok(RateWriteStatus::Confirmed(actual)) if actual == *plan.projected() => RateWriteStatus::Confirmed(actual),
            Ok(RateWriteStatus::Confirmed(_)) => RateWriteStatus::uncertain("connection returned mismatched complete rate confirmation"),
            Ok(RateWriteStatus::Pending) => RateWriteStatus::uncertain("connection returned before rate write completion"),
            Ok(RateWriteStatus::NotApplied {
                code: ResponseCode::Done | ResponseCode::Failed,
                ..
            }) => RateWriteStatus::uncertain("connection mislabeled a success/failure code as pre-store refusal"),
            Ok(status) => status,
            Err(error) if error.delivery == Some(Delivery::MayHaveRun) => RateWriteStatus::uncertain(error.message),
            Err(error) => RateWriteStatus::NotApplied {
                code: ResponseCode::Refused,
                message: error.message,
            },
        };
        self.state.activity = None;
        // Retain historical certainty even if cleanup was queued; no older-generation
        // observation may be published. Session already performed independent readback.
        if self.superseded(epoch).is_err() {
            self.state.last_rate_write = Some(RateWriteRecord { plan, status });
            return Err(BackendError::blocked(
                ErrorKind::Superseded,
                "rate write drained before queued cleanup; inspect retained outcome",
            ));
        }
        if matches!(status, RateWriteStatus::Confirmed(_)) {
            let current_scope = self.state.generation == expected.generation
                && self.state.attached.as_ref() == Some(&target)
                && self.state.runtime_status.as_ref().is_some_and(|s| !s.online && s.manager == expected.manager);
            if !current_scope {
                status = RateWriteStatus::uncertain("rate confirmation lost current process/generation/manager scope");
            } else if let Err(error) = self.admit_using_policy(&target, GatePolicy::Release) {
                status = RateWriteStatus::uncertain(format!("admission lost before rate publication: {error}"));
            }
        }
        self.state.last_rate_write = Some(RateWriteRecord {
            plan: plan.clone(),
            status: status.clone(),
        });
        let result = match status {
            RateWriteStatus::Confirmed(actual) => {
                self.commit(epoch, |this| {
                    this.attack_leech_sequence = sequence;
                    this.state.attack_leech = Some(AttackLeechSnapshot {
                        process: target.clone(),
                        generation: expected.generation,
                        sequence,
                        manager: expected.manager,
                        read: AttackLeechRead::Available(actual),
                    });
                    this.state.error = None;
                })?;
                self.event(BackendEvent::ReadPublished {
                    generation: self.state.generation,
                });
                Ok(())
            }
            RateWriteStatus::NotApplied { message, .. } => Err(rate_error(ErrorKind::WriteNotApplied, message, Delivery::NotSent)),
            RateWriteStatus::Uncertain { message } => Err(rate_error(ErrorKind::WriteUncertain, message, Delivery::MayHaveRun)),
            RateWriteStatus::Pending => unreachable!(),
        };
        if let Err(error) = &result {
            self.fail(error.clone(), Some(target));
        }
        result
    }
}
