use super::*;
use crate::{
    item::edit::{ExpectedObservation, RollEdit},
    item::values::{BoundPreview, BoundTooltip},
};
use eg_trainer_wire::mapping::{ForgeDescriptionRequest, ModifierSnapshot, UniquePreviewRequest, UniqueRollChange};
impl<T: Transport> Session<T> {
    pub async fn describe_unique(&mut self, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, SessionError> {
        if !matches!(expected.rarity, 7 | 9) {
            return Ok(None);
        }
        let manager = self.offline_manager()?;
        let dispatcher = self.dispatcher()?;
        let query = ForgeDescriptionRequest::new(dispatcher, manager, expected.clone()).map_err(SessionError::Protocol)?;
        match self.read_owned(|_| Request::DescribeUnique(query), eg_trainer_wire::mapping::decode_modifiers).await {
            Ok(modifiers) => {
                let count = expected.identity.as_ref().map_or(0, |i| i.rolls.len());
                if modifiers.iter().any(|m| m.roll_ids.iter().any(|id| usize::from(*id) >= count)) {
                    self.release_without_io();
                    self.state = SessionState::Disconnected;
                    return Err(SessionError::Protocol(ProtocolError::InvalidData));
                }
                Ok(Some(modifiers))
            }
            Err(SessionError::Helper { code, .. }) if code.spares_presentation_session() && self.state == SessionState::Ready => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Optional formatting of an exact unique-roll draft; never records a write.
    pub async fn preview_unique(&mut self, expected: &ExpectedObservation, draft: &RollEdit, evidence: &BoundTooltip) -> Result<BoundPreview, SessionError> {
        if !self.observation_matches(expected) {
            return Err(SessionError::Mapping("The preview belongs to a different process or catalog.".into()));
        }
        let prepared = BoundPreview::prepare(expected, draft.clone(), evidence).map_err(|e| SessionError::Mapping(e.to_string()))?;
        let manager = self.offline_manager()?;
        let dispatcher = self.dispatcher()?;
        let changes = draft
            .changes
            .iter()
            .map(|c| {
                Ok(UniqueRollChange {
                    index: u8::try_from(c.index).map_err(|_| SessionError::Mapping("Unsupported unique roll index.".into()))?,
                    value: u8::try_from(c.value).map_err(|_| SessionError::Mapping("Unsupported unique roll value.".into()))?,
                })
            })
            .collect::<Result<Vec<_>, SessionError>>()?;
        let query = UniquePreviewRequest::new(dispatcher, manager, expected.snapshot().clone(), changes).map_err(SessionError::Protocol)?;
        match self.read_owned(|_| Request::PreviewUnique(query), eg_trainer_wire::mapping::decode_modifiers).await {
            Ok(modifiers) => match prepared.with_reply(eg_trainer_wire::mapping::UniqueRollPreview { modifiers }) {
                Ok(preview) => Ok(preview),
                Err(_) => {
                    self.release_without_io();
                    self.state = SessionState::Disconnected;
                    Err(SessionError::Protocol(ProtocolError::InvalidData))
                }
            },
            Err(SessionError::Helper { code, message, .. }) if code.spares_presentation_session() && self.state == SessionState::Ready => Err(SessionError::Mapping(message)),
            Err(error) => Err(error),
        }
    }
}
