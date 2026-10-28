use super::*;
use crate::item::edit::ExpectedObservation;

// One entry per possible affix plus a bounded set of current draft variations.
// Arc snapshots share tables, never clone their 256 sample payloads.
const MAX_MAPPINGS: usize = 64;

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn resolve_mapping(&mut self, expected: ExpectedObservation, request: MappingRequest) -> Result<(), BackendError> {
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        let mut control = MappingControl {
            intent: self.intent.clone(),
            epoch,
            cancel: self.read_cancel.clone(),
        };
        if control.control() == ReadControl::Stop {
            return Err(BackendError::blocked(ErrorKind::Superseded, "Value request canceled."));
        }
        if self.state.expected_observation().as_ref() != Ok(&expected) {
            return Err(BackendError::blocked(ErrorKind::InvalidEdit, "The item changed before its values could be read."));
        }
        request
            .validate(&expected, None)
            .map_err(|error| BackendError::blocked(ErrorKind::MappingUnavailable, error.to_string()))?;
        let target = expected.process().clone();
        self.admit(&target)?;
        if self.refresh_mapping_locale(epoch, &target).await? {
            return Err(BackendError::blocked(
                ErrorKind::MappingRetryable,
                "The game language changed. Reading values in the selected language.",
            ));
        }
        if self.state.mapping(&expected, &request).is_some() {
            return Ok(());
        }
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
            .resolve_mapping(&expected, &request, &mut control)
            .await;
        self.superseded(epoch)?;
        // Optional formatting must not resurrect a failed offline/process gate.
        self.admit(&target)?;
        if let Ok(table) = &result
            && let Err(error) = table.recheck(&expected, &request)
        {
            let error = BackendError::blocked(ErrorKind::Protocol, error.to_string());
            self.fail(error.clone(), Some(target));
            return Err(error);
        }
        let optional_result = result.is_ok()
            || result.as_ref().is_err_and(|error| {
                matches!(
                    error.kind,
                    ErrorKind::MappingUnavailable | ErrorKind::MappingRetryable | ErrorKind::NotReady | ErrorKind::Superseded
                )
            });
        if control.control() == ReadControl::Stop && optional_result && self.connection.as_ref().is_some_and(BackendConnection::is_ready) {
            return Err(BackendError::blocked(ErrorKind::Superseded, "Value request canceled."));
        }
        // Context may also change midway through the paged sample. Refresh the
        // locale instead of memoizing that failure against the old catalog.
        if result.as_ref().is_err_and(|error| error.kind == ErrorKind::MappingRetryable) && self.refresh_mapping_locale(epoch, &target).await? {
            return Err(BackendError::blocked(
                ErrorKind::MappingRetryable,
                "The game language changed. Reading values in the selected language.",
            ));
        }
        let (result, retry_on_read) = match result {
            Ok(table) => (Ok(Arc::new(table)), false),
            Err(error)
                if matches!(error.kind, ErrorKind::MappingUnavailable | ErrorKind::MappingRetryable) && self.connection.as_ref().is_some_and(BackendConnection::is_ready) =>
            {
                let retry = error.kind == ErrorKind::MappingRetryable;
                (Err(error.message), retry)
            }
            Err(error) => {
                self.handle_connection_gate(&error);
                if !matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading) {
                    self.fail(error.clone(), Some(target));
                }
                return Err(error);
            }
        };
        self.commit(epoch, |this| {
            if this.state.expected_observation().as_ref() == Ok(&expected) {
                if this.state.mappings.len() == MAX_MAPPINGS {
                    this.state.mappings.remove(0);
                }
                this.state.mappings.push(ResolvedMapping {
                    expected,
                    request,
                    result,
                    retry_on_read,
                });
            }
        })
    }
    pub(super) async fn locale_changed(&mut self, epoch: u64, target: &ProcessIdentity) -> Result<bool, BackendError> {
        self.admit(target)?;
        let locale = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
            .read_locale()
            .await?;
        self.superseded(epoch)?;
        self.admit(target)?;
        Ok(self.state.catalog.data().is_none_or(|data| data.identity.locale != locale))
    }
    async fn refresh_mapping_locale(&mut self, epoch: u64, target: &ProcessIdentity) -> Result<bool, BackendError> {
        match self.locale_changed(epoch, target).await {
            Ok(false) => Ok(false),
            Ok(true) => {
                // The catalog again under the new language. read's commit drops
                // mappings/previews from the old identity atomically.
                self.read(super::CatalogRead::Reload, false).await?;
                Ok(true)
            }
            Err(error) => {
                self.handle_connection_gate(&error);
                if !matches!(error.kind, ErrorKind::Gate | ErrorKind::Superseded) {
                    self.fail(error.clone(), Some(target.clone()));
                }
                Err(error)
            }
        }
    }
}

struct MappingControl {
    intent: Arc<Mutex<u64>>,
    epoch: u64,
    cancel: Option<Arc<AtomicBool>>,
}
impl ReadObserver for MappingControl {
    fn control(&mut self) -> ReadControl {
        if *self.intent.lock().unwrap_or_else(|p| p.into_inner()) != self.epoch || self.cancel.as_ref().is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            ReadControl::Stop
        } else {
            ReadControl::Continue
        }
    }
}
