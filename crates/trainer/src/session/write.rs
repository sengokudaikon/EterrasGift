use super::*;
use crate::{
    field_writes::{RateWriteRecord, RateWriteStatus},
    writes::{AllowWrite, ForgePlan, WriteControl, WriteObservation, WriteOutcome, WritePlan, WriteRecord, WriteStatus},
};
use eg_trainer_wire::{
    rate_write::{AttackLeechRateWrite, RateWriteReply},
    transport::Delivery,
    write::{ForgeWrite, WriteReply},
};

impl<T: Transport> Session<T> {
    pub fn last_write(&self) -> Option<&WriteRecord> {
        self.last_write.as_ref()
    }

    pub async fn edit_forge(&mut self, plan: &ForgePlan) -> Result<WriteStatus, SessionError> {
        self.edit_forge_controlled(plan, &mut AllowWrite).await
    }
    pub async fn edit_forge_controlled(&mut self, plan: &ForgePlan, control: &mut dyn WriteControl) -> Result<WriteStatus, SessionError> {
        Ok(self.write_controlled(&plan.clone().into(), control).await?.map(WriteObservation::into_forge))
    }

    /// Submit once. Err is pre-submission; all later failures retain certainty.
    /// A native ACK is provisional until a separate admitted read completes.
    /// Dropping this future leaves Pending and blocks further work until cleanup.
    pub async fn write(&mut self, plan: &WritePlan) -> Result<WriteOutcome<WriteObservation>, SessionError> {
        self.write_controlled(plan, &mut AllowWrite).await
    }
    pub async fn write_controlled(&mut self, plan: &WritePlan, control: &mut dyn WriteControl) -> Result<WriteOutcome<WriteObservation>, SessionError> {
        self.admit()?;
        let dispatcher = self.dispatcher()?;
        let (request, readback) = self.prepare_write(plan, dispatcher)?;
        self.write_provenance(&dispatcher)?;
        if !control.allowed() {
            return Err(SessionError::Busy);
        }
        let runtime = self.status.take();
        self.state = SessionState::Connecting;
        self.record_write(plan, WriteOutcome::Pending);
        let (mut result, mut healthy) = match self.transport.send(&request).await {
            Ok(response) => (
                classify_ack(&request, &response),
                !matches!(response.code, ResponseCode::OfflineRequired | ResponseCode::RestartRequired),
            ),
            Err(error) => {
                let result = if error.delivery == Delivery::NotSent {
                    WriteOutcome::NotApplied {
                        code: ResponseCode::NotReady,
                        message: error.to_string(),
                    }
                } else {
                    WriteOutcome::uncertain(error.to_string())
                };
                (result, false)
            }
        };
        // A trusted pre-store refusal stays definitive after admission loss.
        // No further native I/O follows a failed gate or changed artifact.
        if let Err(error) = self.write_provenance(&dispatcher) {
            healthy = false;
            if matches!(result, WriteOutcome::Confirmed(_)) {
                result = WriteOutcome::uncertain(format!("write reply lost confirmation provenance: {error}"));
            }
        }
        if healthy && matches!(result, WriteOutcome::Confirmed(_)) {
            if !control.allowed() {
                result = WriteOutcome::uncertain("write acknowledged before queued cleanup; separate readback was not submitted");
            } else {
                result = match self.transport.send(&readback).await {
                    Ok(response) if response.code == ResponseCode::Done => decode_readback(plan, &response.payload),
                    Ok(response) => WriteOutcome::uncertain(format!("write sent but independent readback was refused: {:?}", response.code)),
                    Err(error) => WriteOutcome::uncertain(format!("write sent but independent readback failed: {error}")),
                };
                if matches!(result, WriteOutcome::Confirmed(_) | WriteOutcome::NotKept(_))
                    && let Err(error) = self.write_provenance(&dispatcher)
                {
                    result = WriteOutcome::uncertain(format!("independent readback lost confirmation provenance: {error}"));
                }
            }
        }
        let reusable = healthy
            && matches!(
                result,
                WriteOutcome::Confirmed(_)
                    | WriteOutcome::NotKept(_)
                    | WriteOutcome::NotApplied {
                        code: ResponseCode::NotReady | ResponseCode::StaleItem | ResponseCode::Refused | ResponseCode::CharacterLoading,
                        ..
                    }
            );
        if reusable {
            self.status = if matches!(
                result,
                WriteOutcome::NotApplied {
                    code: ResponseCode::CharacterLoading,
                    ..
                }
            ) {
                None
            } else {
                runtime
            };
            self.state = SessionState::Ready;
        } else {
            self.release_without_io();
            if self.state == SessionState::Connecting {
                self.state = SessionState::Disconnected;
            }
        }
        self.record_write(plan, result.clone());
        Ok(result)
    }

    fn prepare_write(&self, plan: &WritePlan, dispatcher: DispatcherSpec) -> Result<(Request, Request), SessionError> {
        match plan {
            WritePlan::Forge(plan) => {
                let expected = plan.before();
                if !self.observation_matches(expected) || self.status.as_ref().is_none_or(|status| status.online || status.manager != plan.manager()) {
                    return Err(SessionError::Identity("write belongs to a different runtime, manager or catalog".into()));
                }
                let request = ForgeWrite::edit(dispatcher, plan.manager(), expected.snapshot().clone(), plan.mutation()).map_err(SessionError::Protocol)?;
                Ok((Request::EditForge(request), Request::ReadForge { dispatcher }))
            }
            WritePlan::Field(plan) => {
                let expected = plan.before();
                if expected.process != self.target || self.status.as_ref().is_none_or(|s| s.online || s.manager == 0 || s.manager != expected.manager) {
                    return Err(SessionError::Identity("field write belongs to a different process or offline manager".into()));
                }
                let AttackLeechRead::Available(before) = expected.read else {
                    return Err(SessionError::Identity("field plan has no complete expected observation".into()));
                };
                let target = match plan.target() {
                    crate::field_writes::AttackLeechTarget::AttackRate => fields::AttackLeechTarget::AttackRate,
                    crate::field_writes::AttackLeechTarget::IncreasedLeechRate => fields::AttackLeechTarget::IncreasedLeechRate,
                };
                let request = AttackLeechRateWrite::new(dispatcher, before, target, plan.projected_bits()).map_err(SessionError::Protocol)?;
                Ok((
                    Request::SetAttackLeechRate(request),
                    Request::ReadAttackLeech {
                        dispatcher,
                        expected_manager: expected.manager,
                    },
                ))
            }
        }
    }
    pub(super) fn write_provenance(&mut self, dispatcher: &DispatcherSpec) -> Result<(), SessionError> {
        if let Err(error) = self.validate_profile(dispatcher) {
            self.release_without_io();
            self.state = SessionState::Disconnected;
            return Err(SessionError::Identity(error.to_string()));
        }
        Ok(())
    }
    fn record_write(&mut self, plan: &WritePlan, status: WriteOutcome<WriteObservation>) {
        match plan {
            WritePlan::Forge(plan) => {
                self.last_write = Some(WriteRecord {
                    plan: (**plan).clone(),
                    status: status.map(WriteObservation::into_forge),
                })
            }
            WritePlan::Field(plan) => {
                self.last_rate_write = Some(RateWriteRecord {
                    plan: (**plan).clone(),
                    status: status.map(WriteObservation::into_field),
                })
            }
        }
    }
    pub(super) fn abandon_pending_write(&mut self) {
        if let Some(record) = &mut self.last_write
            && record.status == WriteStatus::Pending
        {
            record.status = WriteStatus::uncertain("write future abandoned before complete confirmation; never replay");
        }
        if let Some(record) = &mut self.last_rate_write
            && record.status == RateWriteStatus::Pending
        {
            record.status = RateWriteStatus::uncertain("field write future abandoned before complete confirmation; never replay");
        }
    }
}

fn classify_ack(request: &Request, response: &eg_trainer_wire::protocol::Response) -> WriteOutcome<WriteObservation> {
    match request {
        Request::EditForge(request) => match eg_trainer_wire::write::classify_response(response, request) {
            WriteReply::Confirmed(actual) => WriteOutcome::Confirmed(WriteObservation::forge(actual)),
            WriteReply::NotApplied { code, message } => WriteOutcome::NotApplied { code, message },
            WriteReply::Uncertain { reason } => WriteOutcome::uncertain(reason),
        },
        Request::SetAttackLeechRate(request) => match eg_trainer_wire::rate_write::classify_response(response, request) {
            RateWriteReply::Confirmed(actual) => WriteOutcome::Confirmed(WriteObservation::Field(actual)),
            RateWriteReply::NotApplied { code, message } => WriteOutcome::NotApplied { code, message },
            RateWriteReply::Uncertain { reason } => WriteOutcome::uncertain(reason),
        },
        _ => unreachable!("prepared mutation request"),
    }
}
fn decode_readback(plan: &WritePlan, payload: &[u8]) -> WriteOutcome<WriteObservation> {
    let actual = match plan {
        WritePlan::Forge(_) => forge::decode(payload).map(|actual| actual.map(WriteObservation::forge)),
        WritePlan::Field(_) => fields::decode(payload).map(|actual| match actual {
            AttackLeechRead::Available(actual) => Some(WriteObservation::Field(actual)),
            AttackLeechRead::Unavailable(_) => None,
        }),
    };
    match actual {
        Ok(Some(actual)) => plan.observe(actual),
        Ok(None) => WriteOutcome::uncertain("independent readback has no complete target observation"),
        Err(error) => WriteOutcome::uncertain(format!("invalid independent write readback: {error}")),
    }
}
