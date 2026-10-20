use super::*;
use crate::{
    edits::{ExpectedObservation, RollEdit, RollTarget},
    mapping::{BoundPreview, BoundTooltip},
};
use eg_game_access::mapping::{ForgeDescriptionRequest, ModifierSnapshot, UniquePreviewRequest, UniqueRollChange};
impl<T: Transport, L: LogSource> Session<T, L> {
    pub async fn describe_unique(&mut self, now: Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, SessionError> {
        if !matches!(expected.rarity, 7 | 9) {
            return Ok(None);
        }
        let manager = self.status.filter(|s| !s.online && s.manager != 0).ok_or(SessionError::NotConnected)?.manager;
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
        let query = ForgeDescriptionRequest::new(dispatcher, manager, expected.clone()).map_err(SessionError::Protocol)?;
        match self.read_owned(now, |_| Request::DescribeUnique(query), eg_game_access::mapping::decode_modifiers).await {
            Ok(modifiers) => {
                let count = expected.identity.as_ref().map_or(0, |i| i.rolls.len());
                if modifiers.iter().any(|m| m.roll_ids.iter().any(|id| usize::from(*id) >= count)) {
                    self.release_without_io();
                    self.state = SessionState::Blocked;
                    return Err(SessionError::Protocol(ProtocolError::InvalidData));
                }
                Ok(Some(modifiers))
            }
            Err(SessionError::Helper {
                code: ResponseCode::Refused | ResponseCode::Failed | ResponseCode::NotReady | ResponseCode::StaleItem,
                ..
            }) if self.state == SessionState::Ready => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Optional formatting of an exact unique-roll draft; never records a write.
    pub async fn preview_unique(&mut self, now: Timestamp, expected: &ExpectedObservation, draft: &RollEdit, evidence: &BoundTooltip) -> Result<BoundPreview, SessionError> {
        evidence.recheck(expected).map_err(|e| SessionError::Mapping(e.to_string()))?;
        if draft.target != RollTarget::Unique {
            return Err(SessionError::Mapping("Only unique rolls have a tooltip preview.".into()));
        }
        let identity = expected.catalog_identity();
        if expected.process() != &self.target
            || !identity.binary_hash.eq_ignore_ascii_case(self.profile.binary_hash())
            || !identity.metadata_hash.eq_ignore_ascii_case(self.profile.metadata_hash())
        {
            return Err(SessionError::Mapping("The preview belongs to a different process or catalog.".into()));
        }
        // Reuse the pure preview validator before entering transport.
        BoundPreview::new(expected, draft.clone(), eg_game_access::mapping::UniqueRollPreview { modifiers: vec![] }, evidence).map_err(|e| SessionError::Mapping(e.to_string()))?;
        let manager = self.status.filter(|s| !s.online && s.manager != 0).ok_or(SessionError::NotConnected)?.manager;
        let dispatcher = self.discovery.as_ref().ok_or(SessionError::NotConnected)?.dispatcher;
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
        match self.read_owned(now, |_| Request::PreviewUnique(query), eg_game_access::mapping::decode_modifiers).await {
            Ok(modifiers) => match BoundPreview::new(expected, draft.clone(), eg_game_access::mapping::UniqueRollPreview { modifiers }, evidence) {
                Ok(preview) => Ok(preview),
                Err(_) => {
                    self.release_without_io();
                    self.state = SessionState::Blocked;
                    Err(SessionError::Protocol(ProtocolError::InvalidData))
                }
            },
            Err(SessionError::Helper {
                code: ResponseCode::Refused | ResponseCode::Failed | ResponseCode::NotReady | ResponseCode::StaleItem,
                message,
                ..
            }) if self.state == SessionState::Ready => Err(SessionError::Mapping(message)),
            Err(error) => Err(error),
        }
    }
}
