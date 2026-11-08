//! Value cells: pressing, dragging, stepping and typing a roll.

use super::*;

impl ForgeEditor {
    pub(super) fn editor_of(&self, cell: Cell) -> Option<(&ValueEditor, u8, Option<u8>)> {
        match cell {
            Cell::Affix(row) => self.affixes.get(row).map(|row| (&row.value, row.roll(), row.game_roll())),
            Cell::Unique(row, roll) => self
                .uniques
                .get(row)
                .and_then(|line| line.rolls.get(roll).map(|draft| (&draft.editor, draft.value, line.game_roll(draft.index)))),
            Cell::Implicit(row, roll) => self
                .implicits
                .get(row)
                .and_then(|line| line.rolls.get(roll).map(|draft| (&draft.editor, draft.value, line.game_roll(draft.index)))),
            Cell::Field(index) => self
                .fields
                .get(index)
                .and_then(|field| field.editor.as_ref().map(|editor| (editor, field.roll(), Some(field.game().clamp(0, 255) as u8)))),
        }
    }

    pub(super) fn editor_mut(&mut self, cell: Cell) -> Option<&mut ValueEditor> {
        match cell {
            Cell::Affix(row) => self.affixes.get_mut(row).map(|row| &mut row.value),
            Cell::Unique(row, roll) => self.uniques.get_mut(row).and_then(|line| line.rolls.get_mut(roll)).map(|draft| &mut draft.editor),
            Cell::Implicit(row, roll) => self.implicits.get_mut(row).and_then(|line| line.rolls.get_mut(roll)).map(|draft| &mut draft.editor),
            Cell::Field(index) => self.fields.get_mut(index).and_then(|field| field.editor.as_mut()),
        }
    }

    /// The cell's value can change now: its mapping is known and nothing locks drafts.
    pub fn can_edit_value(&self, cell: Cell) -> bool {
        let supports = |target| {
            self.game.supports(&ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
                target,
                changes: vec![],
                allow_incompatible: self.unrestricted(),
            }))
        };
        let unlocked = match cell {
            Cell::Implicit(..) => self.can_mutate() && !self.is_base_drafted() && supports(RollTarget::Implicit),
            Cell::Unique(..) => self.can_mutate() && supports(RollTarget::Unique),
            Cell::Field(index) => self.can_change_field(index),
            Cell::Affix(row) => self.can_mutate() && self.affixes.get(row).is_some_and(AffixRow::is_editable) && supports(RollTarget::Affix),
        };
        unlocked && self.editor_of(cell).is_some_and(|(editor, _, _)| editor.has_mapping())
    }

    /// Writes a roll position into the cell's draft; returns whether the draft changed.
    pub(super) fn set_cell(&mut self, cell: Cell, roll: u8) -> bool {
        // Asked first through a shared borrow: a mutable one moves the rows' version, and a scrub that stays on one roll
        // position would redraw the rows and project the whole draft again on every pointer move.
        let takes = match cell {
            Cell::Affix(row) => self.affixes.get(row).is_some_and(|row| row.takes_roll(roll)),
            Cell::Unique(row, position) => self.uniques.get(row).is_some_and(|line| line.takes_roll(position, roll)),
            Cell::Implicit(row, position) => self.implicits.get(row).is_some_and(|line| line.takes_roll(position, roll)),
            Cell::Field(index) => self.fields.get(index).is_some_and(|field| field.takes_roll(roll)),
        };
        if !takes {
            return false;
        }
        let changed = match cell {
            Cell::Affix(row) => self.affixes.get_mut(row).is_some_and(|row| row.set_roll(roll)),
            Cell::Unique(row, position) => self.uniques.get_mut(row).is_some_and(|line| line.set_roll(position, roll)),
            Cell::Implicit(row, position) => self.implicits.get_mut(row).is_some_and(|line| line.set_roll(position, roll)),
            Cell::Field(index) => self.fields.get_mut(index).is_some_and(|field| field.set_roll(roll)),
        };
        if !changed {
            return false;
        }
        match cell {
            Cell::Affix(row) => self.edited(RowRef::Affix(self.affixes[row].uid)),
            Cell::Unique(row, _) => {
                self.edited(RowRef::Unique(self.uniques[row].uid));
                self.mirror_shared(row);
                self.refresh_unique_preview();
            }
            Cell::Implicit(row, _) => self.edited(RowRef::Implicit(self.implicits[row].uid)),
            Cell::Field(index) => self.edited(RowRef::Field(self.fields[index].uid)),
        }
        true
    }

    /// Sets the cell's roll and asks for the tables its new draft context reads; returns whether the draft changed.
    pub(super) fn move_cell(&mut self, cell: Cell, next: Option<u8>) -> bool {
        let changed = next.is_some_and(|next| self.set_cell(cell, next));
        if changed {
            self.resolve_tables();
        }
        changed
    }

    /// A press on a value cell: it selects the row and remembers the value a drag starts from.
    pub fn press(&mut self, cell: Cell) {
        match cell {
            Cell::Affix(row) => self.select_affix(Some(row)),
            Cell::Unique(row, _) => self.select_unique(Some(row)),
            _ => {}
        }
        if let Some((editor, roll, _)) = self.editor_of(cell) {
            let pressed = editor.value(roll);
            if let Some(editor) = self.editor_mut(cell) {
                editor.pressed = pressed;
            }
        }
    }

    pub(super) fn set_dragging(&mut self, cell: Cell, begin: Option<bool>) {
        match (cell, begin) {
            (Cell::Affix(row), Some(true)) => self.affixes[row].begin_drag(),
            (Cell::Affix(row), Some(false)) => self.affixes[row].end_drag(false),
            (Cell::Affix(row), None) => self.affixes[row].end_drag(true),
            (Cell::Unique(row, _), Some(true)) => self.uniques[row].begin_drag(),
            (Cell::Unique(row, _), Some(false)) => self.uniques[row].end_drag(false),
            (Cell::Unique(row, _), None) => self.uniques[row].end_drag(true),
            (Cell::Implicit(row, _), Some(true)) => self.implicits[row].begin_drag(),
            (Cell::Implicit(row, _), Some(false)) => self.implicits[row].end_drag(false),
            (Cell::Implicit(row, _), None) => self.implicits[row].end_drag(true),
            (Cell::Field(index), Some(true)) => self.fields[index].begin_drag(),
            (Cell::Field(index), Some(false)) => self.fields[index].end_drag(false),
            (Cell::Field(index), None) => self.fields[index].end_drag(true),
        }
    }

    /// Scrubs to the pressed value moved by `fraction` of the range: the values a drag passes through are one Undo step.
    /// Returns whether the draft changed; a move that lands on the same roll changes nothing the view shows.
    pub fn scrub(&mut self, cell: Cell, fraction: f64) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let Some((editor, roll, _)) = self.editor_of(cell) else {
            return false;
        };
        let dragging = editor.dragging;
        let target = (editor.pressed + fraction * (editor.maximum() - editor.minimum())).clamp(editor.minimum(), editor.maximum());
        let next = editor.nearest(target, roll);
        if !dragging {
            self.set_dragging(cell, Some(true));
            if let Some(editor) = self.editor_mut(cell) {
                editor.dragging = true;
            }
        }
        self.move_cell(cell, next)
    }

    /// The drag ends; a cancelled one (Escape) puts the value back and leaves no Undo step. A cell whose row was rebuilt
    /// under the pointer ends the drag of the value it began on, wherever that value is now.
    pub fn end_scrub(&mut self, cell: Cell, cancelled: bool) {
        let cell = match self.value_cells().find(|(_, editor)| editor.dragging) {
            Some((dragged, _)) => dragged,
            None => cell,
        };
        let Some(editor) = self.editor_mut(cell) else {
            return;
        };
        if !editor.dragging {
            return;
        }
        editor.dragging = false;
        self.set_dragging(cell, if cancelled { None } else { Some(false) });
        self.after_undo_step(cell);
    }

    pub(super) fn after_undo_step(&mut self, cell: Cell) {
        match cell {
            Cell::Affix(row) => self.track(RowRef::Affix(self.affixes[row].uid)),
            Cell::Unique(row, _) => {
                self.track(RowRef::Unique(self.uniques[row].uid));
                self.mirror_shared(row);
                self.refresh_unique_preview();
            }
            Cell::Implicit(row, _) => self.track(RowRef::Implicit(self.implicits[row].uid)),
            Cell::Field(index) => self.track(RowRef::Field(self.fields[index].uid)),
        }
        self.resolve_tables();
    }

    /// Left / Right: one attainable value; Shift steps five. Returns whether the draft changed.
    pub fn step(&mut self, cell: Cell, steps: i32) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let next = self.editor_of(cell).and_then(|(editor, roll, _)| editor.step(roll, steps));
        self.move_cell(cell, next)
    }

    /// Home / End: the minimum or the maximum. Returns whether the draft changed.
    pub fn step_to_end(&mut self, cell: Cell, maximum: bool) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let next = self.editor_of(cell).and_then(|(editor, roll, _)| editor.end(roll, maximum));
        self.move_cell(cell, next)
    }

    /// Opens the value popover with the value selected; a locked value stays read-only.
    pub fn begin_input(&mut self, cell: Cell) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let Some((_, roll, _)) = self.editor_of(cell) else {
            return false;
        };
        if let Some(editor) = self.editor_mut(cell) {
            editor.begin_input(roll);
        }
        true
    }

    pub fn set_input(&mut self, cell: Cell, text: &str) {
        if let Some(editor) = self.editor_mut(cell) {
            editor.input = text.to_string();
        }
    }

    /// Enter: a valid value goes into the draft (the nearest attainable roll); an invalid one keeps the popover open.
    pub fn commit_input(&mut self, cell: Cell) -> bool {
        if !self.can_edit_value(cell) {
            self.cancel_input(cell);
            return true;
        }
        let Some((editor, roll, _)) = self.editor_of(cell) else {
            return true;
        };
        if !editor.typing {
            return true;
        }
        let Some(value) = editor.parse(&editor.input) else {
            return false;
        };
        let next = editor.nearest(value, roll);
        if let Some(editor) = self.editor_mut(cell) {
            editor.typing = false;
        }
        self.move_cell(cell, next);
        true
    }

    pub fn cancel_input(&mut self, cell: Cell) {
        if let Some(editor) = self.editor_mut(cell) {
            editor.typing = false;
        }
    }

    /// Clicking away keeps a valid typed value and drops an invalid one.
    pub fn finish_input(&mut self, cell: Cell) {
        if !self.commit_input(cell) {
            self.cancel_input(cell);
        }
    }

    /// The value being typed in the popover, if any.
    pub fn typing(&self) -> Option<&ValueEditor> {
        self.affixes
            .iter()
            .map(|row| &row.value)
            .chain(self.uniques.iter().chain(self.implicits.iter()).flat_map(|row| row.rolls.iter().map(|roll| &roll.editor)))
            .chain(self.fields.iter().filter_map(|field| field.editor.as_ref()))
            .find(|editor| editor.typing)
    }

    /// Every value cell with its editor.
    pub(super) fn value_cells(&self) -> impl Iterator<Item = (Cell, &ValueEditor)> {
        fn lines(rows: &[RollRow], cell: fn(usize, usize) -> Cell) -> impl Iterator<Item = (Cell, &ValueEditor)> {
            rows.iter()
                .enumerate()
                .flat_map(move |(row, line)| line.rolls.iter().enumerate().map(move |(roll, value)| (cell(row, roll), &value.editor)))
        }
        self.affixes
            .iter()
            .enumerate()
            .map(|(row, affix)| (Cell::Affix(row), &affix.value))
            .chain(lines(&self.uniques, Cell::Unique))
            .chain(lines(&self.implicits, Cell::Implicit))
            .chain(
                self.fields
                    .iter()
                    .enumerate()
                    .filter_map(|(index, field)| field.editor.as_ref().map(|editor| (Cell::Field(index), editor))),
            )
    }

    /// The cell whose value is being typed in the popover, if any.
    pub fn typing_cell(&self) -> Option<Cell> {
        self.value_cells().find(|(_, editor)| editor.typing).map(|(cell, _)| cell)
    }

    pub fn input_of(&self, cell: Cell) -> Option<&ValueEditor> {
        self.editor_of(cell).map(|(editor, _, _)| editor)
    }

    pub(super) fn typing_rows(&self) -> bool {
        self.affixes.iter().any(|row| row.value.has_invalid_input())
            || self
                .uniques
                .iter()
                .chain(self.implicits.iter())
                .any(|row| row.rolls.iter().any(|roll| roll.editor.has_invalid_input()))
            || self.fields.iter().any(|field| field.editor.as_ref().is_some_and(ValueEditor::has_invalid_input))
    }
}
