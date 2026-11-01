//! Apply and Undo: the plan a set of drafts makes, and the batch of writes that carries it out.

use super::*;

impl ForgeEditor {
    /// Submit one command. Only a separate, typed completion advances a draft.
    pub(super) fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        if !self.game.can_write() {
            phrases! { NotReady: "The session is not ready to write." }
            return refused(NotReady);
        }
        if edit.needs_override() && !self.unrestricted() {
            return refused(tr::OverrideRequired);
        }
        if !self.game.supports(edit) {
            phrases! { Unsupported: "This edit is not available in this build. Nothing was applied." }
            return refused(Unsupported);
        }
        self.game.write(expected, edit)
    }

    /// The item as Apply would leave it, or the game's while the drafts cannot all be applied.
    pub fn drafted_item(&self) -> Option<ForgeItem> {
        let item = self.item.as_ref()?;
        Some(self.project(item).map_or_else(|_| item.clone(), |(_, projected)| projected))
    }

    pub(super) fn plan(&self, current: &ForgeItem) -> Result<Vec<Step>, String> {
        self.project(current).map(|(steps, _)| steps)
    }

    /// The writes Apply makes, in order, with the item they leave, or why none may be made: removals first, so additions
    /// find the slots they free, then the affixes
    /// kept, additions, unique modifiers, implicits and the item fields, the level requirement last. Each step names the
    /// slots the earlier ones leave and is checked against the item they leave.
    pub(super) fn project(&self, current: &ForgeItem) -> Result<(Vec<Step>, ForgeItem), String> {
        let removed: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.is_removed()).collect();
        let rows: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.is_dirty() && !row.added && !row.is_removed()).collect();
        let added: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.added).collect();
        let uniques: Vec<&RollRow> = self.uniques.iter().filter(|row| row.is_dirty()).collect();
        let implicits: Vec<&RollRow> = self.implicits.iter().filter(|row| row.is_dirty()).collect();
        phrases! {
            Stale: "{name} changed in the game after you drafted it. Discard that draft and edit it again. Nothing was applied."
            StaleUniques: "Unique modifier rolls changed in the game after you drafted them. Discard those drafts and edit them again. Nothing was applied."
            StaleImplicits: "Implicit rolls changed in the game after you drafted them. Discard those drafts and edit them again. Nothing was applied."
        }
        if let Some(stale) = removed.iter().chain(&rows).find(|row| row.is_stale()) {
            return Err(Stale.with(&[("name", &stale.name())]));
        }
        if uniques.iter().any(|row| row.is_stale()) {
            return Err(StaleUniques.into());
        }
        if implicits.iter().any(|row| row.is_stale()) {
            return Err(StaleImplicits.into());
        }
        let mut fields: Vec<&FieldDraft> = self.all_fields().filter(|field| field.is_dirty()).collect();
        fields.sort_by_key(|field| field.field == Some(StatusField::LevelRequirement));
        if let Some(stale) = fields.iter().find(|field| field.is_stale()) {
            return Err(Stale.with(&[("name", &stale.name)]));
        }
        let several = removed.len() + rows.len() + added.len() + uniques.len() + implicits.len() + fields.len() > 1;
        let refused = |problem: String| {
            if several && !problem.ends_with(tr::NothingApplied.get()) {
                tr::AndNothingApplied.with(&[("problem", &problem)])
            } else {
                problem
            }
        };
        let named = |row: &AffixRow, problem: String| {
            refused(if several {
                tr::NamedProblem.with(&[("name", &row.name()), ("problem", &problem)])
            } else {
                problem
            })
        };
        let mut steps = Vec::new();
        let mut projected = current.clone();
        let mut at: Moves = current.affixes.iter().map(|affix| (affix.slot, affix.slot)).collect();
        let add = |steps: &mut Vec<Step>, projected: &mut ForgeItem, at: &mut Moves, step: Step| {
            *at = item::then(at, &step.edit.moves(projected));
            *projected = step.edit.project(projected).map_err(|e| e.to_string())?;
            steps.push(step);
            Ok::<(), String>(())
        };
        for row in removed {
            let edit = ItemEdit::RemoveAffix(crate::item::edit::RemoveAffixEdit {
                slot: *at.get(&row.slot()).unwrap_or(&row.slot()) as i32,
                id: i32::from(row.state.id),
                restore_potential: None,
            });
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: row.name(),
                    edit,
                    row: StepRow::Affix(row.uid),
                },
            )
            .map_err(|problem| named(row, problem))?;
        }
        for row in rows {
            let slot = *at.get(&row.slot()).unwrap_or(&row.slot());
            let edit = self.read_affix_edit(row, &projected, slot).map_err(|problem| named(row, problem))?;
            let Some(previous) = projected.affix(slot) else {
                phrases! { Moved: "That affix is no longer in that slot." }
                return Err(refused(Moved.into()));
            };
            let edit = match edit {
                ItemEdit::Affix(crate::item::edit::AffixEdit { slot, id, tier, roll, seal, .. })
                    if (i32::from(previous.id), i32::from(previous.tier), previous.seal) == (id, tier, seal) =>
                {
                    ItemEdit::Roll(crate::item::edit::RollEdit {
                        target: RollTarget::Affix,
                        changes: vec![crate::item::edit::RollChange { index: slot, value: roll }],
                        allow_incompatible: false,
                    })
                }
                edit => edit,
            };
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: row.name(),
                    edit,
                    row: StepRow::Affix(row.uid),
                },
            )?;
        }
        for row in added {
            let edit = self
                .addition(&projected, row.definition(), row.tier(), row.roll(), row.seal(), row.allows_incompatible())
                .map_err(|problem| named(row, problem))?;
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: row.name(),
                    edit,
                    row: StepRow::Affix(row.uid),
                },
            )
            .map_err(|problem| named(row, problem))?;
        }
        if !uniques.is_empty() {
            match self.unique_edit() {
                Some(edit) if edit.is_valid(&projected) => {
                    let rows = uniques.iter().map(|row| row.uid).collect();
                    add(
                        &mut steps,
                        &mut projected,
                        &mut at,
                        Step {
                            name: tr::UniqueModifiersName.into(),
                            edit,
                            row: StepRow::Uniques(rows),
                        },
                    )?;
                }
                _ => {
                    phrases! { UniquePositions: "Enter integer roll positions from 0 to 255 for mapped unique modifiers." }
                    return Err(refused(UniquePositions.into()));
                }
            }
        }
        if !implicits.is_empty() {
            let edit = ItemEdit::Roll(crate::item::edit::RollEdit {
                target: RollTarget::Implicit,
                changes: item::roll_changes(implicits.iter().flat_map(|row| row.changes())),
                allow_incompatible: self.unrestricted(),
            });
            if !edit.is_valid(&projected) {
                phrases! { ImplicitPositions: "Enter integer roll positions from 0 to 255 for implicits whose values roll." }
                return Err(refused(ImplicitPositions.into()));
            }
            let rows = implicits.iter().map(|row| row.uid).collect();
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: tr::ImplicitsName.into(),
                    edit,
                    row: StepRow::Implicits(rows),
                },
            )?;
        }
        for field in fields {
            let edit = match field.kind {
                FieldKind::Identity => self.identity_edit(&projected, field.value() as u16),
                _ => match field.read_edit() {
                    // "Allow incompatible affixes" on a base that breaks none needs no override.
                    ItemEdit::BaseType(
                        rebase @ crate::item::edit::BaseTypeEdit {
                            allow_incompatible: true,
                            item_type: None,
                            ..
                        },
                    ) if self.broken_by_base(&projected, projected.item_type, rebase.sub_type as u16) == 0 => ItemEdit::BaseType(crate::item::edit::BaseTypeEdit {
                        allow_incompatible: false,
                        ..rebase
                    }),
                    edit => edit,
                },
            };
            phrases! {
                SetHoldsNone: "A set item holds no affixes. Remove them first."
                UniqueMisfits: "That unique does not fit this item."
                StatusMisfits: "{name} does not apply to this item."
                FixedBase: "This item's base cannot change."
            }
            let problem = match &edit {
                edit if edit.needs_override() && !self.unrestricted() => Some(tr::OverrideRequired.into()),
                ItemEdit::UniqueIdentity(crate::item::edit::UniqueIdentityEdit { rarity: 8, .. }) if !projected.affixes.is_empty() => Some(SetHoldsNone.into()),
                ItemEdit::UniqueIdentity(crate::item::edit::UniqueIdentityEdit { .. }) if !edit.is_valid(&projected) => Some(UniqueMisfits.into()),
                ItemEdit::Status(crate::item::edit::StatusEdit { .. }) if !edit.is_valid(&projected) => Some(StatusMisfits.with(&[("name", &field.name)])),
                // Before the base's own check, which also refuses an equipment-type break without naming it.
                ItemEdit::BaseType(crate::item::edit::BaseTypeEdit {
                    allow_incompatible: false,
                    item_type: None,
                    sub_type,
                }) if self.broken_by_base(&projected, projected.item_type, *sub_type as u16) > 0 => Some(tr::BrokenByBase.into()),
                ItemEdit::BaseType(crate::item::edit::BaseTypeEdit { .. }) if !edit.is_valid(&projected) => Some(FixedBase.into()),
                _ => None,
            };
            if let Some(problem) = problem {
                return Err(refused(problem));
            }
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: field.name.clone(),
                    edit,
                    row: StepRow::Field(field.uid),
                },
            )?;
        }
        Ok((steps, projected))
    }

    /// A row's draft as an edit of the affix now in `slot`, checked against the item as the earlier drafts leave it.
    pub(super) fn read_affix_edit(&self, row: &AffixRow, item: &ForgeItem, slot: usize) -> Result<ItemEdit, String> {
        let definition = self.catalog().definition(row.draft_id());
        if !definition.writable {
            return Err(tr::Unwritable.into());
        }
        if row.tier() > definition.max_tier {
            phrases! { MaxTier: "This affix supports at most tier {tier}." }
            return Err(MaxTier.with(&[("tier", &definition.max_tier)]));
        }
        // The affix the item holds takes a new tier, seal or roll whatever its compatibility; only a replacement is checked.
        if row.draft_id() == row.state.id {
            let edit = row.read_edit(slot, false);
            edit.prepare_view(item).map_err(|e| e.to_string())?;
            return Ok(edit);
        }
        let allow = row.allows_incompatible();
        let compatibility = self.game.compatibility(item, &definition, item.affix(slot), self.unrestricted());
        if let Some(problem) = self.replacement_problem(compatibility, allow) {
            return Err(problem);
        }
        let edit = row.read_edit(slot, (allow && !compatibility.is_allowed()) || compatibility == Compatibility::Override);
        edit.prepare_view(item).map_err(|e| e.to_string())?;
        Ok(edit)
    }

    pub(super) fn step_summary(&self, row: &StepRow) -> (String, String) {
        match row {
            StepRow::Affix(uid) => self
                .affixes
                .iter()
                .find(|r| r.uid == *uid)
                .map(|r| (r.change_summary(), if r.is_removed() { String::new() } else { r.value.effects(r.roll()) }))
                .unwrap_or_default(),
            StepRow::Uniques(uids) => {
                let rows: Vec<&RollRow> = self.uniques.iter().filter(|r| uids.contains(&r.uid)).collect();
                (
                    rows.iter().map(|r| r.change_summary()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                    rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>().join("\n"),
                )
            }
            StepRow::Implicits(uids) => {
                let rows: Vec<&RollRow> = self.implicits.iter().filter(|r| uids.contains(&r.uid)).collect();
                (
                    rows.iter().map(|r| r.change_summary()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                    rows.iter().map(|r| r.effects()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                )
            }
            StepRow::Field(uid) => (self.all_fields().find(|f| f.uid == *uid).map(FieldDraft::change_summary).unwrap_or_default(), String::new()),
            StepRow::Undo => (String::new(), String::new()),
        }
    }

    /// A confirmed step's row goes back to following the game. An added row gives way to the game's row for the affix,
    /// which `complete_write` marks confirmed; a removed one went with its affix.
    pub(super) fn clear_applied(&mut self, row: &StepRow) {
        match row {
            StepRow::Undo => {}
            StepRow::Affix(uid) => {
                if let Some(index) = self.affixes.iter().position(|r| r.uid == *uid) {
                    if self.affixes[index].added {
                        self.drop_added(index);
                    } else {
                        let definition = self.catalog().definition(self.affixes[index].state.id);
                        self.affixes[index].reset(definition);
                        self.affixes[index].confirmed = true;
                    }
                }
                self.forget(RowRef::Affix(*uid));
            }
            StepRow::Uniques(uids) => {
                for row in self.uniques.iter_mut() {
                    row.reset();
                    row.confirmed = uids.contains(&row.uid);
                }
                self.draft_order.retain(|entry| !matches!(entry, RowRef::Unique(_)));
            }
            StepRow::Implicits(uids) => {
                for row in self.implicits.iter_mut() {
                    row.reset();
                    row.confirmed = uids.contains(&row.uid);
                }
                self.draft_order.retain(|entry| !matches!(entry, RowRef::Implicit(_)));
            }
            StepRow::Field(uid) => {
                if let Some(field) = self.fields_mut().find(|f| f.uid == *uid) {
                    field.reset();
                    field.confirmed = true;
                }
                self.forget(RowRef::Field(*uid));
            }
        }
    }

    /// Ctrl+S and the primary: writes every draft, each read back.
    pub fn apply(&mut self) {
        let Some(current) = (*self.item).clone() else {
            return;
        };
        if !self.can_apply() {
            return;
        }
        let steps = match self.plan(&current) {
            Ok(steps) => steps,
            Err(problem) => {
                self.say(problem, MessageKind::Error);
                return;
            }
        };
        if steps.iter().any(|step| !self.game.supports(&step.edit)) {
            phrases! { UnsupportedDraft: "This draft includes an edit that is not available in this build. Nothing was applied." }
            self.say(UnsupportedDraft, MessageKind::Error);
            return;
        }
        self.start_batch(steps, false);
    }

    /// Draft Undo remains local. Applied reverses run in reverse confirmation order.
    pub fn undo(&mut self) {
        if !self.can_undo() || self.undo_draft_step() {
            return;
        }
        let Some(total) = self.applied else {
            return;
        };
        if self.game.undo_needs_override() && !self.unrestricted() {
            self.say(tr::OverrideRequired, MessageKind::Error);
            return;
        }
        self.batch = Some(WriteBatch {
            steps: VecDeque::new(),
            total,
            names: vec![tr::Undo.into()],
            completed: 0,
            one_way: false,
            undoing: true,
            failed: None,
        });
        *self.writing = true;
        self.advance_writes();
    }

    pub(super) fn start_batch(&mut self, steps: Vec<Step>, undoing: bool) {
        if let Err(error) = self.game.begin_batch() {
            self.say(error, MessageKind::Error);
            return;
        }
        self.batch = Some(WriteBatch {
            total: steps.len(),
            names: steps.iter().map(|s| s.name.clone()).collect(),
            steps: steps.into(),
            completed: 0,
            one_way: false,
            undoing,
            failed: None,
        });
        *self.writing = true;
        self.advance_writes();
    }

    pub(super) fn advance_writes(&mut self) {
        while *self.writing && self.waiting.is_none() {
            if self.game.is_busy() {
                return;
            }
            let undoing = self.batch.as_ref().is_some_and(|b| b.undoing);
            let step = if undoing {
                let batch = self.batch.as_ref().unwrap();
                if batch.completed == batch.total {
                    self.finish_batch(None);
                    return;
                }
                let Some(edit) = self.game.undo_intent() else {
                    phrases! { UndoInvalid: "Undo is no longer valid for the observed item or current settings." }
                    self.finish_batch(Some(UndoInvalid.into()));
                    return;
                };
                Step {
                    name: tr::Undo.into(),
                    edit,
                    row: StepRow::Undo,
                }
            } else if let Some(step) = self.batch.as_mut().and_then(|b| b.steps.pop_front()) {
                step
            } else {
                self.finish_batch(None);
                return;
            };
            let Some(before) = (*self.item).clone() else {
                phrases! { ItemGone: "The forge item is no longer available." }
                self.finish_batch(Some(ItemGone.into()));
                return;
            };
            let (summary, effect) = self.step_summary(&step.row);
            let moves = step.edit.moves(&before);
            self.waiting = Some(WaitingWrite {
                step,
                before,
                moves,
                summary,
                effect,
            });
            phrases! { Applying: "Applying to game… Waiting for independent readback." }
            self.say(Applying, MessageKind::Info);
            let waiting = self.waiting.as_ref().unwrap();
            let expected = waiting.before.clone();
            let edit = waiting.step.edit.clone();
            let outcome = if undoing { self.game.undo(&expected) } else { self.write(&expected, &edit) };
            if matches!(outcome, WriteOutcome::Pending) {
                return;
            }
            self.complete_write(outcome);
        }
    }

    pub(super) fn complete_write(&mut self, outcome: EditOutcome) {
        if matches!(outcome, WriteOutcome::Pending) {
            return;
        }
        let Some(waiting) = self.waiting.take() else {
            return;
        };
        match outcome {
            WriteOutcome::Confirmed(Some(after)) => {
                let known: Vec<u64> = self.affixes.iter().map(|row| row.uid).collect();
                self.update_item(Some(after), Some(&waiting.moves));
                // The row the game made for an added affix takes the draft's place: confirmed, and selected if it was.
                if let (StepRow::Affix(uid), ItemEdit::AddAffix(_)) = (&waiting.step.row, &waiting.step.edit)
                    && let Some(index) = self.affixes.iter().position(|row| !row.added && !known.contains(&row.uid))
                {
                    self.affixes[index].confirmed = true;
                    if self.selected_affix.and_then(|selected| self.affixes.get(selected)).is_some_and(|row| row.uid == *uid) {
                        self.selected_affix = Some(index);
                    }
                }
                let batch = self.batch.as_mut().unwrap();
                if !batch.undoing && self.game.undo_depth() < batch.completed + 1 {
                    batch.one_way = true;
                }
                batch.completed += 1;
                if !batch.undoing {
                    self.history.insert(
                        0,
                        AppliedEntry {
                            name: waiting.step.name,
                            summary: waiting.summary,
                            outcome: tr::Confirmed.into(),
                            time: (self.clock)(),
                            effect: waiting.effect,
                        },
                    );
                    self.clear_applied(&waiting.step.row);
                }
            }
            WriteOutcome::NotApplied { message, .. } => {
                if let Some(batch) = self.batch.as_mut() {
                    batch.failed = Some(waiting.step.name);
                }
                self.finish_batch(Some(message));
            }
            WriteOutcome::Confirmed(None) => {
                let history = std::mem::take(&mut *self.history);
                self.recover_write(None);
                *self.history = history;
                self.history.insert(
                    0,
                    AppliedEntry {
                        name: waiting.step.name,
                        summary: waiting.summary,
                        outcome: tr::Confirmed.into(),
                        time: (self.clock)(),
                        effect: waiting.effect,
                    },
                );
                // Still connected, so no Retry: the item comes back with the game's next report (`refresh`).
                phrases! {
                    ConfirmedUnshown: "The game confirmed the edit, but the item could not be shown afterwards. It shows again the next time the game reports it. \
                     Remaining drafts and Undo were cleared."
                }
                self.say(ConfirmedUnshown, MessageKind::Error);
            }
            WriteOutcome::NotKept(after) => {
                let displayable = after.is_some();
                let history = std::mem::take(&mut *self.history);
                self.recover_write(after);
                *self.history = history;
                self.say(
                    // The worker logs every edit the game did not keep (`log_forge_outcome`). With the item shown the line sits in the
                    // action bar, so it stays short; the empty page has room to say where the details are.
                    if displayable {
                        phrases! { NotKept: "The game did not keep this edit; the item shows what it holds now. Drafts and Undo were cleared." }
                        NotKept.get()
                    } else {
                        phrases! {
                            NotKeptUnshown: "The game did not keep this edit, and the item could not be shown afterwards. It shows again the next time the game reports it. \
                             Drafts and Undo were cleared; details are in the fault log."
                        }
                        NotKeptUnshown.get()
                    },
                    MessageKind::Error,
                );
            }
            // The worker's reason goes to the fault log; the page says what it means and what to do. The connection stops, so the
            // empty forge's one button is Retry.
            WriteOutcome::Uncertain { .. } => {
                let history = std::mem::take(&mut *self.history);
                self.recover_write(None);
                *self.history = history;
                self.say(
                    "The edit may have reached the game, but it could not be confirmed. Check the item in the game, then Retry to read it again. \
                     The edit will not be sent again; drafts and Undo were cleared, and details are in the fault log.",
                    MessageKind::Error,
                );
            }
            WriteOutcome::Pending => unreachable!(),
        }
    }

    pub(super) fn recover_write(&mut self, item: Option<ForgeItem>) {
        self.recovery_required = item.is_none();
        self.batch = None;
        self.waiting = None;
        *self.writing = false;
        self.applied = None;
        self.history.clear();
        // Rebuild from the observation instead of preserving a draft's old base.
        self.update_item(None, None);
        self.update_item(item, None);
    }

    pub(super) fn finish_batch(&mut self, failure: Option<String>) {
        phrases! {
            EditsRestored: "Last applied edit restored" | "{n} applied edits restored"
            Restored: "Restored"
            StepFailed: "{completed} of {total} drafts applied and confirmed; {step} was not applied. {message}"
            SomeRestored: "{completed} of {total} edits restored. {message}"
            SomeApplied: "{completed} of {total} drafts applied and confirmed. {message}"
            AllRestored: "Previous values restored; readback confirmed."
            Confirmed: "Edit readback confirmed: {names}." | "{n} drafts applied. Edit readback confirmed for each: {names}."
            OneWay: "This change is one-way: the game would not give the old rarity back, so Undo apply cannot reverse it."
        }
        let Some(batch) = self.batch.take() else {
            return;
        };
        self.waiting = None;
        *self.writing = false;
        if batch.undoing {
            self.applied = (batch.completed < batch.total && self.game.undo_depth() > 0).then_some(batch.total - batch.completed);
            if batch.completed > 0 {
                for row in self.affixes.iter_mut() {
                    row.confirmed = false;
                }
                for row in self.uniques.iter_mut().chain(self.implicits.iter_mut()) {
                    row.confirmed = false;
                }
                for field in self.fields_mut() {
                    field.confirmed = false;
                }
                self.history.insert(
                    0,
                    AppliedEntry {
                        name: tr::Undo.into(),
                        summary: EditsRestored.count(batch.completed),
                        outcome: Restored.into(),
                        time: (self.clock)(),
                        effect: String::new(),
                    },
                );
            }
        } else if batch.completed > 0 {
            self.applied = if batch.one_way { None } else { Some(batch.completed) };
        }
        if let Some(message) = failure {
            self.say(
                match (batch.completed, batch.undoing, &batch.failed) {
                    (0, ..) => message,
                    // "{applied} of {count} drafts applied and confirmed; {step} was not applied."
                    (completed, false, Some(step)) => StepFailed.with(&[("completed", &completed), ("total", &batch.total), ("step", step), ("message", &message)]),
                    (completed, undoing, _) => {
                        let phrase = if undoing { SomeRestored } else { SomeApplied };
                        phrase.with(&[("completed", &completed), ("total", &batch.total), ("message", &message)])
                    }
                },
                MessageKind::Error,
            );
        } else if batch.undoing {
            self.say(AllRestored, MessageKind::Confirmed);
        } else {
            let mut message = Confirmed.with(batch.total, &[("names", &list(&batch.names, Conjunction::And))]);
            if batch.one_way {
                message.push(' ');
                message.push_str(OneWay.get());
            }
            self.say(message, MessageKind::Confirmed);
        }
        self.applied_at = self.applied.as_ref().and((*self.item).clone());
        self.say_preview("", false, false);
        self.resolve_tables();
    }
}
