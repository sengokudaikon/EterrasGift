//! The ledger: drafts in the order they were made, discarding them, and Ctrl+Z.

use super::*;

impl ForgeEditor {
    pub(super) fn track(&mut self, row: RowRef) {
        let steps = match row {
            RowRef::Affix(uid) => self.affixes.iter().find(|r| r.uid == uid).map(AffixRow::undo_steps),
            RowRef::Unique(uid) => self.uniques.iter().find(|r| r.uid == uid).map(RollRow::undo_steps),
            RowRef::Implicit(uid) => self.implicits.iter().find(|r| r.uid == uid).map(RollRow::undo_steps),
            RowRef::Field(uid) => self.all_fields().find(|r| r.uid == uid).map(FieldDraft::undo_steps),
        }
        .unwrap_or(0);
        let mut entries = self.draft_order.iter().filter(|entry| **entry == row).count();
        while entries < steps {
            self.draft_order.push(row);
            entries += 1;
        }
        let mut index = self.draft_order.len();
        while entries > steps && index > 0 {
            index -= 1;
            if self.draft_order[index] == row {
                self.draft_order.remove(index);
                entries -= 1;
            }
        }
    }

    pub(super) fn edited(&mut self, row: RowRef) {
        self.track(row);
    }

    pub(super) fn forget(&mut self, row: RowRef) {
        self.draft_order.retain(|entry| *entry != row);
    }

    /// The pending drafts in the ledger's order: affixes, unique modifiers, implicits, then item fields.
    pub fn drafts(&self) -> Vec<DraftEntry> {
        let mut entries = Vec::new();
        for row in self.affixes.iter().filter(|row| row.is_dirty()) {
            entries.push(DraftEntry {
                row: RowRef::Affix(row.uid),
                name: row.name(),
                summary: row.change_summary(),
                stale: row.is_stale(),
                // What the item gains: a removal gains nothing.
                effect: if row.is_removed() { String::new() } else { row.value.effects(row.roll()) },
            });
        }
        // Modifiers that only mirror a shared roll stay out of the ledger; their rows preview the change in place.
        for row in self.uniques.iter().filter(|row| row.is_dirty() && row.can_undo()) {
            entries.push(DraftEntry {
                row: RowRef::Unique(row.uid),
                name: row.state.text.clone(),
                summary: row.change_summary(),
                stale: row.is_stale(),
                effect: row.text.clone(),
            });
        }
        for row in self.implicits.iter().filter(|row| row.is_dirty()) {
            entries.push(DraftEntry {
                row: RowRef::Implicit(row.uid),
                name: row.state.text.clone(),
                summary: row.change_summary(),
                stale: row.is_stale(),
                effect: row.effects(),
            });
        }
        for field in self.all_fields().filter(|field| field.is_dirty()) {
            entries.push(DraftEntry {
                row: RowRef::Field(field.uid),
                name: field.name.clone(),
                summary: field.change_summary(),
                stale: field.is_stale(),
                effect: String::new(),
            });
        }
        entries
    }

    /// How many entries `drafts` lists, without building them.
    pub fn draft_count(&self) -> usize {
        self.affixes.iter().filter(|row| row.is_dirty()).count()
            + self.uniques.iter().filter(|row| row.is_dirty() && row.can_undo()).count()
            + self.implicits.iter().filter(|row| row.is_dirty()).count()
            + self.all_fields().filter(|field| field.is_dirty()).count()
    }

    /// An entry selects its affix or unique row, or names the cell that edits it.
    pub fn show_draft(&mut self, entry: usize) -> Option<RowRef> {
        let row = self.drafts().get(entry)?.row;
        match row {
            RowRef::Affix(uid) => {
                let index = self.affixes.iter().position(|row| row.uid == uid);
                self.select_affix(index);
                None
            }
            RowRef::Unique(uid) => {
                let index = self.uniques.iter().position(|row| row.uid == uid);
                self.select_unique(index);
                None
            }
            other => Some(other),
        }
    }

    /// Discards one ledger entry without opening it.
    pub fn discard_entry(&mut self, entry: usize) {
        if !self.can_change_draft() {
            return;
        }
        let Some(DraftEntry { row, name, .. }) = self.drafts().get(entry).cloned() else {
            return;
        };
        self.discard_row_ref(row);
        phrases! { Discarded: "Draft for {name} discarded." }
        self.say(Discarded.with(&[("name", &name)]), MessageKind::Info);
        self.resolve_tables();
    }

    pub(super) fn discard_row_ref(&mut self, row: RowRef) {
        match row {
            RowRef::Affix(uid) => {
                if let Some(index) = self.affixes.iter().position(|row| row.uid == uid) {
                    if self.affixes[index].added {
                        self.drop_added(index);
                    } else {
                        let definition = self.catalog().definition(self.affixes[index].state.id);
                        self.affixes[index].reset(definition);
                    }
                }
            }
            RowRef::Unique(uid) => {
                if let Some(index) = self.uniques.iter().position(|row| row.uid == uid) {
                    self.uniques[index].reset();
                    self.mirror_shared(index);
                    self.refresh_unique_preview();
                }
            }
            RowRef::Implicit(uid) => {
                if let Some(row) = self.implicits.iter_mut().find(|row| row.uid == uid) {
                    row.reset();
                }
            }
            RowRef::Field(uid) => {
                if let Some(field) = self.fields_mut().find(|field| field.uid == uid) {
                    field.reset();
                }
            }
        }
        self.forget(row);
    }

    /// Takes an added row away, keeping the selection on the row it was on, or the one before.
    pub(super) fn drop_added(&mut self, index: usize) {
        let selected = self.selected_affix.and_then(|selected| self.affixes.get(selected)).map(|row| row.uid);
        let uid = self.affixes.remove(index).uid;
        self.forget(RowRef::Affix(uid));
        self.selected_affix = match selected {
            Some(selected) if selected == uid => index.checked_sub(1).or((!self.affixes.is_empty()).then_some(0)),
            Some(selected) => self.affixes.iter().position(|row| row.uid == selected),
            None => None,
        };
    }

    /// The row menu's "Discard draft".
    pub fn discard_row(&mut self, row: usize) {
        if let Some(entry) = self
            .affixes
            .get(row)
            .and_then(|affix| self.drafts().iter().position(|entry| entry.row == RowRef::Affix(affix.uid)))
        {
            self.discard_entry(entry);
        }
    }

    pub fn can_discard_all(&self) -> bool {
        self.can_change_draft() && self.has_pending_drafts()
    }

    pub fn discard_all(&mut self) {
        if !self.can_discard_all() {
            return;
        }
        let count = self.draft_count();
        while let Some(index) = self.affixes.iter().position(|row| row.added) {
            self.drop_added(index);
        }
        for index in 0..self.affixes.len() {
            let definition = self.catalog().definition(self.affixes[index].state.id);
            self.affixes[index].reset(definition);
        }
        for row in self.uniques.iter_mut().chain(self.implicits.iter_mut()) {
            row.reset();
        }
        for field in self.fields_mut() {
            field.reset();
        }
        self.draft_order.clear();
        *self.picker = None;
        self.say_preview("", false, false);
        phrases! { Discarded: "Draft discarded." | "{n} drafts discarded." }
        self.say(Discarded.count(count), MessageKind::Info);
        self.resolve_tables();
    }

    pub fn draft_count_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! { NoDrafts: "No drafts" }
        if drafts == 0 { NoDrafts.into() } else { tr::Drafts.count(drafts) }
    }

    pub fn changes_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! {
            Changes: "Changes"
            ChangesCounted: "Changes ({n})"
        }
        if drafts == 0 { Changes.into() } else { ChangesCounted.with(&[("n", &drafts)]) }
    }

    pub fn discard_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! { Discard: "Discard draft" | "Discard {n} drafts" }
        Discard.count(drafts)
    }

    pub fn apply_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! {
            ApplyMany: "Apply {n} draft" | "Apply {n} drafts"
            Apply: "Apply to game"
        }
        if drafts > 1 { ApplyMany.count(drafts) } else { Apply.into() }
    }

    pub fn can_apply(&self) -> bool {
        self.can_change_draft() && self.picker.is_none() && self.has_pending_drafts() && !self.typing_rows()
    }

    pub(super) fn has_draft_steps(&self) -> bool {
        self.affixes.iter().any(AffixRow::can_undo) || self.uniques.iter().chain(self.implicits.iter()).any(RollRow::can_undo) || self.all_fields().any(FieldDraft::can_undo)
    }

    /// With no draft change left to step back, Undo writes the values from before the last Apply back to the game, while
    /// the worker still holds them: it drops them when the item it observes no longer matches.
    pub fn undo_writes_to_game(&self) -> bool {
        self.can_change_draft() && !self.has_draft_steps() && self.applied.is_some() && self.game.undo_depth() > 0
    }

    pub fn can_undo(&self) -> bool {
        (self.can_change_draft() && self.has_draft_steps()) || self.undo_writes_to_game()
    }

    pub fn undo_text(&self) -> &'static str {
        phrases! { UndoApply: "Undo apply" }
        if self.undo_writes_to_game() { UndoApply.get() } else { tr::Undo.get() }
    }

    pub fn undo_tip(&self) -> &'static str {
        phrases! {
            UndoApplyTip: "Write back the values from before the last Apply"
            UndoTip: "Undo the last draft change (Ctrl+Z)"
        }
        if self.undo_writes_to_game() { UndoApplyTip.get() } else { UndoTip.get() }
    }

    pub(super) fn can_undo_row(&self, row: RowRef) -> bool {
        match row {
            RowRef::Affix(uid) => self.affixes.iter().any(|r| r.uid == uid && r.can_undo()),
            RowRef::Unique(uid) => self.uniques.iter().any(|r| r.uid == uid && r.can_undo()),
            RowRef::Implicit(uid) => self.implicits.iter().any(|r| r.uid == uid && r.can_undo()),
            RowRef::Field(uid) => self.all_fields().any(|r| r.uid == uid && r.can_undo()),
        }
    }

    pub(super) fn last_edited(&mut self) -> Option<RowRef> {
        while let Some(row) = self.draft_order.pop() {
            if self.can_undo_row(row) {
                return Some(row);
            }
        }
        self.affixes
            .iter()
            .find(|r| r.can_undo())
            .map(|r| RowRef::Affix(r.uid))
            .or_else(|| self.uniques.iter().find(|r| r.can_undo()).map(|r| RowRef::Unique(r.uid)))
            .or_else(|| self.implicits.iter().find(|r| r.can_undo()).map(|r| RowRef::Implicit(r.uid)))
            .or_else(|| self.all_fields().find(|r| r.can_undo()).map(|r| RowRef::Field(r.uid)))
    }

    /// Steps back the most recent draft change, selecting the affix or unique row it belongs to.
    pub(super) fn undo_draft_step(&mut self) -> bool {
        if !self.can_change_draft() {
            return false;
        }
        let Some(row) = self.last_edited() else {
            return false;
        };
        match row {
            RowRef::Affix(uid) => {
                let index = self.affixes.iter().position(|r| r.uid == uid);
                self.select_affix(index);
                if let Some(index) = index {
                    // An addition's first step is the addition itself.
                    if self.affixes[index].added && self.affixes[index].undo_steps() == 1 {
                        self.drop_added(index);
                    } else {
                        self.affixes[index].undo_draft();
                    }
                }
            }
            RowRef::Unique(uid) => {
                let index = self.uniques.iter().position(|r| r.uid == uid);
                self.select_unique(index);
                if let Some(index) = index {
                    self.uniques[index].undo_draft();
                    self.mirror_shared(index);
                    self.refresh_unique_preview();
                }
            }
            RowRef::Implicit(uid) => {
                if let Some(row) = self.implicits.iter_mut().find(|r| r.uid == uid) {
                    row.undo_draft();
                }
            }
            RowRef::Field(uid) => {
                if let Some(field) = self.fields_mut().find(|r| r.uid == uid) {
                    field.undo_draft();
                }
            }
        }
        phrases! { Undone: "Last draft change undone." }
        self.say(Undone, MessageKind::Info);
        self.resolve_tables();
        true
    }

    /// Ctrl+Z: draft changes only; past the last one it says where Undo apply is instead of writing.
    pub fn undo_draft_only(&mut self) {
        if !self.undo_draft_step() && self.undo_writes_to_game() {
            phrases! { NothingLeft: "Nothing left to undo. Undo apply writes back the values from before the last Apply." }
            self.say(NothingLeft, MessageKind::Info);
        }
    }
}
