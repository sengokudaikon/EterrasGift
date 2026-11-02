//! Generic read/apply operations, bound to a connection generation and full observation. Feature semantics live in
//! the helper. Accepted edits use the same admission, no-retry/readback and cleanup policy as the other worker writes.
use super::*;
use eg_trainer_wire::field_edits::{Edit, FieldSet, Snapshot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldRead {
    pub generation: u64,
    pub target: ProcessIdentity,
    pub snapshot: Snapshot,
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn read_fields(&mut self, set: FieldSet) -> Result<(), BackendError> {
        let epoch = self.command_epoch;
        let target = self.state.attached.clone().ok_or_else(released)?;
        self.admit(&target)?;
        if !self.state.capabilities.fields[set as usize] {
            return Err(BackendError::blocked(ErrorKind::NotReady, "field reads are unavailable"));
        }
        let snapshot = self.call(epoch, &target, async |connection| connection.read_fields(set).await).await?;
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => return self.finish_write(Err(error), None, target),
        };
        self.admit(&target)?;
        self.commit(epoch, |this| {
            this.state.fields.insert(
                set,
                FieldRead {
                    generation: this.state.generation,
                    target,
                    snapshot,
                },
            );
        })
    }
    pub(super) async fn write_fields(&mut self, expected: FieldRead, values: Vec<Option<i32>>) -> Result<(), BackendError> {
        let epoch = self.command_epoch;
        self.admit(&expected.target)?;
        if expected.generation != self.state.generation || self.state.fields.get(&expected.snapshot.set) != Some(&expected) {
            return Err(BackendError::blocked(ErrorKind::InvalidEdit, "Field observation changed; read again"));
        }
        let edit = Edit {
            expected: expected.snapshot.clone(),
            values,
        };
        if !self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.capabilities().fields[edit.expected.set as usize])
        {
            return Err(BackendError::blocked(ErrorKind::NotReady, "field edits are unavailable"));
        }
        edit.validate().map_err(|e| BackendError::blocked(ErrorKind::InvalidEdit, e.to_string()))?;
        self.recheck_before_write(epoch, &expected.target, edit.expected.manager).await?;
        let mut control = write::CurrentIntent {
            intent: self.intent.clone(),
            epoch,
        };
        let outcome = self.connection.as_mut().ok_or_else(released)?.apply_fields(&edit, &mut control).await;
        let mut failure = None;
        let result = match outcome {
            Ok(WriteOutcome::Confirmed(mut snapshot)) => {
                // The row compares requested fields with this full, independent read and reports any changed result.
                // Keep surviving rows' displayed names until the next explicit Read, including duplicate-name suffixes.
                for field in &mut snapshot.fields {
                    if let Some(before) = expected.snapshot.fields.iter().find(|before| before.key == field.key && before.identity == field.identity) {
                        field.label.clone_from(&before.label);
                    }
                }
                self.state.fields.insert(snapshot.set, FieldRead { snapshot, ..expected.clone() });
                Ok(())
            }
            Ok(WriteOutcome::NotKept(snapshot)) => {
                self.state.fields.insert(snapshot.set, FieldRead { snapshot, ..expected.clone() });
                Err(write::write_error(
                    ErrorKind::WriteNotKept,
                    "The game kept different fields or counts; read again",
                    Delivery::MayHaveRun,
                ))
            }
            Ok(WriteOutcome::NotApplied { code, message }) => {
                failure = write::refusal(code, &message);
                Err(write::write_error(ErrorKind::WriteNotApplied, message, Delivery::NotSent))
            }
            Ok(WriteOutcome::Uncertain { message }) => Err(write::write_error(ErrorKind::WriteUncertain, message, Delivery::MayHaveRun)),
            Ok(WriteOutcome::Pending) => Err(write::write_error(ErrorKind::WriteUncertain, "Field change has no confirmed outcome", Delivery::MayHaveRun)),
            Err(error) => Err(error),
        };
        self.publish_if_current(epoch).map_err(|mut error| {
            error.delivery = Some(Delivery::MayHaveRun);
            error
        })?;
        self.finish_write(result, failure, expected.target)
    }
}
