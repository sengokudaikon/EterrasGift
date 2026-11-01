//! What the rows may do now.

use super::*;

impl ForgeEditor {
    pub(super) fn can_change_draft(&self) -> bool {
        !self.writes.is_running() && self.game.can_write()
    }

    /// Drafts can change: the session is ready, nothing is being written and no identity is drafted (a new unique brings
    /// its own modifiers, so it is drafted alone).
    pub fn can_mutate(&self) -> bool {
        self.can_change_draft() && !self.is_identity_drafted()
    }

    pub(super) fn has_other_drafts(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_dirty)
            || self.uniques.iter().any(RollRow::is_dirty)
            || self.implicits.iter().any(RollRow::is_dirty)
            || self.fields.iter().chain(self.base.iter()).any(FieldDraft::is_dirty)
    }

    pub(super) fn fields_mut(&mut self) -> impl Iterator<Item = &mut FieldDraft> {
        self.fields.iter_mut().chain(self.base.iter_mut()).chain(self.identity.iter_mut())
    }

    pub(super) fn unrestricted(&self) -> bool {
        self.game.unrestricted()
    }

    pub(super) fn has_pending_drafts(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_dirty)
            || self.uniques.iter().any(RollRow::is_dirty)
            || self.implicits.iter().any(RollRow::is_dirty)
            || self.all_fields().any(FieldDraft::is_dirty)
    }

    pub(super) fn has_implicit_drafts(&self) -> bool {
        self.implicits.iter().any(|row| row.is_dirty() || row.can_undo())
    }

    pub(super) fn all_fields(&self) -> impl Iterator<Item = &FieldDraft> {
        self.fields.iter().chain(self.base.iter()).chain(self.identity.iter())
    }
}
