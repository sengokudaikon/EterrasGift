//! Read-only attack/leech publication; no general fields or write capability is granted.
use super::*;

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn read_attack_leech(&mut self) -> Result<(), BackendError> {
        if !self.state.enabled {
            return Err(BackendError::blocked(ErrorKind::Disabled, "trainer disabled"));
        }
        if self.state.phase != Phase::Ready {
            return Err(BackendError::blocked(ErrorKind::NotConnected, "session is not ready"));
        }
        if !self.connection.as_ref().is_some_and(|connection| connection.capabilities().read_attack_leech) {
            return Err(BackendError::blocked(ErrorKind::NotReady, "attack/leech reads are unavailable"));
        }
        let target = self
            .state
            .attached
            .clone()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "no offline character connected"))?;
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        let result: Result<(), BackendError> = async {
            self.admit(&target)?;
            self.state.activity = Some(Activity::ReadingAttackLeech);
            self.publish_if_current(epoch)?;
            let status = self.connection.as_mut().unwrap().refresh().await?;
            self.superseded(epoch)?;
            self.check_status(status)?;
            self.admit(&target)?;
            let read = self.connection.as_mut().unwrap().read_attack_leech().await?;
            self.superseded(epoch)?;
            self.admit(&target)?;
            if matches!(
                read,
                AttackLeechRead::Unavailable(AttackLeechUnavailable::OfflineRequired | AttackLeechUnavailable::ManagerAbsent | AttackLeechUnavailable::ManagerChanged)
            ) {
                return Err(BackendError::waiting(ErrorKind::Gate, "attack/leech read lost runtime offline/manager admission"));
            }
            let manager = self
                .state
                .runtime_status
                .filter(|status| !status.online && status.manager != 0)
                .ok_or_else(|| BackendError::waiting(ErrorKind::Gate, "attack/leech read has no admitted manager"))?
                .manager;
            if matches!(read, AttackLeechRead::Available(observation) if observation.manager.wrapper_token() != manager) {
                return Err(BackendError::blocked(ErrorKind::Protocol, "attack/leech manager differs from current runtime status"));
            }
            let sequence = self
                .attack_leech_sequence
                .checked_add(1)
                .ok_or_else(|| BackendError::blocked(ErrorKind::Fault, "attack/leech observation sequence exhausted"))?;
            self.commit(epoch, |this| {
                this.attack_leech_sequence = sequence;
                this.state.attack_leech = Some(AttackLeechSnapshot {
                    process: target.clone(),
                    generation: this.state.generation,
                    sequence,
                    manager,
                    read,
                });
                this.state.activity = None;
                this.state.error = None;
            })?;
            self.event(BackendEvent::ReadPublished {
                generation: self.state.generation,
            });
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.handle_connection_gate(error);
            if error.kind != ErrorKind::Superseded && !matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading) {
                self.fail(error.clone(), Some(target));
            }
        }
        result
    }
}
