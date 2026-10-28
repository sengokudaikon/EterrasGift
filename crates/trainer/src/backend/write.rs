use super::*;

pub(super) struct CurrentIntent {
    pub(super) intent: Arc<Mutex<u64>>,
    pub(super) epoch: u64,
}
impl WriteControl for CurrentIntent {
    fn allowed(&mut self) -> bool {
        *self.intent.lock().unwrap_or_else(|p| p.into_inner()) == self.epoch
    }
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn write_forge(&mut self, plan: ForgePlan) -> Result<(), BackendError> {
        self.execute_forge(plan, false).await
    }
    pub(super) async fn undo_forge(&mut self, undo: crate::item::undo::ForgeUndo) -> Result<(), BackendError> {
        let plan = self.state.forge_undo_plan(&undo).map_err(invalid_edit)?;
        self.execute_forge(plan, true).await
    }
    async fn execute_forge(&mut self, plan: ForgePlan, undoing: bool) -> Result<(), BackendError> {
        let result = self.write_forge_inner(plan, undoing).await;
        self.check_write_failure(result)
    }
    // Both write families must revoke a failed session even when an error occurs
    // before submission (for example a failed refresh or exhausted publication counter).
    pub(super) fn check_write_failure(&mut self, result: Result<(), BackendError>) -> Result<(), BackendError> {
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
    async fn write_forge_inner(&mut self, plan: ForgePlan, undoing: bool) -> Result<(), BackendError> {
        if !self.state.enabled {
            return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
        }
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        if !self.connection.as_ref().is_some_and(|connection| connection.capabilities().allows(plan.edit_intent())) {
            return Err(BackendError::blocked(ErrorKind::NotReady, "this Forge edit is unavailable"));
        }
        let current = self.state.expected_observation().map_err(invalid_edit)?;
        let manager = self.state.runtime_status.ok_or_else(|| invalid_edit("runtime status missing"))?.manager;
        plan.recheck(&current, manager, self.state.override_enabled).map_err(invalid_edit)?;
        let target = current.process().clone();
        self.admit_using_policy(&target, GatePolicy::Release)?;
        let refreshed = self.connection.as_mut().ok_or_else(|| invalid_edit("connection released"))?.refresh().await;
        let status = match refreshed {
            Ok(status) => status,
            Err(error) => {
                self.handle_connection_gate(&error);
                if !matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading) {
                    self.fail(error.clone(), Some(target));
                }
                return Err(error);
            }
        };
        self.superseded(epoch)?;
        self.check_status(status)?;
        if status.manager != plan.manager() {
            return Err(BackendError::blocked(ErrorKind::Identity, "character manager changed before write"));
        }
        self.admit_using_policy(&target, GatePolicy::Release)?;
        self.commit(epoch, |this| {
            this.state.activity = Some(Activity::WritingForge);
            this.state.error = None;
            this.state.undo_forge = None;
            this.state.last_write = Some(WriteRecord {
                plan: plan.clone(),
                status: WriteStatus::Pending,
            });
        })?;
        let mut control = CurrentIntent {
            intent: self.intent.clone(),
            epoch,
        };
        let write_plan = WritePlan::from(plan.clone());
        let (status, failure) = self.submit_write(&write_plan, &mut control).await;
        let status = status.map(WriteObservation::into_forge);
        self.log_forge_outcome(&plan, &status);
        self.state.last_write = Some(WriteRecord {
            plan: plan.clone(),
            status: status.clone(),
        });
        self.state.activity = None;
        if self.superseded(epoch).is_err() {
            // The cleanup command executes next. Its snapshot will retain this
            // record, while the superseded generation gets no item publication.
            return Err(BackendError::blocked(
                ErrorKind::Superseded,
                "write drained before queued cleanup; inspect its retained outcome",
            ));
        }
        let kept = matches!(&status, WriteStatus::Confirmed(_));
        let mut describe_failure = None;
        let result = match status {
            WriteStatus::Confirmed(snapshot) | WriteStatus::NotKept(snapshot) => {
                // The readback is a changed item: the game describes its implicits again before it is published, so the
                // item and its text arrive together (the monitor keeps text only for an item that has not changed).
                let implicits = match self.connection.as_mut().unwrap().describe_implicits(&snapshot).await {
                    Ok(implicits) => implicits,
                    Err(error) => {
                        describe_failure = Some(error);
                        None
                    }
                };
                if let Err(error) = self.admit_using_policy(&target, GatePolicy::Release) {
                    // Confirmation is historical. Admission controls publication,
                    // not whether the completed write is retroactively unknown.
                    Err(error)
                } else {
                    let data = self.state.catalog.data().cloned().ok_or_else(|| invalid_edit("catalog missing after write"))?;
                    if kept {
                        let _ = self.forge_history.confirmed(plan.prepared(), plan.manager(), &snapshot, undoing);
                    } else {
                        self.forge_history.clear();
                    }
                    let undo = self
                        .forge_history
                        .current()
                        .cloned()
                        .filter(|undo| undo.prepare_with_override(undo.after(), plan.manager(), &data, self.state.override_enabled).is_ok());
                    if undo.is_none() {
                        self.forge_history.clear();
                    }
                    let mut forge = self.forge_state(Some(snapshot), &data);
                    if let ForgeState::Item { implicits: observed, .. } = &mut forge {
                        *observed = implicits;
                    }
                    self.commit(epoch, |this| {
                        this.state.forge = forge;
                        this.state.undo_forge = undo;
                    })?;
                    if kept {
                        Ok(())
                    } else {
                        Err(write_error(
                            ErrorKind::WriteNotKept,
                            "independent readback differs from the requested item",
                            Delivery::MayHaveRun,
                        ))
                    }
                }
            }
            WriteStatus::NotApplied { message, .. } => {
                self.state.undo_forge = self.forge_history.current().cloned();
                Err(write_error(ErrorKind::WriteNotApplied, message, Delivery::NotSent))
            }
            WriteStatus::Uncertain { message } => {
                self.forge_history.clear();
                Err(write_error(ErrorKind::WriteUncertain, message, Delivery::MayHaveRun))
            }
            WriteStatus::Pending => unreachable!(),
        };
        // The write's outcome stands; a session that broke while describing is revoked as any other.
        if let Some(error) = describe_failure {
            let _ = self.check_write_failure(Err(error));
        }
        self.finish_write(result, failure, target)
    }
    pub(super) async fn submit_write(&mut self, plan: &WritePlan, control: &mut dyn WriteControl) -> (WriteOutcome<WriteObservation>, Option<BackendError>) {
        let (status, mut failure) = match self.connection.as_mut().unwrap().write(plan, control).await {
            Ok(status) => (status, None),
            Err(error) => {
                let status = if error.delivery == Some(Delivery::MayHaveRun) {
                    WriteOutcome::uncertain(error.message.clone())
                } else {
                    WriteOutcome::NotApplied {
                        code: ResponseCode::Refused,
                        message: error.message.clone(),
                    }
                };
                (status, Some(error))
            }
        };
        let status = plan.completed(status);
        if let WriteOutcome::NotApplied { code, message } = &status
            && let Some(error) = refusal(*code, message)
        {
            failure = Some(error);
        }
        (status, failure)
    }
    pub(super) fn finish_write(&mut self, result: Result<(), BackendError>, failure: Option<BackendError>, target: ProcessIdentity) -> Result<(), BackendError> {
        let result = match failure {
            Some(error) => Err(error),
            None => result,
        };
        if let Err(error) = &result {
            let reusable = (error.kind == ErrorKind::WriteNotKept
                || (matches!(error.kind, ErrorKind::WriteNotApplied | ErrorKind::InvalidEdit | ErrorKind::NotReady) && error.delivery != Some(Delivery::MayHaveRun)))
                && self.connection.as_ref().is_some_and(BackendConnection::is_ready);
            if reusable {
                self.state.error = Some(error.clone());
                self.publish();
            } else if matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading) {
                self.handle_connection_gate(error);
            } else if self.connection.is_some() {
                self.fail(error.clone(), Some(target));
            }
        }
        result
    }
}
/// A helper's refusal that says more than "not applied": the session's state changed under the request.
pub(super) fn refusal(code: ResponseCode, message: &str) -> Option<BackendError> {
    let kind = match code {
        ResponseCode::OfflineRequired => ErrorKind::Gate,
        ResponseCode::CharacterLoading => ErrorKind::CharacterLoading,
        ResponseCode::RestartRequired => ErrorKind::RestartRequired,
        _ => return None,
    };
    Some(BackendError::blocked(kind, message))
}
pub(super) fn invalid_edit(error: impl fmt::Display) -> BackendError {
    BackendError::blocked(ErrorKind::InvalidEdit, error.to_string())
}
pub(super) fn write_error(kind: ErrorKind, message: impl Into<String>, delivery: Delivery) -> BackendError {
    BackendError {
        kind,
        message: message.into(),
        delivery: Some(delivery),
        retry_when_waiting: false,
    }
}
