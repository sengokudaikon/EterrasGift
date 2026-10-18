//! The forge in the window: `ForgeEditor` behind the `ForgeState` global. Every edit goes to the editor and the whole
//! state is shown again; the row models are updated in place so a row's cells, popups and drags survive the update.

use super::editor::{Cell, ForgeEditor, KindFilter, MessageKind, RowRef};
use super::game::ForgeGame;
use super::item::Seal;
use super::rows::{FieldKind, ValueEditor};
use crate::{
    AffixRowView, AppliedEntryView, CellKind, CellRef, DraftEntryView, FieldView, ForgeState, ItemRarity, MainWindow, MenuOptionView, MessageTone, ModifierRowView, PickerRowView,
    TierOption, ValueCell,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::{Cell as Flag, RefCell};
use std::rc::Rc;

/// The models a row keeps across updates, so its repeated cells are not rebuilt under the pointer.
#[derive(Default)]
struct Models {
    affixes: Rc<VecModel<AffixRowView>>,
    tiers: RefCell<Vec<Rc<VecModel<TierOption>>>>,
    uniques: Rc<VecModel<ModifierRowView>>,
    unique_rolls: RefCell<Vec<Rc<VecModel<ValueCell>>>>,
    implicits: Rc<VecModel<ModifierRowView>>,
    implicit_rolls: RefCell<Vec<Rc<VecModel<ValueCell>>>>,
    implicit_preview: Rc<VecModel<SharedString>>,
    fields: Rc<VecModel<FieldView>>,
    drafts: Rc<VecModel<DraftEntryView>>,
    history: Rc<VecModel<AppliedEntryView>>,
    rarity_options: Rc<VecModel<MenuOptionView>>,
    base_options: Rc<VecModel<MenuOptionView>>,
    picker_rows: Rc<VecModel<PickerRowView>>,
}

struct View {
    editor: RefCell<ForgeEditor>,
    window: Weak<MainWindow>,
    models: Models,
    focus_serial: Flag<i32>,
}

/// The forge as a window shows it, for whoever holds the game (the trainer's session once ported; captures until then).
#[derive(Clone)]
pub struct ForgeHandle {
    view: Rc<View>,
}

impl ForgeHandle {
    /// Reads the forge again, as when the session publishes another read of the item.
    pub fn refresh(&self) {
        self.view.editor.borrow_mut().refresh();
        self.view.show();
    }

    /// Runs `change` on the editor and shows the result.
    pub fn edit<T>(&self, change: impl FnOnce(&mut ForgeEditor) -> T) -> T {
        let result = change(&mut self.view.editor.borrow_mut());
        self.view.show();
        result
    }
}

/// Puts a game behind the window's forge. The real app has none yet (TODO(forge): the trainer's session), so its Forge
/// page keeps the connection's empty state.
pub fn attach(window: &MainWindow, game: Box<dyn ForgeGame>) -> ForgeHandle {
    let view = Rc::new(View {
        editor: RefCell::new(ForgeEditor::new(game)),
        window: window.as_weak(),
        models: Models::default(),
        focus_serial: Flag::new(0),
    });
    let state = window.global::<ForgeState>();
    state.set_affixes(ModelRc::from(view.models.affixes.clone()));
    state.set_unique_modifiers(ModelRc::from(view.models.uniques.clone()));
    state.set_implicits(ModelRc::from(view.models.implicits.clone()));
    state.set_implicit_preview(ModelRc::from(view.models.implicit_preview.clone()));
    state.set_fields(ModelRc::from(view.models.fields.clone()));
    state.set_drafts(ModelRc::from(view.models.drafts.clone()));
    state.set_history(ModelRc::from(view.models.history.clone()));
    state.set_rarity_options(ModelRc::from(view.models.rarity_options.clone()));
    state.set_base_options(ModelRc::from(view.models.base_options.clone()));
    state.set_picker_rows(ModelRc::from(view.models.picker_rows.clone()));
    bind(&state, &view);
    view.show();
    ForgeHandle { view }
}

fn cell_of(cell: &CellRef) -> Cell {
    let row = cell.row.max(0) as usize;
    let roll = cell.roll.max(0) as usize;
    match cell.kind {
        CellKind::Affix => Cell::Affix(row),
        CellKind::Unique => Cell::Unique(row, roll),
        CellKind::Implicit => Cell::Implicit(row, roll),
        CellKind::Field => Cell::Field(row),
    }
}

fn reference(kind: CellKind, row: usize, roll: usize) -> CellRef {
    CellRef {
        kind,
        row: row as i32,
        roll: roll as i32,
    }
}

fn bind(state: &ForgeState, view: &Rc<View>) {
    let on = |view: &Rc<View>| Rc::downgrade(view);
    macro_rules! edit {
        ($view:expr, |$editor:ident $(, $arg:ident)*| $body:expr) => {{
            let view = on($view);
            move |$($arg),*| {
                if let Some(view) = view.upgrade() {
                    {
                        let mut $editor = view.editor.borrow_mut();
                        $body;
                    }
                    view.show();
                }
            }
        }};
    }
    state.on_select_affix(edit!(view, |editor, index| editor.select_affix(usize::try_from(index).ok())));
    state.on_select_unique(edit!(view, |editor, index| editor.select_unique(usize::try_from(index).ok())));
    state.on_press(edit!(view, |editor, cell| editor.press(cell_of(&cell))));
    state.on_scrub(edit!(view, |editor, cell, fraction| editor.scrub(cell_of(&cell), f64::from(fraction))));
    state.on_end_scrub(edit!(view, |editor, cell, cancelled| editor.end_scrub(cell_of(&cell), cancelled)));
    state.on_step(edit!(view, |editor, cell, steps| editor.step(cell_of(&cell), steps)));
    state.on_step_to_end(edit!(view, |editor, cell, maximum| editor.step_to_end(cell_of(&cell), maximum)));
    state.on_input_edited(edit!(view, |editor, cell, text| editor.set_input(cell_of(&cell), &text)));
    state.on_cancel_input(edit!(view, |editor, cell| editor.cancel_input(cell_of(&cell))));
    state.on_finish_input(edit!(view, |editor, cell| editor.finish_input(cell_of(&cell))));
    state.on_pick_tier(edit!(view, |editor, row, index| editor.pick_tier(row.max(0) as usize, (index + 1).clamp(1, 8) as u8)));
    state.on_pick_seal(edit!(view, |editor, row, index| editor.pick_seal(row.max(0) as usize, Seal::ALL[index.clamp(0, 3) as usize])));
    state.on_toggle_flag(edit!(view, |editor, index, on| editor.toggle_flag(index.max(0) as usize, on)));
    state.on_pick_rarity(edit!(view, |editor, index| editor.pick_rarity(index == 1)));
    state.on_pick_base(edit!(view, |editor, index| editor.pick_base(index.max(0) as usize)));
    state.on_base_allow(edit!(view, |editor, allow| editor.set_base_allow_incompatible(allow)));
    state.on_picker_search_edited(edit!(view, |editor, text| editor.picker_search(&text)));
    state.on_picker_filter_changed(edit!(view, |editor, filter| editor.picker_filter(match filter {
        1 => KindFilter::Prefixes,
        2 => KindFilter::Suffixes,
        _ => KindFilter::All,
    })));
    state.on_picker_allow_changed(edit!(view, |editor, allow| editor.picker_allow(allow)));
    state.on_picker_select(edit!(view, |editor, index| editor.picker_select(index.max(0) as usize)));
    state.on_use_picker(edit!(view, |editor| editor.use_picker()));
    state.on_cancel_picker(edit!(view, |editor| editor.cancel_picker()));
    state.on_discard_row(edit!(view, |editor, row| editor.discard_row(row.max(0) as usize)));
    state.on_remove_affix(edit!(view, |editor, row| editor.remove_affix(row.max(0) as usize)));
    state.on_discard_entry(edit!(view, |editor, index| editor.discard_entry(index.max(0) as usize)));
    state.on_discard_all(edit!(view, |editor| editor.discard_all()));
    state.on_apply(edit!(view, |editor| editor.apply()));
    state.on_undo(edit!(view, |editor| editor.undo()));
    state.on_undo_draft(edit!(view, |editor| editor.undo_draft_only()));

    let weak = on(view);
    state.on_begin_input(move |cell| {
        let Some(view) = weak.upgrade() else {
            return false;
        };
        let opened = view.editor.borrow_mut().begin_input(cell_of(&cell));
        if opened {
            let editor = view.editor.borrow();
            if let (Some(input), Some(window)) = (editor.input_of(cell_of(&cell)), view.window.upgrade()) {
                let state = window.global::<ForgeState>();
                state.set_input_label(input.label().into());
                state.set_input_text(input.input.clone().into());
                state.set_input_range(format!("{} – {}", input.minimum_text(), input.maximum_text()).into());
            }
        }
        view.show();
        opened
    });
    let weak = on(view);
    state.on_commit_input(move |cell| {
        let Some(view) = weak.upgrade() else {
            return true;
        };
        let committed = view.editor.borrow_mut().commit_input(cell_of(&cell));
        view.show();
        committed
    });
    let weak = on(view);
    state.on_open_replace(move |row| {
        let Some(view) = weak.upgrade() else {
            return false;
        };
        let opened = view.editor.borrow_mut().open_replace(row.max(0) as usize);
        view.show();
        opened
    });
    let weak = on(view);
    state.on_open_add(move || {
        let Some(view) = weak.upgrade() else {
            return false;
        };
        let opened = view.editor.borrow_mut().open_add();
        view.show();
        opened
    });
    // Up and Down from a cell move the selection; the same cell of the next row takes focus.
    let weak = on(view);
    state.on_move_row(move |row, delta, part| {
        let Some(view) = weak.upgrade() else {
            return;
        };
        let count = view.editor.borrow().affixes.len() as i32;
        let next = (row + delta).clamp(0, (count - 1).max(0));
        view.editor.borrow_mut().select_affix(Some(next as usize));
        view.focus(reference(CellKind::Affix, next as usize, 0), part);
    });
    // A ledger entry selects its affix or unique row, or brings the cell that edits it into focus.
    let weak = on(view);
    state.on_show_draft(move |index| {
        let Some(view) = weak.upgrade() else {
            return;
        };
        let target = view.editor.borrow_mut().show_draft(index.max(0) as usize);
        let cell = target.and_then(|row| {
            let editor = view.editor.borrow();
            match row {
                RowRef::Implicit(uid) => editor.implicits.iter().position(|line| line.uid == uid).map(|line| reference(CellKind::Implicit, line, 0)),
                RowRef::Field(uid) => editor
                    .fields
                    .iter()
                    .position(|field| field.uid == uid && field.kind == FieldKind::Number)
                    .map(|field| reference(CellKind::Field, field, 0)),
                _ => None,
            }
        });
        view.show();
        if let Some(cell) = cell {
            view.focus(cell, 0);
        }
    });
}

impl View {
    fn focus(&self, cell: CellRef, part: i32) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<ForgeState>();
        self.show();
        state.set_focus_cell(cell);
        state.set_focus_part(part);
        self.focus_serial.set(self.focus_serial.get() + 1);
        state.set_focus_serial(self.focus_serial.get());
    }

    fn show(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<ForgeState>();
        let editor = self.editor.borrow();
        state.set_has_item(editor.has_item());
        if !editor.has_item() {
            return;
        }
        state.set_item_name(editor.item().map(|item| item.name.clone()).unwrap_or_default().into());
        state.set_name_rarity(match editor.rarity_name() {
            "Legendary" => ItemRarity::Legendary,
            "Set" => ItemRarity::Set,
            "Unique" => ItemRarity::Unique,
            "Exalted" => ItemRarity::Exalted,
            "Rare" => ItemRarity::Rare,
            "Magic" => ItemRarity::Magic,
            _ => ItemRarity::Normal,
        });
        state.set_rarity_text(editor.rarity_text().into());
        state.set_rarity_drafted(editor.is_rarity_drafted());
        state.set_can_change_rarity(editor.can_change_rarity());
        state.set_rarity_tip(editor.rarity_tip().into());
        let rarities: Vec<MenuOptionView> = editor
            .rarity_options()
            .into_iter()
            .map(|option| MenuOptionView {
                name: option.name.into(),
                detail: option.detail.into(),
                implicits: SharedString::new(),
                in_game: option.in_game,
                available: option.available,
                current: option.current,
            })
            .collect();
        state.set_rarity_caption_lines(rarities.iter().filter(|option| !option.detail.is_empty()).count() as i32);
        update(&self.models.rarity_options, rarities);
        state.set_base_text(editor.base_text().into());
        state.set_base_drafted(editor.is_base_drafted());
        state.set_can_change_base(editor.can_change_base());
        state.set_base_tip(editor.base_tip().into());
        let bases: Vec<MenuOptionView> = editor
            .base
            .as_ref()
            .map(|base| {
                base.options
                    .iter()
                    .map(|option| MenuOptionView {
                        name: option.name.clone().into(),
                        detail: option.detail.clone().into(),
                        implicits: option.implicits.clone().into(),
                        in_game: option.in_game,
                        available: option.available,
                        current: option.value == base.value(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let base_lines = bases
            .iter()
            .map(|option| option.implicits.lines().count() + usize::from(!option.detail.is_empty()))
            .sum::<usize>();
        state.set_base_caption_lines(base_lines as i32);
        update(&self.models.base_options, bases);
        state.set_base_allow_incompatible(editor.base.as_ref().is_some_and(|base| base.allow_incompatible));
        state.set_summary(editor.summary().into());
        self.show_fields(&state, &editor);

        state.set_has_implicits(editor.has_implicits());
        state.set_implicit_rows_shown(!editor.shows_implicit_preview());
        state.set_implicit_note(editor.implicit_note().into());
        state.set_implicit_preview_draft(editor.is_base_drafted());
        update(&self.models.implicit_preview, editor.implicit_preview().into_iter().map(SharedString::from).collect());
        let implicits = self.lines(&editor, CellKind::Implicit);
        update(&self.models.implicits, implicits);

        self.show_affixes(&state, &editor);

        state.set_has_unique(editor.item().is_some_and(|item| item.unique.is_some()));
        let uniques = self.lines(&editor, CellKind::Unique);
        update(&self.models.uniques, uniques);
        state.set_selected_unique(editor.selected_unique().map_or(-1, |index| index as i32));
        state.set_preview_status(editor.preview_status.clone().into());
        state.set_preview_failed(editor.preview_failed);
        state.set_preview_stale(editor.preview_stale);
        state.set_formatting_error(
            editor
                .item()
                .and_then(|item| item.unique.as_ref())
                .map(|unique| unique.formatting_error.clone())
                .unwrap_or_default()
                .into(),
        );

        let drafts = editor.drafts();
        update(
            &self.models.drafts,
            drafts
                .iter()
                .map(|entry| DraftEntryView {
                    name: entry.name.clone().into(),
                    summary: entry.summary.clone().into(),
                    effect: entry.effect.clone().into(),
                    stale: entry.stale,
                })
                .collect(),
        );
        state.set_draft_count_text(editor.draft_count_text().into());
        update(
            &self.models.history,
            editor
                .history
                .iter()
                .map(|entry| AppliedEntryView {
                    name: entry.name.clone().into(),
                    time: entry.time.clone().into(),
                    summary: entry.summary.clone().into(),
                    effect: entry.effect.clone().into(),
                    outcome: entry.outcome.clone().into(),
                })
                .collect(),
        );

        let (message, kind) = editor.message();
        state.set_message(message.into());
        state.set_message_tone(match kind {
            MessageKind::Info => MessageTone::Note,
            MessageKind::Confirmed => MessageTone::Confirmed,
            MessageKind::Error => MessageTone::Error,
        });
        state.set_changes_text(editor.changes_text().into());
        state.set_undo_text(editor.undo_text().into());
        state.set_undo_tip(editor.undo_tip().into());
        state.set_can_undo(editor.can_undo());
        state.set_discard_text(editor.discard_text().into());
        state.set_can_discard(editor.can_discard_all());
        state.set_apply_text(editor.apply_text().into());
        state.set_can_apply(editor.can_apply());

        state.set_input_error(editor.typing().map(ValueEditor::input_error).unwrap_or_default().into());
        self.show_picker(&state, &editor);
    }

    fn show_fields(&self, state: &ForgeState, editor: &ForgeEditor) {
        let fields: Vec<FieldView> = editor
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let cell = reference(CellKind::Field, index, 0);
                let value = match field.editor.as_ref() {
                    Some(values) => value_cell(
                        cell,
                        values,
                        field.roll(),
                        Some(field.game().clamp(0, 255) as u8),
                        field.is_dirty(),
                        field.was(),
                        editor.can_edit_value(Cell::Field(index)),
                        field.value_tip(),
                    ),
                    None => ValueCell { cell, ..ValueCell::default() },
                };
                FieldView {
                    name: field.name.clone().into(),
                    numeric: field.kind == FieldKind::Number,
                    value,
                    tip: field.tip().into(),
                    confirmed: field.confirmed,
                    stale: field.is_stale(),
                    editable: field.is_editable(),
                    can_edit: field.is_editable() && editor.can_mutate(),
                    on_name: field.on_name().into(),
                    off_name: field.off_name().into(),
                    checked: field.value() != 0,
                    drafted: field.is_dirty(),
                }
            })
            .collect();
        update(&self.models.fields, fields);
        let _ = state;
    }

    fn show_affixes(&self, state: &ForgeState, editor: &ForgeEditor) {
        let mut tier_models = self.models.tiers.borrow_mut();
        let rows: Vec<AffixRowView> = editor
            .affixes
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let tiers: Vec<TierOption> = editor
                    .tier_options(index)
                    .into_iter()
                    .map(|(tier, range, in_game)| TierOption {
                        label: format!("T{tier}").into(),
                        range: range.into(),
                        in_game,
                    })
                    .collect();
                let range = editor.range_text(index);
                AffixRowView {
                    value: value_cell(
                        reference(CellKind::Affix, index, 0),
                        &row.value,
                        row.roll(),
                        row.game_roll(),
                        row.is_value_drafted(),
                        row.was(),
                        editor.can_edit_value(Cell::Affix(index)),
                        row.value.tip(row.roll(), row.game_roll()),
                    ),
                    name: row.name().into(),
                    replaced: row.is_replaced(),
                    detail: row.detail().into(),
                    reforged: row.is_reforged(),
                    name_tip: row.name_tip().into(),
                    range: range.into(),
                    tier: format!("T{}", row.tier()).into(),
                    tier_drafted: row.is_tier_drafted(),
                    exalted: row.is_exalted(),
                    tier_index: i32::from(row.tier()) - 1,
                    seal: row.seal().name().into(),
                    sealed: row.seal() != Seal::Normal,
                    seal_drafted: row.is_seal_drafted(),
                    seal_index: row.seal().index() as i32,
                    can_edit: editor.can_mutate(),
                    dirty: row.is_dirty(),
                    pending: row.is_pending(),
                    stale: row.is_stale(),
                    draft_tip: row.change_summary().into(),
                    confirmed: row.confirmed,
                    tiers: stable(&mut tier_models, index, tiers),
                }
            })
            .collect();
        tier_models.truncate(rows.len());
        update(&self.models.affixes, rows);
        state.set_selected_affix(editor.selected_affix().map_or(-1, |index| index as i32));
        state.set_can_change_affix_list(editor.can_change_affix_list());
        state.set_add_affix_tip(editor.add_affix_tip().into());
        state.set_affix_list_note(editor.affix_list_note().into());
        state.set_ladder_note(editor.tier_ladder_note().into());
    }

    fn lines(&self, editor: &ForgeEditor, kind: CellKind) -> Vec<ModifierRowView> {
        let (rows, models) = match kind {
            CellKind::Unique => (&editor.uniques, &self.models.unique_rolls),
            _ => (&editor.implicits, &self.models.implicit_rolls),
        };
        let mut models = models.borrow_mut();
        let lines: Vec<ModifierRowView> = rows
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let cells: Vec<ValueCell> = line
                    .rolls
                    .iter()
                    .enumerate()
                    .map(|(position, roll)| {
                        let cell = if kind == CellKind::Unique {
                            Cell::Unique(index, position)
                        } else {
                            Cell::Implicit(index, position)
                        };
                        let game = line.game_roll(roll.index);
                        value_cell(
                            reference(kind, index, position),
                            &roll.editor,
                            roll.value,
                            game,
                            line.is_dirty(),
                            line.was(roll),
                            editor.can_edit_value(cell),
                            roll.editor.tip(roll.value, game),
                        )
                    })
                    .collect();
                ModifierRowView {
                    rolls: stable(&mut models, index, cells),
                    editable: line.is_editable(),
                    fixed_text: line.read_only_text().into(),
                    text: line.text.clone().into(),
                    drafted: line.is_dirty(),
                    range: line.range.clone().into(),
                    confirmed: line.confirmed,
                    stale: line.is_stale(),
                    pending: line.is_pending(),
                    draft_tip: line.change_summary().into(),
                    linked: if kind == CellKind::Unique {
                        editor.linked_with(index).into()
                    } else {
                        SharedString::new()
                    },
                }
            })
            .collect();
        models.truncate(lines.len());
        lines
    }

    fn show_picker(&self, state: &ForgeState, editor: &ForgeEditor) {
        let Some(picker) = editor.picker.as_ref() else {
            state.set_picker_open(false);
            return;
        };
        state.set_picker_open(true);
        state.set_picker_title(editor.picker_title().into());
        state.set_picker_subtitle(editor.picker_subtitle().into());
        state.set_picker_search(picker.search.clone().into());
        state.set_picker_filter(match picker.filter {
            KindFilter::All => 0,
            KindFilter::Prefixes => 1,
            KindFilter::Suffixes => 2,
        });
        state.set_picker_allow(picker.allow_incompatible);
        state.set_picker_allow_enabled(editor.can_mutate());
        update(
            &self.models.picker_rows,
            picker
                .list
                .iter()
                .map(|definition| PickerRowView {
                    name: definition.name.clone().into(),
                    detail: definition.kind_description().into(),
                    reforged: definition.is_reforged(),
                })
                .collect(),
        );
        state.set_picker_selected(
            picker
                .selected
                .and_then(|id| picker.list.iter().position(|definition| definition.id == id))
                .map_or(-1, |index| index as i32),
        );
        state.set_picker_status(editor.picker_status().into());
        state.set_picker_action(editor.picker_action().into());
        state.set_can_use_picker(editor.can_use_picker());
    }
}

#[allow(clippy::too_many_arguments)]
fn value_cell(cell: CellRef, editor: &ValueEditor, roll: u8, game: Option<u8>, draft: bool, was: String, editable: bool, tip: String) -> ValueCell {
    let fraction = editor.fraction(roll) as f32;
    ValueCell {
        cell,
        label: editor.label().into(),
        display: editor.display(roll).into(),
        was: was.into(),
        draft,
        mapped: editor.has_mapping(),
        editable,
        fraction,
        game_fraction: game.map_or(fraction, |game| editor.fraction(game) as f32),
        game_tick: editor.has_mapping() && game.is_some_and(|game| game != roll),
        tip: tip.into(),
    }
}

/// The model kept for row `index`, updated in place.
fn stable<T: Clone + PartialEq + 'static>(models: &mut Vec<Rc<VecModel<T>>>, index: usize, rows: Vec<T>) -> ModelRc<T> {
    while models.len() <= index {
        models.push(Rc::new(VecModel::default()));
    }
    update(&models[index], rows);
    ModelRc::from(models[index].clone())
}

fn update<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
    } else {
        for (index, row) in rows.into_iter().enumerate() {
            if model.row_data(index).as_ref() != Some(&row) {
                model.set_row_data(index, row);
            }
        }
    }
}
