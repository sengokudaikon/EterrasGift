//! Narrow CURRENT-player writes. Expected tokens/sequence are guards, not lifetime tickets.
use super::*;
#[cfg(test)]
mod tests;

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn set_attack_leech_rate(&mut self, plan: AttackLeechPlan) -> Result<(), BackendError> {
        let result = self.set_attack_leech_rate_inner(plan).await;
        self.check_write_failure(result)
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
        let current = self
            .state
            .attack_leech
            .as_ref()
            .ok_or_else(|| super::write::invalid_edit("current rate observation missing"))?;
        if self.state.attached.as_ref() != Some(&expected.process)
            || expected.generation != self.state.generation
            || self
                .state
                .runtime_status
                .as_ref()
                .is_none_or(|status| status.online || status.manager == 0 || status.manager != expected.manager)
        {
            return Err(super::write::invalid_edit("rate plan belongs to another process, generation or manager"));
        }
        plan.recheck(current).map_err(super::write::invalid_edit)?;
        // Reserve before any RPC: a confirmed store must never discover counter exhaustion.
        let sequence = self
            .attack_leech_sequence
            .checked_add(1)
            .ok_or_else(|| BackendError::blocked(ErrorKind::Fault, "attack/leech observation sequence exhausted"))?;
        let target = expected.process.clone();
        self.admit_using_policy(&target, GatePolicy::Release)?;
        let refreshed = self
            .connection
            .as_mut()
            .ok_or_else(|| super::write::invalid_edit("connection released"))?
            .refresh(self.clock.now())
            .await;
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
        self.admit_using_policy(&target, GatePolicy::Release)?;
        self.commit(epoch, |this| {
            this.state.activity = Some(Activity::WritingAttackLeech);
            this.state.error = None;
            this.state.last_rate_write = Some(RateWriteRecord {
                plan: plan.clone(),
                status: RateWriteStatus::Pending,
            });
        })?;
        let mut control = super::write::CurrentIntent {
            intent: self.intent.clone(),
            epoch,
        };
        let write_plan = WritePlan::from(plan.clone());
        let (status, failure) = self.submit_write(&write_plan, &mut control).await;
        let status = status.map(WriteObservation::into_field);
        self.log_rate_outcome(&plan, &status);
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
        self.state.last_rate_write = Some(RateWriteRecord {
            plan: plan.clone(),
            status: status.clone(),
        });
        let result = match status {
            RateWriteStatus::Confirmed(actual) | RateWriteStatus::NotKept(actual) => {
                // The retained result predates this publication check.
                if self.state.generation != expected.generation
                    || self.state.attached.as_ref() != Some(&target)
                    || self.state.runtime_status.as_ref().is_none_or(|s| s.online || s.manager != expected.manager)
                {
                    return self.finish_write(
                        Err(BackendError::blocked(ErrorKind::Identity, "field write lost current publication scope")),
                        failure,
                        target,
                    );
                }
                if let Err(error) = self.admit_using_policy(&target, GatePolicy::Release) {
                    return self.finish_write(Err(error), failure, target);
                }
                let kept = actual == *plan.projected();
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
                if kept {
                    Ok(())
                } else {
                    Err(super::write::write_error(
                        ErrorKind::WriteNotKept,
                        "independent readback differs from the requested fields",
                        Delivery::MayHaveRun,
                    ))
                }
            }
            RateWriteStatus::NotApplied { message, .. } => Err(super::write::write_error(ErrorKind::WriteNotApplied, message, Delivery::NotSent)),
            RateWriteStatus::Uncertain { message } => Err(super::write::write_error(ErrorKind::WriteUncertain, message, Delivery::MayHaveRun)),
            RateWriteStatus::Pending => unreachable!(),
        };
        self.finish_write(result, failure, target)
    }
}
