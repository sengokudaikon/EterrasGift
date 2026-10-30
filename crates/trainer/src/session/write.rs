use super::*;
use crate::write::{AllowWrite, ForgePlan, WriteControl, WriteOutcome, WriteStatus};
use eg_trainer_wire::{
    protocol::Response,
    transport::Delivery,
    write::{ForgeWrite, WriteReply},
};

impl<T: Transport> Session<T> {
    pub async fn edit_forge(&mut self, plan: &ForgePlan) -> Result<WriteStatus, SessionError> {
        self.edit_forge_controlled(plan, &mut AllowWrite).await
    }
    /// Submit once. Err is pre-submission; all later failures retain certainty.
    /// A native ACK is provisional until a separate admitted read completes.
    /// Dropping this future leaves Pending and blocks further work until cleanup.
    pub async fn edit_forge_controlled(&mut self, plan: &ForgePlan, control: &mut dyn WriteControl) -> Result<WriteStatus, SessionError> {
        self.admit()?;
        let dispatcher = self.dispatcher()?;
        let (request, readback) = self.prepare_write(plan, dispatcher)?;
        self.provenance(&dispatcher)?;
        if !control.allowed() {
            return Err(SessionError::Busy);
        }
        let acknowledged = |response: &Response| classify_ack(&request, response);
        let observed = |_: &mut Self, payload: &[u8]| decode_readback(plan, payload);
        Ok(self.mutate(dispatcher, &request, &readback, control, acknowledged, observed).await)
    }

    /// A mutation, submitted once, then read back independently: only after an acknowledgement that confirms it, under
    /// the same artifact lease, and while no cleanup is queued. The session stays usable after an outcome that is
    /// certain and reports what the game holds; anything else, or a lost lease, ends it.
    pub(super) async fn mutate<O>(
        &mut self,
        dispatcher: DispatcherSpec,
        request: &Request,
        readback: &Request,
        control: &mut dyn WriteControl,
        acknowledged: impl FnOnce(&Response) -> WriteOutcome<O>,
        observed: impl FnOnce(&mut Self, &[u8]) -> WriteOutcome<O>,
    ) -> WriteOutcome<O> {
        let runtime = self.status.take();
        self.state = SessionState::Connecting;
        let (mut result, mut healthy) = match self.transport.send(request).await {
            Ok(response) => (
                acknowledged(&response),
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
        if let Err(error) = self.provenance(&dispatcher) {
            healthy = false;
            if matches!(result, WriteOutcome::Confirmed(_)) {
                result = WriteOutcome::uncertain(format!("write reply lost confirmation provenance: {error}"));
            }
        }
        if healthy && matches!(result, WriteOutcome::Confirmed(_)) {
            if !control.allowed() {
                result = WriteOutcome::uncertain("write acknowledged before queued cleanup; separate readback was not submitted");
            } else {
                result = match self.transport.send(readback).await {
                    Ok(response) if response.code == ResponseCode::Done => observed(self, &response.payload),
                    Ok(response) => WriteOutcome::uncertain(format!("write sent but independent readback was refused: {:?}", response.code)),
                    Err(error) => WriteOutcome::uncertain(format!("write sent but independent readback failed: {error}")),
                };
                if matches!(result, WriteOutcome::Confirmed(_) | WriteOutcome::NotKept(_))
                    && let Err(error) = self.provenance(&dispatcher)
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
            let loading = matches!(
                result,
                WriteOutcome::NotApplied {
                    code: ResponseCode::CharacterLoading,
                    ..
                }
            );
            self.status = if loading { None } else { runtime };
            self.state = SessionState::Ready;
        } else {
            self.revoke();
        }
        result
    }

    fn prepare_write(&self, plan: &ForgePlan, dispatcher: DispatcherSpec) -> Result<(Request, Request), SessionError> {
        let expected = plan.before();
        if !self.observation_matches(expected) || self.status.as_ref().is_none_or(|status| status.online || status.manager != plan.manager()) {
            return Err(SessionError::Identity("write belongs to a different runtime, manager or catalog".into()));
        }
        let request = ForgeWrite::edit(dispatcher, plan.manager(), expected.snapshot().clone(), plan.mutation()).map_err(SessionError::Protocol)?;
        Ok((Request::EditForge(request), Request::ReadForge { dispatcher }))
    }
}

fn classify_ack(request: &Request, response: &Response) -> WriteStatus {
    match request {
        Request::EditForge(request) => match eg_trainer_wire::write::classify_response(response, request) {
            WriteReply::Confirmed(actual) => WriteOutcome::Confirmed(actual),
            WriteReply::NotApplied { code, message } => WriteOutcome::NotApplied { code, message },
            WriteReply::Uncertain { reason } => WriteOutcome::uncertain(reason),
        },
        _ => unreachable!("prepared mutation request"),
    }
}
fn decode_readback(plan: &ForgePlan, payload: &[u8]) -> WriteStatus {
    match forge::decode(payload) {
        Ok(Some(actual)) => plan.observe(actual),
        Ok(None) => WriteOutcome::uncertain("independent readback has no complete target observation"),
        Err(error) => WriteOutcome::uncertain(format!("invalid independent write readback: {error}")),
    }
}
