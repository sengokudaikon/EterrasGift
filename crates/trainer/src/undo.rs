//! Conditional Undo tokens and the confirmed steps of one mixed Forge batch.
//!
//! Draft handling stays in the editor. A token performs no I/O and grants no
//! write permission. Its owner must separately obtain fresh runtime observations,
//! enforce offline/process/generation admission, and explicitly submit the plan.

use crate::{
    catalog::GameCatalog,
    edits::{BaseTypeEdit, EditError, ExpectedObservation, ItemEdit, RollChange, RollEdit, StatusEdit, UniqueIdentityEdit},
    writes::{ForgePlan, WriteRecord, WriteStatus},
};

/// One Apply's confirmed steps. Draft ordering stays with the editor; all
/// reversal and conditional restoration rules live here.
#[derive(Clone, Debug, Default)]
pub struct ForgeUndoHistory {
    steps: Vec<ForgeUndo>,
    begin: bool,
}
impl ForgeUndoHistory {
    pub fn begin_batch(&mut self) {
        self.begin = true;
    }
    pub fn clear(&mut self) {
        self.steps.clear();
        self.begin = false;
    }
    pub fn current(&self) -> Option<&ForgeUndo> {
        self.steps.last()
    }
    pub fn len(&self) -> usize {
        self.steps.len()
    }
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
    pub fn needs_override(&self) -> bool {
        self.steps.iter().any(|u| u.reverse.needs_override())
    }
    /// Called only for a confirmed step. Refusal retains the stack; NotKept or
    /// Uncertain must clear it. A new batch replaces old Undo on its first confirmation.
    pub fn confirmed(&mut self, prepared: &crate::edits::PreparedEdit, manager: u64, actual: &eg_trainer_wire::forge::ForgeSnapshot, undoing: bool) -> Result<(), EditError> {
        if !prepared.inspect_readback(Some(actual)).confirmed() {
            self.clear();
            return Err(EditError::InvalidObservation);
        }
        if undoing {
            let current = self.steps.pop().ok_or(EditError::StaleItem)?;
            if current.after != *prepared.before() || current.manager != manager {
                self.clear();
                return Err(EditError::StaleItem);
            }
            if let Some(previous) = self.steps.last_mut() {
                let mut restored = actual.clone();
                // Rebuilds may refresh names and normalize affix order. Match
                // every saved affix exactly (apart from slot/name), once each,
                // then carry the older intent's slot references through that map.
                // All other item fields must equal the prior confirmed state.
                restored.name.clone_from(&previous.after.snapshot().name);
                restored.fingerprint = previous.after.snapshot().fingerprint;
                let mut used = std::collections::BTreeSet::new();
                let mut moves = std::collections::BTreeMap::new();
                let mut ordered = Vec::new();
                for saved in &previous.after.snapshot().affixes {
                    let found = actual.affixes.iter().find_map(|candidate| {
                        if used.contains(&candidate.slot) {
                            return None;
                        }
                        let mut normalized = candidate.clone();
                        normalized.slot = saved.slot;
                        normalized.name.clone_from(&saved.name);
                        (normalized == *saved).then_some((candidate.slot, normalized))
                    });
                    let Some((slot, normalized)) = found else {
                        self.clear();
                        return Err(EditError::StaleItem);
                    };
                    used.insert(slot);
                    moves.insert(saved.slot, slot);
                    ordered.push(normalized);
                }
                if actual.affixes.len() != ordered.len() {
                    self.clear();
                    return Err(EditError::StaleItem);
                }
                restored.affixes = ordered;
                if restored != *previous.after.snapshot() {
                    self.clear();
                    return Err(EditError::StaleItem);
                }
                previous.after = ExpectedObservation::new(
                    prepared.before().generation(),
                    prepared.before().process().clone(),
                    prepared.before().catalog_identity().clone(),
                    actual.clone(),
                )?;
                previous.reverse = previous.reverse.follow(&moves);
            }
        } else {
            if self.begin || self.steps.last().is_some_and(|u| u.after != *prepared.before() || u.manager != manager) {
                self.steps.clear();
            }
            self.begin = false;
            match ForgeUndo::from_checked(prepared, manager, actual) {
                Ok(undo) => self.steps.push(undo),
                Err(error) => {
                    self.clear();
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeUndo {
    after: ExpectedObservation,
    manager: u64,
    reverse: ItemEdit,
}

impl ForgeUndo {
    /// Validate the retained readback and derive a conditional reverse edit.
    /// Current literal ranges are checked later against the fresh catalog; the
    /// original plan retains their identity but not their complete values.
    pub fn from_confirmed(record: &WriteRecord) -> Result<Self, EditError> {
        let WriteStatus::Confirmed(snapshot) = &record.status else {
            return Err(EditError::InvalidObservation);
        };
        Self::from_prepared(record.plan.prepared(), record.plan.manager(), snapshot)
    }

    /// Pure reversal from a completed, independently confirmed observation.
    /// Authored sessions use this same domain path as production.
    pub fn from_prepared(prepared: &crate::edits::PreparedEdit, manager: u64, snapshot: &eg_trainer_wire::forge::ForgeSnapshot) -> Result<Self, EditError> {
        if manager == 0 || !prepared.inspect_readback(Some(snapshot)).confirmed() {
            return Err(EditError::InvalidObservation);
        }
        Self::from_checked(prepared, manager, snapshot)
    }
    fn from_checked(prepared: &crate::edits::PreparedEdit, manager: u64, snapshot: &eg_trainer_wire::forge::ForgeSnapshot) -> Result<Self, EditError> {
        let reverse = Self::reverse_checked(prepared, snapshot)?;
        let before = prepared.before();
        Ok(Self {
            after: ExpectedObservation::new(before.generation(), before.process().clone(), before.catalog_identity().clone(), snapshot.clone())?,
            manager,
            reverse,
        })
    }

    /// Pure intent calculation for draft cautions; it creates no accepted history.
    pub fn reverse_intent(prepared: &crate::edits::PreparedEdit, snapshot: &eg_trainer_wire::forge::ForgeSnapshot) -> Result<ItemEdit, EditError> {
        if !prepared.inspect_readback(Some(snapshot)).confirmed() {
            return Err(EditError::InvalidObservation);
        }
        Self::reverse_checked(prepared, snapshot)
    }
    fn reverse_checked(prepared: &crate::edits::PreparedEdit, snapshot: &eg_trainer_wire::forge::ForgeSnapshot) -> Result<ItemEdit, EditError> {
        let before = prepared.before();
        let item = before.snapshot();
        let reverse = match prepared.edit() {
            ItemEdit::AddAffix(_) => {
                let occupied = prepared.moves().values().copied().collect::<std::collections::BTreeSet<_>>();
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
                    slot: i32::from(*prepared.moves().get(&(edit.slot as u8)).ok_or(EditError::InvalidValues)?),
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
            ItemEdit::Roll(edit) => ItemEdit::Roll(RollEdit {
                changes: edit
                    .changes
                    .iter()
                    .map(|c| {
                        Ok(RollChange {
                            index: c.index,
                            value: i32::from(match edit.target {
                                crate::edits::RollTarget::Affix => item.affixes[c.index as usize].roll,
                                crate::edits::RollTarget::Unique => item.identity.as_ref().ok_or(EditError::InvalidRollMapping)?.rolls[c.index as usize],
                                crate::edits::RollTarget::Implicit => item.implicit_rolls[c.index as usize],
                            }),
                        })
                    })
                    .collect::<Result<_, EditError>>()?,
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
        };
        Ok(reverse)
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
    /// changed process, generation or catalog invalidates the token.
    /// Failure leaves this pure token unchanged and never retries a submission.
    pub fn prepare(&self, current: &ExpectedObservation, manager: u64, catalog: &GameCatalog) -> Result<ForgePlan, EditError> {
        self.prepare_with_override(current, manager, catalog, false)
    }
    pub fn prepare_with_override(&self, current: &ExpectedObservation, manager: u64, catalog: &GameCatalog, override_enabled: bool) -> Result<ForgePlan, EditError> {
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
        // exact catalog identity. Retain the
        // original edit's flag and recheck the current override permission.
        let mut reverse = self.reverse.clone();
        if let ItemEdit::AddAffix(add) = &mut reverse {
            let definition = catalog.affixes.get(&(add.id as u16)).ok_or(EditError::MissingDefinition)?;
            let compatible = crate::edits::compatibility(catalog, current.snapshot(), definition, None, None, false).allowed;
            let (prefixes, suffixes) = crate::edits::free_slots(current.snapshot());
            let free = add.seal != crate::edits::SealState::Normal || (if add.kind == crate::edits::AffixKind::Prefix { prefixes } else { suffixes }) > 0;
            add.allow_incompatible = !(compatible && free);
        }
        ForgePlan::edit(current, manager, catalog, reverse, override_enabled)
    }
}
