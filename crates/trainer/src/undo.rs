//! Conditional undo intent for one confirmed Forge edit.
//!
//! This is narrower than the C# EditWorkflow's reversed confirmed batches and
//! draft handling. A token performs no I/O, consumes no history, and grants no
//! write permission. Its owner must separately obtain fresh runtime observations,
//! enforce offline/process/generation admission, and explicitly submit the plan.

use crate::{
    catalog::GameCatalog,
    edits::{BaseTypeEdit, CatalogProvenance, EditError, ExpectedObservation, ItemEdit, StatusEdit, UniqueIdentityEdit},
    writes::{ForgePlan, WriteRecord, WriteStatus},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeUndo {
    after: ExpectedObservation,
    manager: u64,
    reverse: ItemEdit,
}

impl ForgeUndo {
    /// Capture a confirmed result under the edit's readback policy. The record's status is a
    /// caller-supplied fact, not a certificate of transport/runtime execution.
    /// Current literal ranges are checked later against the fresh catalog; the
    /// original plan retains their identity but not their complete values.
    pub fn from_confirmed(record: &WriteRecord) -> Result<Self, EditError> {
        let WriteStatus::Confirmed(snapshot) = &record.status else {
            return Err(EditError::InvalidObservation);
        };
        if !record.plan.matches(snapshot) {
            return Err(EditError::InvalidObservation);
        }
        let before = record.plan.before();
        let item = before.snapshot();
        let reverse = match record.plan.edit_intent() {
            ItemEdit::Status(edit) => {
                let value = edit.field.value(item);
                if u8::try_from(value).is_err() {
                    return Err(EditError::InvalidStatus);
                }
                ItemEdit::Status(StatusEdit { value, ..edit.clone() })
            }
            ItemEdit::BaseType(edit) => ItemEdit::BaseType(BaseTypeEdit {
                sub_type: i32::from(item.sub_type),
                allow_incompatible: edit.allow_incompatible,
                item_type: edit.item_type.map(|_| i32::from(item.item_type)),
            }),
            ItemEdit::UniqueIdentity(edit) => ItemEdit::UniqueIdentity(UniqueIdentityEdit {
                unique_id: i32::from(item.identity.as_ref().ok_or(EditError::InvalidUniqueIdentity)?.id),
                sub_type: i32::from(item.sub_type),
                rarity: i32::from(item.rarity),
                unrestricted: edit.unrestricted,
            }),
            _ => return Err(EditError::InvalidValues),
        };
        Ok(Self {
            after: ExpectedObservation::new(before.generation(), before.process().clone(), before.catalog_identity().clone(), snapshot.clone())?,
            manager: record.plan.manager(),
            reverse,
        })
    }

    pub fn after(&self) -> &ExpectedObservation {
        &self.after
    }

    pub fn manager(&self) -> u64 {
        self.manager
    }

    pub fn reverse_edit(&self) -> &ItemEdit {
        &self.reverse
    }

    /// Build a new conditional write from the exact confirmed after-state.
    /// Full equality includes fields omitted from the legacy fingerprint. A
    /// cache or an older generation cannot establish fresh catalog provenance.
    /// Failure leaves this pure token unchanged and never retries a submission.
    pub fn prepare(&self, current: &ExpectedObservation, manager: u64, catalog: &GameCatalog, provenance: &CatalogProvenance) -> Result<ForgePlan, EditError> {
        self.prepare_with_override(current, manager, catalog, provenance, false)
    }
    pub fn prepare_with_override(
        &self,
        current: &ExpectedObservation,
        manager: u64,
        catalog: &GameCatalog,
        provenance: &CatalogProvenance,
        override_enabled: bool,
    ) -> Result<ForgePlan, EditError> {
        if manager != self.manager
            || current.generation() != self.after.generation()
            || current.process() != self.after.process()
            || current.catalog_identity() != self.after.catalog_identity()
        {
            return Err(EditError::IdentityChanged);
        }
        if current.snapshot() != self.after.snapshot() {
            return Err(EditError::StaleItem);
        }
        // This rechecks actual current literal limits, field eligibility,
        // exact catalog identity and fresh RuntimeRead provenance. Retain the
        // original edit's flag and recheck the current override permission.
        ForgePlan::edit(current, manager, catalog, provenance, self.reverse.clone(), override_enabled)
    }
}
