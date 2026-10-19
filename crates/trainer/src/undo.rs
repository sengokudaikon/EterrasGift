//! Conditional undo intent for one confirmed status change.
//!
//! This is narrower than the C# EditWorkflow's reversed confirmed batches and
//! draft handling. A token performs no I/O, consumes no history, and grants no
//! write permission. Its owner must separately obtain fresh runtime observations,
//! enforce offline/process/generation admission, and explicitly submit the plan.

use crate::{
    catalog::GameCatalog,
    edits::{CatalogProvenance, EditError, ExpectedObservation},
    writes::{StatusPlan, WriteRecord, WriteStatus},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusUndo {
    after: ExpectedObservation,
    manager: u64,
    restore_value: i32,
    field: crate::edits::StatusField,
}

impl StatusUndo {
    /// Capture only an exact confirmed projection. The record's status is a
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
        let field = record.plan.field();
        let restore_value = field.value(before.snapshot());
        // Every supported game's literal range is byte-valued. An impossible
        // previous value is unavailable for undo, with no implicit override.
        if u8::try_from(restore_value).is_err() {
            return Err(EditError::InvalidStatus);
        }
        Ok(Self {
            after: ExpectedObservation::new(before.generation(), before.process().clone(), before.catalog_identity().clone(), snapshot.clone())?,
            manager: record.plan.manager(),
            restore_value,
            field,
        })
    }

    pub fn after(&self) -> &ExpectedObservation {
        &self.after
    }

    pub fn manager(&self) -> u64 {
        self.manager
    }

    pub fn restore_value(&self) -> i32 {
        self.restore_value
    }

    /// Build a new conditional write from the exact confirmed after-state.
    /// Full equality includes fields omitted from the legacy fingerprint. A
    /// cache or an older generation cannot establish fresh catalog provenance.
    /// Failure leaves this pure token unchanged and never retries a submission.
    pub fn prepare(&self, current: &ExpectedObservation, manager: u64, catalog: &GameCatalog, provenance: &CatalogProvenance) -> Result<StatusPlan, EditError> {
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
        // exact catalog identity and fresh RuntimeRead provenance. No override.
        StatusPlan::new(current, manager, catalog, provenance, self.field, self.restore_value)
    }
}
