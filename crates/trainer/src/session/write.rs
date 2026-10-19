use super::*;
use crate::writes::{AllowWrite, LevelRequirementPlan, WriteControl, WriteRecord, WriteStatus};
use eg_game_access::write::{LevelRequirementWrite, WriteReply, classify_response};

impl<T: Transport, L: LogSource> Session<T, L> {
    pub fn last_write(&self) -> Option<&WriteRecord> {
        self.last_write.as_ref()
    }

    /// Explicitly submit one level requirement change. Errors returned through
    /// Err occur before transport is entered. After that point the owned record
    /// and returned status always preserve certainty, including lost admission.
    /// Confirmation requires native full readback and a separate fresh ReadForge
    /// exchange. Every failure of that later exchange is an uncertain write.
    /// The application must additionally recheck its generation and observation.
    /// Dropping this future preserves Pending and blocks all work until cleanup.
    pub async fn set_level_requirement(&mut self, now: Timestamp, plan: &LevelRequirementPlan) -> Result<WriteStatus, SessionError> {
        self.set_level_requirement_controlled(now, plan, &mut AllowWrite).await
    }
    pub async fn set_level_requirement_controlled(&mut self, now: Timestamp, plan: &LevelRequirementPlan, control: &mut dyn WriteControl) -> Result<WriteStatus, SessionError> {
        if matches!(self.state, SessionState::Connecting | SessionState::Closing) {
            return Err(SessionError::Busy);
        }
        self.write_gate(now)?;
        if self.state != SessionState::Ready {
            return Err(SessionError::NotConnected);
        }
        let expected = plan.before();
        if expected.process() != &self.target
            || !expected.catalog_identity().binary_hash.eq_ignore_ascii_case(self.profile.binary_hash())
            || !expected.catalog_identity().metadata_hash.eq_ignore_ascii_case(self.profile.metadata_hash())
            || self.status.as_ref().is_none_or(|status| status.online || status.manager != plan.manager())
        {
            return Err(SessionError::Identity("write belongs to a different runtime, manager or catalog".into()));
        }
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let request = LevelRequirementWrite::new(dispatcher, plan.manager(), expected.snapshot().clone(), plan.value()).map_err(SessionError::Protocol)?;
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
        self.last_write = Some(WriteRecord {
            plan: plan.clone(),
            status: WriteStatus::Pending,
        });
        let mut result = match self.transport.send(&Request::SetLevelRequirement(request.clone())).await {
            Ok(response) => match classify_response(&response, &request) {
                WriteReply::Confirmed(snapshot) => WriteStatus::Confirmed(snapshot),
                WriteReply::NotApplied { code, message } => WriteStatus::NotApplied { code, message },
                WriteReply::Uncertain { reason } => WriteStatus::uncertain(reason),
            },
            Err(error) if error.delivery == Delivery::NotSent => WriteStatus::NotApplied {
                code: ResponseCode::NotReady,
                message: error.to_string(),
            },
            Err(error) => WriteStatus::uncertain(error.to_string()),
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
        if let WriteStatus::Confirmed(_) = result {
            if let Err(error) = &admitted {
                result = WriteStatus::uncertain(format!("write reply arrived after admission loss: {error}"));
            } else if let Err(error) = &artifacts {
                result = WriteStatus::uncertain(format!("write artifacts changed before confirmation: {error}"));
            }
        }
        if matches!(result, WriteStatus::Confirmed(_)) {
            // File verification may take time. Recheck admission immediately
            // before the independent read, and honor cleanup between requests.
            // The store has already run: no subsequent failure is NotApplied.
            if let Err(error) = advanced(now, elapsed).and_then(|time| self.write_gate(time)) {
                result = WriteStatus::uncertain(format!("write sent but independent readback admission failed: {error}"));
            } else if !control.allowed() {
                result = WriteStatus::uncertain("write acknowledged before queued cleanup; separate readback was not submitted");
            } else {
                result = match self.transport.send(&Request::ReadForge { dispatcher }).await {
                    Ok(response) if response.code == ResponseCode::Done => match forge::decode(&response.payload) {
                        Ok(Some(actual)) if actual == *plan.projected() => WriteStatus::Confirmed(actual),
                        Ok(_) => WriteStatus::uncertain("separate readback differs from the full projected item"),
                        Err(error) => WriteStatus::uncertain(format!("invalid separate write readback: {error}")),
                    },
                    Ok(response) => WriteStatus::uncertain(format!("write sent but separate readback was refused: {:?}", response.code)),
                    Err(error) => WriteStatus::uncertain(format!("write sent but separate readback failed: {error}")),
                };
                if matches!(result, WriteStatus::Confirmed(_)) {
                    let provenance = advanced(now, elapsed)
                        .and_then(|time| self.write_gate(time))
                        .and_then(|()| self.profile.validate(&dispatcher).map_err(|error| SessionError::Identity(error.to_string())))
                        .and_then(|()| advanced(now, elapsed).and_then(|time| self.write_gate(time)));
                    if let Err(error) = provenance {
                        result = WriteStatus::uncertain(format!("independent readback lost confirmation provenance: {error}"));
                    }
                }
            }
        }
        let reusable = admitted.is_ok() && artifacts.is_ok() && matches!(result, WriteStatus::Confirmed(_));
        if reusable {
            self.status = status;
            self.state = SessionState::Ready;
        } else {
            self.release_without_io();
            if self.state == SessionState::Connecting {
                self.state = if matches!(
                    result,
                    WriteStatus::NotApplied {
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
        self.last_write.as_mut().expect("write record retained during exclusive submission").status = result.clone();
        Ok(result)
    }

    pub(super) fn write_gate(&mut self, now: Timestamp) -> Result<(), SessionError> {
        if !self.enabled {
            return Err(SessionError::Disabled);
        }
        // Development probe permission can discover a runtime, never write one.
        let decision = gate::evaluate(&self.log.read(self.started), self.started, now, GatePolicy::Release);
        if decision.verdict == GateVerdict::Allow {
            return Ok(());
        }
        self.release_without_io();
        self.state = if decision.verdict == GateVerdict::Refuse {
            SessionState::Blocked
        } else {
            SessionState::WaitingForCharacter
        };
        Err(SessionError::Gate(decision))
    }

    pub(super) fn abandon_pending_write(&mut self) {
        if let Some(record) = &mut self.last_write
            && record.status == WriteStatus::Pending
        {
            record.status = WriteStatus::uncertain("write future abandoned before complete confirmation; never replay");
        }
        if let Some(record) = &mut self.last_rate_write
            && record.status == crate::field_writes::RateWriteStatus::Pending
        {
            record.status = crate::field_writes::RateWriteStatus::uncertain("rate write future abandoned before complete confirmation; never replay");
        }
    }
}
