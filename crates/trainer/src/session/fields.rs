use super::*;
use crate::write::{WriteControl, WriteOutcome};
use eg_trainer_wire::{
    field_edits::{Edit, FieldSet, Snapshot},
    protocol::Response,
};

impl<T: Transport> Session<T> {
    pub async fn read_fields(&mut self, set: FieldSet) -> Result<Snapshot, SessionError> {
        let manager = self.offline_manager()?;
        let snapshot = self
            .read_owned(
                |dispatcher| Request::ReadFields {
                    dispatcher,
                    expected_manager: manager,
                    set,
                },
                Snapshot::decode,
            )
            .await?;
        if snapshot.manager != manager || snapshot.set != set {
            self.revoke();
            return Err(SessionError::Identity("Field observation belongs to another character or editor".into()));
        }
        Ok(snapshot)
    }
    pub async fn apply_fields(&mut self, edit: &Edit, control: &mut dyn WriteControl) -> Result<WriteOutcome<Snapshot>, SessionError> {
        edit.validate().map_err(SessionError::Protocol)?;
        self.admit()?;
        let dispatcher = self.dispatcher()?;
        if self.offline_manager()? != edit.expected.manager {
            return Err(SessionError::Identity("Character changed; read again".into()));
        }
        self.provenance(&dispatcher)?;
        if !control.allowed() {
            return Err(SessionError::Busy);
        }
        let request = Request::WriteFields {
            dispatcher,
            edit: Box::new(edit.clone()),
        };
        let readback = Request::ReadFields {
            dispatcher,
            expected_manager: edit.expected.manager,
            set: edit.expected.set,
        };
        let acknowledged = |response: &Response| match response.code {
            ResponseCode::Done if response.payload.is_empty() => WriteOutcome::Confirmed(edit.expected.clone()),
            ResponseCode::NotReady
            | ResponseCode::StaleItem
            | ResponseCode::Refused
            | ResponseCode::CharacterLoading
            | ResponseCode::OfflineRequired
            | ResponseCode::RestartRequired => WriteOutcome::NotApplied {
                code: response.code,
                message: String::from_utf8_lossy(&response.payload).into_owned(),
            },
            _ => WriteOutcome::uncertain("Field edit returned no valid acknowledgement"),
        };
        let observed = |_: &mut Self, bytes: &[u8]| match Snapshot::decode(bytes) {
            Ok(actual) if actual.manager == edit.expected.manager && actual.set == edit.expected.set => {
                if actual.keeps(&edit.expected, &edit.values) {
                    WriteOutcome::Confirmed(actual)
                } else {
                    WriteOutcome::NotKept(actual)
                }
            }
            _ => WriteOutcome::uncertain("Field readback belongs to another character or is invalid"),
        };
        Ok(self.mutate(dispatcher, &request, &readback, control, acknowledged, observed).await)
    }
}
