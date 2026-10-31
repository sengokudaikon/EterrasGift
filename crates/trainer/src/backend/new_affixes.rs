//! Has the game hold the unique modifiers in play, before anything asks about them: the ones in use and the ones the
//! item holds, on every read, and a drafted one before its values are sampled or its write is sent.
use super::*;
use crate::definitions::unique_mods::{is_added, new_affix};
use crate::item::edit::ItemEdit;
use eg_trainer_wire::forge::ForgeSnapshot;

/// The added affix an edit would put on the item.
pub(super) fn edit_affix(edit: &ItemEdit) -> Option<u16> {
    let id = match edit {
        ItemEdit::Affix(edit) => edit.id,
        ItemEdit::AddAffix(edit) => edit.id,
        _ => return None,
    };
    u16::try_from(id).ok().filter(|id| is_added(*id))
}

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    /// Those in use and those this item holds.
    pub(super) async fn register_in_play(&mut self, epoch: u64, target: &ProcessIdentity, data: &GameCatalog, item: Option<&ForgeSnapshot>) -> Result<(), BackendError> {
        let held = item.into_iter().flat_map(|item| item.affixes.iter().map(|affix| affix.id)).filter(|id| is_added(*id));
        let ids: Vec<u16> = self.in_use.ids().chain(held).filter(|id| !self.registered.contains(id)).collect();
        self.register_added(epoch, target, data, ids).await
    }

    /// A refusal is written down once and not asked again on this connection: the item stays as it is and the game answers
    /// any later request for that ID itself. Only a session that can't go on is an error here.
    pub(super) async fn register_added(&mut self, epoch: u64, target: &ProcessIdentity, data: &GameCatalog, ids: Vec<u16>) -> Result<(), BackendError> {
        let wanted: Vec<_> = ids
            .into_iter()
            .filter(|id| !self.registered.contains(id))
            .filter_map(|id| data.affixes.get(&id))
            .filter_map(new_affix)
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let ids: Vec<u16> = wanted.iter().map(|affix| affix.id).collect();
        let result = self.call(epoch, target, async |connection| connection.register_affixes(&wanted).await).await?;
        match result {
            Ok(()) => {
                self.registered.extend(&ids);
                if let Err(problem) = self.in_use.extend(ids)
                    && let Some(faults) = &self.faults
                {
                    faults.write("Saving the unique modifiers in use", Some(&problem), None, None);
                }
                Ok(())
            }
            // The character is between areas: the next read asks again.
            Err(error) if matches!(error.kind, ErrorKind::CharacterLoading | ErrorKind::NotReady) => Ok(()),
            Err(error) if self.connection.as_ref().is_some_and(BackendConnection::is_ready) => {
                self.registered.extend(&ids);
                if let Some(faults) = &self.faults {
                    faults.write("Adding unique modifiers to the game's affixes", Some(&error.message), None, None);
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}
