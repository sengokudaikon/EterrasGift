use super::*;
use crate::{
    edits::{ExpectedObservation, RollEdit},
    mapping::{BoundTooltip, TooltipKind},
};
impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) async fn describe_unique(&mut self, expected: ExpectedObservation) -> Result<(), BackendError> {
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        if self.state.expected_observation().as_ref() != Ok(&expected) {
            return Err(BackendError::blocked(ErrorKind::InvalidEdit, "The item changed before its descriptions could be read."));
        }
        if matches!(&self.state.forge, ForgeState::Item { unique: Some(_), .. }) {
            return Ok(());
        }
        let target = expected.process().clone();
        self.admit(&target)?;
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
            .describe_unique(self.clock.now(), expected.snapshot())
            .await;
        self.superseded(epoch)?;
        self.admit(&target)?;
        let result = match result {
            Ok(Some(lines)) => {
                if let Err(error) = BoundTooltip::new(&expected, TooltipKind::Unique, lines.clone()) {
                    let error = BackendError::blocked(ErrorKind::Protocol, error.to_string());
                    self.fail(error.clone(), Some(target));
                    return Err(error);
                }
                Ok(lines)
            }
            Ok(None) => Err("Localized descriptions are unavailable.".into()),
            Err(error) => {
                self.handle_connection_gate(&error);
                if !matches!(error.kind, ErrorKind::Gate | ErrorKind::HelperDeparted | ErrorKind::CharacterLoading) {
                    self.fail(error.clone(), Some(target));
                }
                return Err(error);
            }
        };
        self.commit(epoch, |this| {
            if this.state.expected_observation().as_ref() == Ok(&expected)
                && let ForgeState::Item { unique, .. } = &mut this.state.forge
            {
                *unique = Some(result);
            }
        })
    }
    pub(super) async fn preview_unique(&mut self, expected: ExpectedObservation, draft: RollEdit) -> Result<(), BackendError> {
        let epoch = self.command_epoch;
        self.superseded(epoch)?;
        if self.state.expected_observation().as_ref() != Ok(&expected) {
            return Err(BackendError::blocked(ErrorKind::InvalidEdit, "The item changed before its preview could be read."));
        }
        if self.state.unique_preview.as_ref().is_some_and(|p| p.expected == expected && p.draft == draft) {
            return Ok(());
        }
        let evidence = match &self.state.forge {
            ForgeState::Item { unique: Some(Ok(lines)), .. } => BoundTooltip::new(&expected, TooltipKind::Unique, lines.clone()),
            _ => return Err(BackendError::blocked(ErrorKind::MappingUnavailable, "Unique descriptions are unavailable.")),
        }
        .map_err(|e| BackendError::blocked(ErrorKind::MappingUnavailable, e.to_string()))?;
        let target = expected.process().clone();
        self.admit(&target)?;
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| BackendError::blocked(ErrorKind::NotConnected, "connection released"))?
            .preview_unique(self.clock.now(), &expected, &draft, &evidence)
            .await;
        self.superseded(epoch)?;
        self.admit(&target)?;
        let result = match result {
            Ok(preview) => {
                if let Err(error) = preview.recheck(&expected, &draft) {
                    let error = BackendError::blocked(ErrorKind::Protocol, error.to_string());
                    self.fail(error.clone(), Some(target));
                    return Err(error);
                }
                Ok(preview)
            }
            Err(error)
                if matches!(error.kind, ErrorKind::MappingUnavailable | ErrorKind::MappingRetryable) && self.connection.as_ref().is_some_and(BackendConnection::is_ready) =>
            {
                Err(error.message)
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
                this.state.unique_preview = Some(ResolvedPreview { expected, draft, result });
            }
        })
    }
}
