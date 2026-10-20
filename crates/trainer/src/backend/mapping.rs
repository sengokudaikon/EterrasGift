use super::*;
use crate::edits::ExpectedObservation;

// One entry per possible affix plus a bounded set of current draft variations.
// Arc snapshots share tables, never clone their 256 sample payloads.
const MAX_MAPPINGS: usize = 64;

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn resolve_mapping(&mut self, expected: ExpectedObservation, request: MappingRequest) -> Result<(), BackendError> {
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        if self.state.expected_observation().as_ref() != Ok(&expected) {
            return Err(BackendError::blocked(ErrorKind::InvalidEdit, "The item changed before its values could be read."));
        }
        request
            .validate(&expected, None)
            .map_err(|error| BackendError::blocked(ErrorKind::MappingUnavailable, error.to_string()))?;
        if self.state.mapping(&expected, &request).is_some() {
            return Ok(());
        }
        let target = expected.process().clone();
        self.admit(&target)?;
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
            .resolve_mapping(self.clock.now(), &expected, &request)
            .await;
        self.superseded(epoch)?;
        // Optional formatting must not resurrect a failed offline/process gate.
        self.admit(&target)?;
        let (result, retry_on_read) = match result {
            Ok(table) => {
                table
                    .recheck(&expected, &request)
                    .map_err(|error| BackendError::blocked(ErrorKind::MappingUnavailable, error.to_string()))?;
                (Ok(Arc::new(table)), false)
            }
            Err(error)
                if matches!(error.kind, ErrorKind::MappingUnavailable | ErrorKind::MappingRetryable) && self.connection.as_ref().is_some_and(BackendConnection::is_ready) =>
            {
                let retry = error.kind == ErrorKind::MappingRetryable;
                (Err(error.message), retry)
            }
            Err(error) => {
                self.handle_connection_gate(&error);
                if error.kind != ErrorKind::Gate {
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
}
