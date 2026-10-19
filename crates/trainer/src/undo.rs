//! Conditional undo intent for one confirmed Forge edit.
//!
//! This is narrower than the C# EditWorkflow's reversed confirmed batches and
//! draft handling. A token performs no I/O, consumes no history, and grants no
//! write permission. Its owner must separately obtain fresh runtime observations,
//! enforce offline/process/generation admission, and explicitly submit the plan.

use crate::{
    catalog::GameCatalog,
    edits::{BaseTypeEdit, CatalogProvenance, EditError, ExpectedObservation, ItemEdit, RollChange, RollEdit, StatusEdit, UniqueIdentityEdit},
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
            ItemEdit::AddAffix(_) => {
                let occupied = record.plan.moves().values().copied().collect::<std::collections::BTreeSet<_>>();
                let added = snapshot.affixes.iter().filter(|a| !occupied.contains(&a.slot)).collect::<Vec<_>>();
                let [added] = added.as_slice() else {
                    return Err(EditError::InvalidObservation);
                };
                ItemEdit::RemoveAffix(crate::edits::RemoveAffixEdit {
                    slot: i32::from(added.slot),
                    id: i32::from(added.id),
                })
            }
            ItemEdit::RemoveAffix(edit) => {
                let removed = &item.affixes[edit.slot as usize];
                if removed.id > 4095 {
                    return Err(EditError::InvalidValues);
                }
                ItemEdit::AddAffix(crate::edits::AddAffixEdit {
                    id: i32::from(removed.id),
                    tier: i32::from(removed.tier),
                    roll: i32::from(removed.roll),
                    seal: match removed.seal {
                        0 => crate::edits::SealState::Normal,
                        1 => crate::edits::SealState::Sealed,
                        2 => crate::edits::SealState::Primordial,
                        3 => crate::edits::SealState::Corrupted,
                        _ => return Err(EditError::InvalidValues),
                    },
                    kind: match removed.kind {
                        0 => crate::edits::AffixKind::Prefix,
                        1 => crate::edits::AffixKind::Suffix,
                        _ => return Err(EditError::InvalidValues),
                    },
                    category: Some(removed.special),
                    allow_incompatible: false,
                })
            }
            ItemEdit::Affix(edit) => {
                let old = &item.affixes[edit.slot as usize];
                ItemEdit::Affix(crate::edits::AffixEdit {
                    slot: i32::from(*record.plan.moves().get(&(edit.slot as u8)).ok_or(EditError::InvalidValues)?),
                    id: i32::from(old.id),
                    tier: i32::from(old.tier),
                    roll: i32::from(old.roll),
                    seal: match old.seal {
                        0 => crate::edits::SealState::Normal,
                        1 => crate::edits::SealState::Sealed,
                        2 => crate::edits::SealState::Primordial,
                        3 => crate::edits::SealState::Corrupted,
                        _ => return Err(EditError::InvalidValues),
                    },
                    allow_incompatible: edit.allow_incompatible,
                })
            }
            ItemEdit::Roll(edit) if edit.target == crate::edits::RollTarget::Affix => ItemEdit::Roll(RollEdit {
                changes: edit
                    .changes
                    .iter()
                    .map(|c| RollChange {
                        index: c.index,
                        value: i32::from(item.affixes[c.index as usize].roll),
                    })
                    .collect(),
                ..edit.clone()
            }),
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
        let mut reverse = self.reverse.clone();
        if let ItemEdit::AddAffix(add) = &mut reverse {
            let definition = catalog.affixes.get(&(add.id as u16)).ok_or(EditError::MissingDefinition)?;
            let compatible = crate::edits::compatibility(catalog, current.snapshot(), definition, None, None, false).allowed;
            let (prefixes, suffixes) = crate::edits::free_slots(current.snapshot());
            let free = add.seal != crate::edits::SealState::Normal || (if add.kind == crate::edits::AffixKind::Prefix { prefixes } else { suffixes }) > 0;
            add.allow_incompatible = !(compatible && free);
        }
        ForgePlan::edit(current, manager, catalog, provenance, reverse, override_enabled)
    }
}
