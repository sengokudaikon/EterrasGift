use super::*;

struct CurrentIntent {
    intent: Arc<Mutex<u64>>,
    epoch: u64,
}
impl WriteControl for CurrentIntent {
    fn allowed(&mut self) -> bool {
        *self.intent.lock().unwrap_or_else(|p| p.into_inner()) == self.epoch
    }
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn write_level_requirement(&mut self, plan: LevelRequirementPlan) -> Result<(), BackendError> {
        self.execute_level_requirement(plan, false).await
    }
    pub(super) async fn undo_level_requirement(&mut self, undo: crate::undo::LevelUndo) -> Result<(), BackendError> {
        let plan = self.state.level_undo_plan(&undo).map_err(invalid_edit)?;
        self.execute_level_requirement(plan, true).await
    }
    async fn execute_level_requirement(&mut self, plan: LevelRequirementPlan, undoing: bool) -> Result<(), BackendError> {
        let result = self.write_level_requirement_inner(plan, undoing).await;
        if let Err(error) = &result
            && self.state.phase == Phase::Ready
            && matches!(error.kind, ErrorKind::Transport | ErrorKind::Protocol | ErrorKind::Identity | ErrorKind::RestartRequired)
        {
            self.fail(error.clone(), self.state.attached.clone());
        }
        result
    }
    async fn write_level_requirement_inner(&mut self, plan: LevelRequirementPlan, undoing: bool) -> Result<(), BackendError> {
        if !self.state.enabled {
            return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
        }
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        if !self.connection.as_ref().is_some_and(BackendConnection::supports_level_requirement) {
            return Err(BackendError::blocked(ErrorKind::NotReady, "level requirement writes are unavailable"));
        }
        let current = self.state.expected_observation().map_err(invalid_edit)?;
        let manager = self.state.runtime_status.ok_or_else(|| invalid_edit("runtime status missing"))?.manager;
        plan.recheck(&current, manager).map_err(invalid_edit)?;
        let target = current.process().clone();
        self.admit_using_policy(&target, GatePolicy::Release)?;
        let refreshed = self.connection.as_mut().ok_or_else(|| invalid_edit("connection released"))?.refresh(self.clock.now()).await;
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
        if status.manager != plan.manager() {
            return Err(BackendError::blocked(ErrorKind::Identity, "character manager changed before write"));
        }
        plan.recheck(&current, status.manager).map_err(invalid_edit)?;
        self.admit_using_policy(&target, GatePolicy::Release)?;
        self.commit(epoch, |this| {
            this.state.activity = Some(Activity::WritingForge);
            this.state.error = None;
            this.state.undo_level_requirement = None;
            this.state.last_write = Some(WriteRecord {
                plan: plan.clone(),
                status: WriteStatus::Pending,
            });
        })?;
        let mut control = CurrentIntent {
            intent: self.intent.clone(),
            epoch,
        };
        let status = match self.connection.as_mut().unwrap().set_level_requirement(self.clock.now(), &plan, &mut control).await {
            Ok(status) => status,
            Err(error) if error.delivery == Some(Delivery::MayHaveRun) => WriteStatus::uncertain(error.message),
            Err(error) => WriteStatus::NotApplied {
                code: ResponseCode::Refused,
                message: error.message,
            },
        };
        // This operation has drained. Its outcome belongs to the submitted plan
        // even if cleanup was queued while it ran. Never discard certainty, and
        // never publish its forge snapshot into a newer generation.
        let status = match status {
            WriteStatus::Confirmed(snapshot) if &snapshot == plan.projected() => WriteStatus::Confirmed(snapshot),
            WriteStatus::Confirmed(_) => WriteStatus::uncertain("connection returned a mismatched write confirmation"),
            WriteStatus::Pending => WriteStatus::uncertain("connection returned before write completion"),
            WriteStatus::NotApplied {
                code: ResponseCode::Done | ResponseCode::Failed,
                ..
            } => WriteStatus::uncertain("connection mislabeled a success/failure code as pre-store refusal"),
            status => status,
        };
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
        let result = match status {
            WriteStatus::Confirmed(snapshot) => {
                if let Err(error) = self.admit_using_policy(&target, GatePolicy::Release) {
                    let uncertain = WriteStatus::uncertain(format!("admission lost before write publication: {error}"));
                    self.state.last_write.as_mut().unwrap().status = uncertain;
                    Err(write_error(
                        ErrorKind::WriteUncertain,
                        "write reply could not be confirmed after admission loss",
                        Delivery::MayHaveRun,
                    ))
                } else {
                    let data = self.state.catalog.data().cloned().ok_or_else(|| invalid_edit("catalog missing after write"))?;
                    let forge = self.forge_state(Some(snapshot), &data);
                    let undo = if undoing {
                        None
                    } else {
                        self.state.last_write.as_ref().and_then(|record| crate::undo::LevelUndo::from_confirmed(record).ok())
                    };
                    self.commit(epoch, |this| {
                        this.state.forge = forge;
                        this.state.undo_level_requirement = undo;
                    })?;
                    Ok(())
                }
            }
            WriteStatus::NotApplied { message, .. } => Err(write_error(ErrorKind::WriteNotApplied, message, Delivery::NotSent)),
            WriteStatus::Uncertain { message } => Err(write_error(ErrorKind::WriteUncertain, message, Delivery::MayHaveRun)),
            WriteStatus::Pending => unreachable!(),
        };
        if let Err(error) = &result {
            self.fail(error.clone(), Some(target));
        }
        result
    }
}
fn invalid_edit(error: impl fmt::Display) -> BackendError {
    BackendError::blocked(ErrorKind::InvalidEdit, error.to_string())
}
fn write_error(kind: ErrorKind, message: impl Into<String>, delivery: Delivery) -> BackendError {
    BackendError {
        kind,
        message: message.into(),
        delivery: Some(delivery),
        retry_when_waiting: false,
    }
}
