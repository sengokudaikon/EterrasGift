//! Narrow conditional current-player rate write lifecycle with independent confirmation.
use super::*;
use crate::{
    field_writes::{AttackLeechPlan, RateWriteRecord, RateWriteStatus},
    writes::{AllowWrite, WriteControl},
};
use eg_game_access::rate_write::{AttackLeechRateWrite, RateWriteReply, classify_response};

impl<T: Transport, L: LogSource> Session<T, L> {
    pub fn last_rate_write(&self) -> Option<&RateWriteRecord> {
        self.last_rate_write.as_ref()
    }

    /// Explicitly submit one attack/leech field change. Errors returned through
    /// Err occur before transport is entered. After that point the owned record
    /// and returned status always preserve certainty, including lost admission.
    /// Confirmation requires native full readback and a separate fresh ReadAttackLeech
    /// exchange. Every failure of that later exchange is an uncertain write.
    /// The backend must separately check its exact process/generation/sequence-bound
    /// current snapshot. Session has no backend sequence ledger; this entrypoint
    /// validates target/profile/current manager and the complete expected wire data.
    /// Current-player semantics confer no historical selection or replay ticket.
    /// Dropping this future preserves Pending and blocks all work until cleanup.
    pub async fn set_attack_leech_rate(&mut self, now: Timestamp, plan: &AttackLeechPlan) -> Result<RateWriteStatus, SessionError> {
        self.set_attack_leech_rate_controlled(now, plan, &mut AllowWrite).await
    }
    pub async fn set_attack_leech_rate_controlled(&mut self, now: Timestamp, plan: &AttackLeechPlan, control: &mut dyn WriteControl) -> Result<RateWriteStatus, SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.write_gate(now)?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        let expected = plan.before();
        if expected.process != self.target
            || self
                .status
                .as_ref()
                .is_none_or(|status| status.online || status.manager == 0 || status.manager != expected.manager)
        {
            return Err(SessionError::Identity("rate write belongs to a different process or offline manager".into()));
        }
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let AttackLeechRead::Available(before) = expected.read else {
            return Err(SessionError::Identity("rate plan has no complete expected observation".into()));
        };
        let target = match plan.target() {
            crate::field_writes::AttackLeechTarget::AttackRate => eg_game_access::fields::AttackLeechTarget::AttackRate,
            crate::field_writes::AttackLeechTarget::IncreasedLeechRate => eg_game_access::fields::AttackLeechTarget::IncreasedLeechRate,
        };
        let request = AttackLeechRateWrite::new(dispatcher, before, target, plan.projected_bits()).map_err(SessionError::Protocol)?;
        let elapsed = std::time::Instant::now();
        if let Err(error) = self.profile.validate(&dispatcher) {
            self.release_without_io();
            self.state = SessionState::Blocked;
            return Err(SessionError::Identity(error.to_string()));
        }
        self.write_gate(advanced(now, elapsed)?)?;
        if !control.allowed() {
            return Err(SessionError::Busy);
        }
        let status = self.status.take();
        self.state = SessionState::Connecting;
        // Set this before polling transport: after cancellation no caller may
        // infer nonexecution from absence of a returned acknowledgement.
        self.last_rate_write = Some(RateWriteRecord {
            plan: plan.clone(),
            status: RateWriteStatus::Pending,
        });
        let mut result = match self.transport.send(&Request::SetAttackLeechRate(request.clone())).await {
            Ok(response) => match classify_response(&response, &request) {
                RateWriteReply::Confirmed(snapshot) => RateWriteStatus::Confirmed(snapshot),
                RateWriteReply::NotApplied { code, message } => RateWriteStatus::NotApplied { code, message },
                RateWriteReply::Uncertain { reason } => RateWriteStatus::uncertain(reason),
            },
            Err(error) if error.delivery == Delivery::NotSent => RateWriteStatus::NotApplied {
                code: ResponseCode::NotReady,
                message: error.to_string(),
            },
            Err(error) => RateWriteStatus::uncertain(error.to_string()),
        };
        // No more process/pipe access follows a loss of Player.log admission.
        // A trusted pre-store refusal stays definitive even if admission changed;
        // an apparent success is withheld if its provenance can no longer hold.
        let admitted = advanced(now, elapsed).and_then(|time| self.write_gate(time));
        let artifacts = if admitted.is_ok() {
            self.profile.validate(&dispatcher).map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        if let RateWriteStatus::Confirmed(_) = result {
            if let Err(error) = &admitted {
                result = RateWriteStatus::uncertain(format!("write reply arrived after admission loss: {error}"));
            } else if let Err(error) = &artifacts {
                result = RateWriteStatus::uncertain(format!("write artifacts changed before confirmation: {error}"));
            }
        }
        if matches!(result, RateWriteStatus::Confirmed(_)) {
            // File verification may take time. Recheck admission immediately
            // before the independent read, and honor cleanup between requests.
            // The store has already run: no subsequent failure is NotApplied.
            if let Err(error) = advanced(now, elapsed).and_then(|time| self.write_gate(time)) {
                result = RateWriteStatus::uncertain(format!("write sent but independent readback admission failed: {error}"));
            } else if !control.allowed() {
                result = RateWriteStatus::uncertain("write acknowledged before queued cleanup; separate readback was not submitted");
            } else {
                result = match self
                    .transport
                    .send(&Request::ReadAttackLeech {
                        dispatcher,
                        expected_manager: expected.manager,
                    })
                    .await
                {
                    Ok(response) if response.code == ResponseCode::Done => match fields::decode(&response.payload) {
                        Ok(AttackLeechRead::Available(actual)) if actual == *plan.projected() => RateWriteStatus::Confirmed(actual),
                        Ok(_) => RateWriteStatus::uncertain("separate readback differs from the complete projected rate observation"),
                        Err(error) => RateWriteStatus::uncertain(format!("invalid separate write readback: {error}")),
                    },
                    Ok(response) => RateWriteStatus::uncertain(format!("write sent but separate readback was refused: {:?}", response.code)),
                    Err(error) => RateWriteStatus::uncertain(format!("write sent but separate readback failed: {error}")),
                };
                if matches!(result, RateWriteStatus::Confirmed(_)) {
                    let provenance = advanced(now, elapsed)
                        .and_then(|time| self.write_gate(time))
                        .and_then(|()| self.profile.validate(&dispatcher).map_err(|error| SessionError::Identity(error.to_string())))
                        .and_then(|()| advanced(now, elapsed).and_then(|time| self.write_gate(time)));
                    if let Err(error) = provenance {
                        result = RateWriteStatus::uncertain(format!("independent readback lost confirmation provenance: {error}"));
                    }
                }
            }
        }
        let reusable = admitted.is_ok() && artifacts.is_ok() && matches!(result, RateWriteStatus::Confirmed(_));
        if reusable {
            self.status = status;
            self.state = SessionState::Ready;
        } else {
            self.release_without_io();
            if self.state == SessionState::Connecting {
                self.state = if matches!(
                    result,
                    RateWriteStatus::NotApplied {
                        code: ResponseCode::OfflineRequired,
                        ..
                    }
                ) {
                    SessionState::WaitingForCharacter
                } else {
                    SessionState::Blocked
                };
            }
        }
        self.last_rate_write.as_mut().expect("write record retained during exclusive submission").status = result.clone();
        Ok(result)
    }
}
