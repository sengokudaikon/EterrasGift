use super::*;
use crate::{
    item::edit::{ExpectedObservation, ItemEdit, RollTarget},
    item::values::{CapturedDisplayParser, MappingBuilder, MappingError, MappingRequest, ValueTable},
};
use eg_trainer_wire::mapping::{self, AffixMappingDraft, AffixMappingQuery, MappingStart, MappingTarget};

impl<T: Transport> Session<T> {
    /// Samples the game's exact display on detached items. Each page checks the
    /// full selected item/manager; all pages must have one captured locale and
    /// opaque context before a table is exposed. This never records a write.
    pub async fn resolve_mapping(&mut self, expected: &ExpectedObservation, request: &MappingRequest) -> Result<ValueTable, SessionError> {
        self.resolve_mapping_observed(expected, request, &mut ContinueRead).await
    }
    /// Stop only before or after a completed RPC; never abandon pipe I/O or its
    /// artifact lease when the editor moves to a different draft.
    pub async fn resolve_mapping_observed(
        &mut self,
        expected: &ExpectedObservation,
        request: &MappingRequest,
        observer: &mut dyn ReadObserver,
    ) -> Result<ValueTable, SessionError> {
        if observer.control() == ReadControl::Stop {
            return Err(SessionError::Busy);
        }
        self.admit()?;
        request.validate(expected, None).map_err(unavailable)?;
        if request.address.target != MappingTarget::Affix {
            return Err(SessionError::Mapping("This roll mapping is unavailable.".into()));
        }
        let identity = expected.catalog_identity();
        if !self.observation_matches(expected) {
            return Err(SessionError::Mapping("The mapping belongs to a different process or catalog.".into()));
        }
        let item = expected.snapshot();
        let draft = match request.draft.as_ref() {
            None => None,
            Some(ItemEdit::Affix(edit)) => Some(AffixMappingDraft {
                id: u16::try_from(edit.id).map_err(|_| unavailable(MappingError::InvalidQuery))?,
                tier: u8::try_from(edit.tier).map_err(|_| unavailable(MappingError::InvalidQuery))?,
                roll: u8::try_from(edit.roll).map_err(|_| unavailable(MappingError::InvalidQuery))?,
                seal: edit.seal as u8,
            }),
            // A roll only changes the sample cursor itself; it cannot change
            // another modifier's context without a corresponding native draft.
            Some(ItemEdit::Roll(edit)) if edit.target == RollTarget::Affix && edit.changes.len() == 1 && edit.changes[0].index == i32::from(request.address.index) => None,
            _ => return Err(unavailable(MappingError::InvalidQuery)),
        };
        let manager = self.offline_manager()?;
        let dispatcher = self.dispatcher()?;
        let query = |start| {
            AffixMappingQuery::new(dispatcher, manager, item.clone(), request.address.index, start, draft)
                .map(Request::MapRoll)
                .map_err(SessionError::Protocol)
        };
        let context = self.mapping_page(query(MappingStart::ContextOnly)?, observer).await?;
        let mut builder = MappingBuilder::new(expected.clone(), request.clone(), context, None).map_err(unavailable)?;
        let mut parser = CapturedDisplayParser::new(&identity.locale.language).map_err(SessionError::Mapping)?;
        while let Some(start) = builder.next_start() {
            let page = self.mapping_page(query(MappingStart::Samples(start))?, observer).await?;
            builder.push(expected, request, page, &mut parser).map_err(unavailable)?;
        }
        builder.finish().map_err(unavailable)
    }
    async fn mapping_page(&mut self, request: Request, observer: &mut dyn ReadObserver) -> Result<mapping::RawMappingPage, SessionError> {
        let Request::MapRoll(query) = &request else {
            return Err(SessionError::Protocol(ProtocolError::InvalidData));
        };
        let address = mapping::MappingAddress {
            target: MappingTarget::Affix,
            index: query.slot(),
            line: None,
        };
        let start = query.start();
        match self
            .read_owned_control(|_| request, |bytes| mapping::decode_mapping(bytes, address, start), Some(observer))
            .await
        {
            Err(SessionError::Helper {
                code: ResponseCode::Refused,
                message,
                ..
            }) if self.state == SessionState::Ready => Err(SessionError::Mapping(message)),
            Err(SessionError::Helper {
                code: ResponseCode::Failed | ResponseCode::NotReady | ResponseCode::StaleItem,
                message,
                ..
            }) if self.state == SessionState::Ready => Err(SessionError::MappingRetryable(message)),
            Ok(OwnedRead::Complete(page)) => Ok(page),
            Ok(OwnedRead::Stopped) => Err(SessionError::Busy),
            Err(error) => Err(error),
        }
    }
}
fn unavailable(error: MappingError) -> SessionError {
    match error {
        MappingError::WrongContext => SessionError::MappingRetryable(error.to_string()),
        error => SessionError::Mapping(error.to_string()),
    }
}
