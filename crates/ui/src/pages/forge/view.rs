//! The forge in the window: `ForgeEditor` behind the `ForgeState` global. Every edit goes to the editor and the whole
//! state is shown again; the row models are updated in place so a row's cells, popups and drags survive the update.

use super::editor::{Cell, ForgeEditor, KindFilter, MessageKind, RowRef};
use super::game::{self, Lookup};
use super::item::{ForgeItem, Seal};
use super::rows::{FieldKind, ValueEditor};
use super::worker::ForgeGame;
use crate::models::update_rows as update;
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
    identity_options: Rc<VecModel<MenuOptionView>>,
    picker_rows: Rc<VecModel<PickerRowView>>,
}

/// What each menu model was last built from: a scrub or a keystroke rebuilds none of them unless that changed.
#[derive(Default)]
struct Built {
    /// The menu generation, the held unique and the search.
    identity: RefCell<Option<(u64, i64, String)>>,
    /// The menu generation and the held base.
    base: RefCell<Option<(u64, i64)>>,
    /// Per affix row, what its tier list shows.
    tiers: RefCell<Vec<Option<TierKey>>>,
    /// The menu generation and the definitions the picker lists.
    picker: RefCell<Option<(u64, Vec<usize>)>>,
}

/// What a row's tier list depends on: the affix drafted, its highest tier and the game's own, and the catalog.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TierKey {
    affix: u16,
    max_tier: u8,
    added: bool,
    game_affix: u16,
    game_tier: u8,
    generation: u64,
}

struct View {
    editor: RefCell<ForgeEditor>,
    window: Weak<MainWindow>,
    models: Models,
    built: Built,
    focus_serial: Flag<i32>,
}

/// The forge as a window shows it. The trainer binding and the captures each supply its game.
#[derive(Clone)]
pub struct ForgeHandle {
    view: Rc<View>,
}

impl ForgeHandle {
    pub fn observe(&self, game: ForgeGame) {
        self.view.editor.borrow_mut().observe(game);
        self.view.show();
    }

    /// Runs `change` on the editor and shows the result.
    pub fn edit<T>(&self, change: impl FnOnce(&mut ForgeEditor) -> T) -> T {
        let result = change(&mut self.view.editor.borrow_mut());
        self.view.show();
        result
    }

    /// Runs `change` on the editor and shows the result only when `change` reports that something changed.
    pub fn edit_if(&self, change: impl FnOnce(&mut ForgeEditor) -> bool) -> bool {
        let changed = change(&mut self.view.editor.borrow_mut());
        if changed {
            self.view.show();
        }
        changed
    }
}

/// Puts an observation provider behind the window's forge. Production uses owned
/// snapshots and command-based writes; fixtures exercise the same editor flow.
pub fn attach(window: &MainWindow, game: ForgeGame) -> ForgeHandle {
    let view = Rc::new(View {
        editor: RefCell::new(ForgeEditor::new(game)),
        window: window.as_weak(),
        models: Models::default(),
        built: Built::default(),
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
    state.set_identity_options(ModelRc::from(view.models.identity_options.clone()));
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

/// The view's name for a cell, the inverse of `cell_of`.
fn reference_of(cell: Cell) -> CellRef {
    match cell {
        Cell::Affix(row) => reference(CellKind::Affix, row, 0),
        Cell::Unique(row, roll) => reference(CellKind::Unique, row, roll),
        Cell::Implicit(row, roll) => reference(CellKind::Implicit, row, roll),
        Cell::Field(row) => reference(CellKind::Field, row, 0),
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
    // `edit!` runs the body on the editor and shows the result, returning the body's value (`else` gives the value for a
    // window already gone). `edit!(view, changed ...)` shows only when the body returns true.
    macro_rules! edit {
        ($view:expr, |$editor:ident $(, $arg:ident)*| $body:expr) => {
            edit!($view, else Default::default(), |$editor $(, $arg)*| $body)
        };
        ($view:expr, else $fallback:expr, |$editor:ident $(, $arg:ident)*| $body:expr) => {{
            let view = on($view);
            move |$($arg),*| {
                let Some(view) = view.upgrade() else {
                    return $fallback;
                };
                let result = {
                    let mut $editor = view.editor.borrow_mut();
                    $body
                };
                view.show();
                result
            }
        }};
        ($view:expr, changed |$editor:ident $(, $arg:ident)*| $body:expr) => {{
            let view = on($view);
            move |$($arg),*| {
                if let Some(view) = view.upgrade() {
                    let changed = {
                        let mut $editor = view.editor.borrow_mut();
                        $body
                    };
                    if changed {
                        view.show();
                    }
                }
            }
        }};
    }
    state.on_select_affix(edit!(view, |editor, index| editor.select_affix(usize::try_from(index).ok())));
    state.on_select_unique(edit!(view, |editor, index| editor.select_unique(usize::try_from(index).ok())));
    state.on_press(edit!(view, |editor, cell| editor.press(cell_of(&cell))));
    // A move that lands on the same roll changes nothing on screen, so the pointer's every move is not a full update.
    state.on_scrub(edit!(view, changed | editor, cell, fraction | editor.scrub(cell_of(&cell), f64::from(fraction))));
    state.on_end_scrub(edit!(view, |editor, cell, cancelled| editor.end_scrub(cell_of(&cell), cancelled)));
    state.on_step(edit!(view, changed | editor, cell, steps | editor.step(cell_of(&cell), steps)));
    state.on_step_to_end(edit!(view, changed | editor, cell, maximum | editor.step_to_end(cell_of(&cell), maximum)));
    state.on_input_edited(edit!(view, |editor, cell, text| editor.set_input(cell_of(&cell), &text)));
    state.on_cancel_input(edit!(view, |editor, cell| editor.cancel_input(cell_of(&cell))));
    state.on_finish_input(edit!(view, |editor, cell| editor.finish_input(cell_of(&cell))));
    state.on_pick_tier(edit!(view, |editor, row, index| editor.pick_tier(row.max(0) as usize, (index + 1).clamp(1, 8) as u8)));
    state.on_pick_seal(edit!(view, |editor, row, index| editor.pick_seal(row.max(0) as usize, Seal::ALL[index.clamp(0, 3) as usize])));
    state.on_toggle_flag(edit!(view, |editor, index, on| editor.toggle_flag(index.max(0) as usize, on)));
    state.on_pick_rarity(edit!(view, |editor, index| editor.pick_rarity(index == 1)));
    state.on_pick_base(edit!(view, |editor, index| editor.pick_base(index.max(0) as usize)));
    state.on_pick_identity(edit!(view, |editor, index| editor.pick_identity(index.max(0) as usize)));
    state.on_identity_search_edited(edit!(view, |editor, text| editor.set_identity_search(&text)));
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
    state.on_max_tiers(edit!(view, |editor| editor.max_tiers()));
    state.on_max_rolls(edit!(view, |editor| editor.max_rolls()));
    state.on_apply(edit!(view, |editor| editor.apply()));
    state.on_undo(edit!(view, |editor| editor.undo()));
    state.on_undo_draft(edit!(view, |editor| editor.undo_draft_only()));
    let weak = on(view);
    state.on_begin_input(edit!(view, |editor, cell| {
        let cell = cell_of(&cell);
        let opened = editor.begin_input(cell);
        if opened && let (Some(input), Some(window)) = (editor.input_of(cell), weak.upgrade().and_then(|view| view.window.upgrade())) {
            let state = window.global::<ForgeState>();
            state.set_input_label(input.label().into());
            state.set_input_text(input.input.clone().into());
            state.set_input_range(format!("{} – {}", input.minimum_text(), input.maximum_text()).into());
        }
        opened
    }));
    // A window already gone has no popover to keep open.
    state.on_commit_input(edit!(view, else true, |editor, cell| editor.commit_input(cell_of(&cell))));
    state.on_open_replace(edit!(view, |editor, row| editor.open_replace(row.max(0) as usize)));
    state.on_open_add(edit!(view, |editor| editor.open_add()));
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
    // Up and Down from a unique modifier's roll: the nearest line in that direction with rolls to scrub takes the selection
    // and the focus; past the ends nothing moves.
    let weak = on(view);
    state.on_move_unique(move |row, delta, roll| {
        let Some(view) = weak.upgrade() else {
            return;
        };
        let target = {
            let editor = view.editor.borrow();
            let lines = &editor.uniques;
            let step = delta.signum() as isize;
            let mut next = row as isize + step;
            let mut found = None;
            while step != 0 && next >= 0 && (next as usize) < lines.len() {
                let line = &lines[next as usize];
                if line.is_editable() && !line.rolls.is_empty() {
                    found = Some((next as usize, (roll.max(0) as usize).min(line.rolls.len() - 1)));
                    break;
                }
                next += step;
            }
            found
        };
        if let Some((line, roll)) = target {
            view.editor.borrow_mut().select_unique(Some(line));
            view.focus(reference(CellKind::Unique, line, roll), 0);
        }
    });
    // Up and Down from an implicit's roll: the nearest implicit in that direction with a roll takes the focus.
    let weak = on(view);
    state.on_move_implicit(move |row, delta, roll| {
        let Some(view) = weak.upgrade() else {
            return;
        };
        let target = {
            let editor = view.editor.borrow();
            let lines = &editor.implicits;
            let step = delta.signum() as isize;
            let mut next = row as isize + step;
            let mut found = None;
            while step != 0 && next >= 0 && (next as usize) < lines.len() {
                let line = &lines[next as usize];
                if !line.rolls.is_empty() {
                    found = Some((next as usize, (roll.max(0) as usize).min(line.rolls.len() - 1)));
                    break;
                }
                next += step;
            }
            found
        };
        if let Some((line, roll)) = target {
            view.focus(reference(CellKind::Implicit, line, roll), 0);
        }
    });
    // A ledger entry selects its affix or unique row, or brings the cell that edits it into focus: a value's scrubber, a
    // flag's toggle, or the base or the identity in the header.
    let weak = on(view);
    state.on_show_draft(move |index| {
        let Some(view) = weak.upgrade() else {
            return;
        };
        let target = view.editor.borrow_mut().show_draft(index.max(0) as usize);
        let cell = target.and_then(|row| {
            let editor = view.editor.borrow();
            let header = CellRef {
                kind: CellKind::Field,
                row: -1,
                roll: -1,
            };
            match row {
                RowRef::Implicit(uid) => editor
                    .implicits
                    .iter()
                    .position(|line| line.uid == uid)
                    .map(|line| (reference(CellKind::Implicit, line, 0), 0)),
                RowRef::Field(uid) if editor.base.as_ref().is_some_and(|base| base.uid == uid) => Some((header, 4)),
                RowRef::Field(uid) if editor.identity.as_ref().is_some_and(|identity| identity.uid == uid) => Some((header, 5)),
                RowRef::Field(uid) => editor
                    .fields
                    .iter()
                    .position(|field| field.uid == uid)
                    .map(|index| (reference(CellKind::Field, index, 0), i32::from(editor.fields[index].kind != FieldKind::Number))),
                _ => None,
            }
        });
        // `focus` shows the state itself.
        match cell {
            Some((cell, part)) => view.focus(cell, part),
            None => view.show(),
        }
    });
    // Menus' keyboard highlights start on the held entry and skip the entries that can't be picked.
    state.on_option_start(|options| {
        let available = |index: usize| options.row_data(index).is_some_and(|option| option.available);
        (0..options.row_count())
            .find(|&index| available(index) && options.row_data(index).is_some_and(|option| option.current))
            .or_else(|| (0..options.row_count()).find(|&index| available(index)))
            .map_or(-1, |index| index as i32)
    });
    state.on_option_step(|options, from, delta| {
        let step = delta.signum();
        let mut next = from + step;
        while step != 0 && next >= 0 && (next as usize) < options.row_count() {
            if options.row_data(next as usize).is_some_and(|option| option.available) {
                return next;
            }
            next += step;
        }
        from
    });
}

impl View {
    /// Shows the state, then moves the keyboard focus to `cell`.
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
        // The item as Apply would leave it, projected once for everything that reads it.
        let drafted = editor.drafted_item();
        let drafted = drafted.as_ref();
        let generation = editor.menu_generation();
        state.set_has_item(editor.has_item());
        state.set_write_recovery_required(editor.needs_write_recovery());
        state.set_item_name(editor.item_name().into());
        state.set_has_identity(editor.has_identity());
        state.set_identity_drafted(editor.is_identity_drafted());
        state.set_can_change_identity(editor.can_change_identity());
        state.set_identity_tip(editor.identity_tip().into());
        state.set_identity_search(editor.identity_search.clone().into());
        let held = editor.identity.as_ref().map_or(-1, |identity| identity.value());
        let identities_built = self
            .built
            .identity
            .borrow()
            .as_ref()
            .is_some_and(|(built, built_held, search)| *built == generation && *built_held == held && *search == editor.identity_search);
        if !identities_built {
            let identities: Vec<MenuOptionView> = editor
                .identity_choices()
                .into_iter()
                .map(|option| MenuOptionView {
                    name: option.name.clone().into(),
                    detail: option.detail.clone().into(),
                    implicits: SharedString::new(),
                    in_game: option.in_game,
                    available: option.available,
                    current: option.value == held,
                    rarity: rarity_of(option.rarity),
                })
                .collect();
            update(&self.models.identity_options, identities);
            *self.built.identity.borrow_mut() = Some((generation, held, editor.identity_search.clone()));
        }
        // Unique, set and legendary come from the item before exalted does, so a legendary with a T6+ affix keeps its own
        // colour.
        let rarity = editor.rarity_code();
        state.set_name_rarity(if !matches!(rarity, 7..=9) && editor.name_is_exalted() {
            ItemRarity::Exalted
        } else {
            rarity_of(rarity)
        });
        state.set_rarity_text(editor.rarity_text_of(drafted).into());
        state.set_rarity_drafted(editor.is_rarity_drafted_of(drafted));
        state.set_can_change_rarity(editor.can_change_rarity());
        state.set_rarity_tip(editor.rarity_tip().into());
        let rarities: Vec<MenuOptionView> = editor
            .rarity_options_of(drafted)
            .into_iter()
            .map(|option| MenuOptionView {
                name: option.name.into(),
                detail: option.detail.into(),
                implicits: SharedString::new(),
                in_game: option.in_game,
                available: option.available,
                current: option.current,
                rarity: ItemRarity::Normal,
            })
            .collect();
        state.set_rarity_caption_lines(rarities.iter().filter(|option| !option.detail.is_empty()).count() as i32);
        update(&self.models.rarity_options, rarities);
        state.set_base_text(editor.base_text().into());
        state.set_base_drafted(editor.is_base_drafted());
        state.set_can_change_base(editor.can_change_base());
        state.set_base_tip(editor.base_tip().into());
        let base_held = editor.base.as_ref().map_or(-1, |base| base.value());
        if *self.built.base.borrow() != Some((generation, base_held)) {
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
                            rarity: ItemRarity::Normal,
                        })
                        .collect()
                })
                .unwrap_or_default();
            update(&self.models.base_options, bases);
            *self.built.base.borrow_mut() = Some((generation, base_held));
        }
        state.set_base_allow_incompatible(editor.base.as_ref().is_some_and(|base| base.allow_incompatible));
        state.set_base_can_edit(editor.can_edit_base_field());
        state.set_summary(editor.summary().into());
        self.show_fields(&editor);

        state.set_has_implicits(editor.has_implicits());
        state.set_implicit_rows_shown(!editor.shows_implicit_preview());
        state.set_implicit_note(editor.implicit_note().into());
        state.set_implicit_preview_draft(editor.is_base_drafted());
        update(&self.models.implicit_preview, editor.implicit_preview().into_iter().map(SharedString::from).collect());
        let implicits = self.lines(&editor, CellKind::Implicit);
        update(&self.models.implicits, implicits);

        self.show_affixes(&state, &editor, drafted);

        let unique = editor.item().and_then(|item| item.backing());
        state.set_has_unique(unique.is_some());
        let uniques = self.lines(&editor, CellKind::Unique);
        update(&self.models.uniques, uniques);
        state.set_selected_unique(editor.selected_unique().map_or(-1, |index| index as i32));
        state.set_preview_status(editor.preview_status.clone().into());
        state.set_preview_failed(editor.preview_failed);
        state.set_preview_stale(editor.preview_stale);
        let formatting = unique.map(|unique| &unique.formatting);
        state.set_formatting_loading(matches!(formatting, Some(Lookup::Loading)));
        state.set_formatting_error(
            match formatting {
                Some(Lookup::Loading) => game::UNIQUE_LOADING,
                Some(Lookup::Unavailable(reason)) => reason.as_str(),
                Some(Lookup::Ready(())) | None => "",
            }
            .into(),
        );

        update(
            &self.models.drafts,
            editor
                .drafts()
                .into_iter()
                .map(|entry| DraftEntryView {
                    name: entry.name.into(),
                    summary: entry.summary.into(),
                    effect: entry.effect.into(),
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
        let typing = editor.typing_cell();
        state.set_typing(typing.is_some());
        state.set_typing_cell(typing.map_or(
            CellRef {
                kind: CellKind::Affix,
                row: -1,
                roll: -1,
            },
            reference_of,
        ));
        self.show_picker(&state, &editor, drafted);
    }

    fn show_fields(&self, editor: &ForgeEditor) {
        let fields: Vec<FieldView> = editor
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let cell = reference(CellKind::Field, index, 0);
                let value = match field.editor.as_ref() {
                    Some(values) => value_cell(
                        cell,
                        uid_of(field.uid, 0),
                        values,
                        field.roll(),
                        Some(field.game().clamp(0, 255) as u8),
                        field.is_dirty(),
                        field.was(),
                        editor.can_edit_value(Cell::Field(index)),
                        field.value_tip(),
                    ),
                    None => ValueCell {
                        cell,
                        uid: uid_of(field.uid, 0),
                        ..ValueCell::default()
                    },
                };
                FieldView {
                    name: field.name.clone().into(),
                    numeric: field.kind == FieldKind::Number,
                    value,
                    tip: field.tip().into(),
                    tip_heading: field.tip_heading().into(),
                    confirmed: field.confirmed,
                    stale: field.is_stale(),
                    editable: field.is_editable(),
                    can_edit: editor.can_change_field(index),
                    on_name: field.on_name().into(),
                    off_name: field.off_name().into(),
                    checked: field.value() != 0,
                    drafted: field.is_dirty(),
                }
            })
            .collect();
        update(&self.models.fields, fields);
    }

    fn show_affixes(&self, state: &ForgeState, editor: &ForgeEditor, drafted: Option<&ForgeItem>) {
        let mut tier_models = self.models.tiers.borrow_mut();
        let mut tier_keys = self.built.tiers.borrow_mut();
        let generation = editor.menu_generation();
        let rows: Vec<AffixRowView> = editor
            .affixes
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let key = TierKey {
                    affix: row.draft_id(),
                    max_tier: row.max_tier(),
                    added: row.added,
                    game_affix: row.state.id,
                    game_tier: row.state.tier,
                    generation,
                };
                let tiers = if tier_keys.get(index) == Some(&Some(key)) {
                    ModelRc::from(tier_models[index].clone())
                } else {
                    let tiers: Vec<TierOption> = editor
                        .tier_options(index)
                        .into_iter()
                        .map(|(tier, range, in_game)| TierOption {
                            label: format!("T{tier}").into(),
                            range: range.into(),
                            in_game,
                        })
                        .collect();
                    let model = stable(&mut tier_models, index, tiers);
                    tier_keys.resize(tier_models.len(), None);
                    tier_keys[index] = Some(key);
                    model
                };
                let range = editor.range_text(index);
                AffixRowView {
                    value: value_cell(
                        reference(CellKind::Affix, index, 0),
                        uid_of(row.uid, 0),
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
                    added: row.added,
                    removed: row.is_removed(),
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
                    can_edit: editor.can_change_affix() && row.is_editable(),
                    dirty: row.is_dirty(),
                    pending: row.is_pending(),
                    stale: row.is_stale(),
                    draft_tip: row.change_summary().into(),
                    confirmed: row.confirmed,
                    tiers,
                }
            })
            .collect();
        tier_models.truncate(rows.len());
        tier_keys.truncate(rows.len());
        update(&self.models.affixes, rows);
        state.set_selected_affix(editor.selected_affix().map_or(-1, |index| index as i32));
        state.set_can_change_affix_list(editor.can_change_affix_list());
        state.set_add_affix_tip(editor.add_affix_tip().into());
        state.set_can_max_tiers(editor.can_max_tiers());
        state.set_max_tiers_tip(editor.max_tiers_tip().into());
        state.set_can_max_rolls(editor.can_max_rolls());
        state.set_max_rolls_tip(editor.max_rolls_tip().into());
        state.set_affix_list_note(editor.free_slots_text_of(drafted).into());
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
                            reference_of(cell),
                            uid_of(line.uid, position),
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

    fn show_picker(&self, state: &ForgeState, editor: &ForgeEditor, drafted: Option<&ForgeItem>) {
        let Some(picker) = editor.picker.as_ref() else {
            state.set_picker_open(false);
            return;
        };
        state.set_picker_open(true);
        state.set_picker_title(editor.picker_title().into());
        state.set_picker_subtitle(editor.picker_subtitle_of(drafted).into());
        state.set_picker_search(picker.search.clone().into());
        state.set_picker_filter(match picker.filter {
            KindFilter::All => 0,
            KindFilter::Prefixes => 1,
            KindFilter::Suffixes => 2,
        });
        state.set_picker_allow(picker.allow_incompatible);
        state.set_picker_allow_enabled(editor.can_mutate());
        // Choosing an entry shows again with the same list; its rows are rebuilt only when the list changes.
        let generation = editor.menu_generation();
        let listed = editor.picker_indices();
        let built = self
            .built
            .picker
            .borrow()
            .as_ref()
            .is_some_and(|(built, indices)| *built == generation && indices.as_slice() == listed);
        if !built {
            update(
                &self.models.picker_rows,
                editor
                    .picker_list()
                    .map(|definition| PickerRowView {
                        name: definition.name.clone().into(),
                        detail: definition.picker_description().into(),
                        reforged: definition.is_reforged(),
                    })
                    .collect(),
            );
            *self.built.picker.borrow_mut() = Some((generation, listed.to_vec()));
        }
        state.set_picker_selected(
            picker
                .selected
                .and_then(|id| editor.picker_list().position(|definition| definition.id == id))
                .map_or(-1, |index| index as i32),
        );
        state.set_picker_status(editor.picker_status().into());
        state.set_picker_action(editor.picker_action().into());
        state.set_can_use_picker(editor.can_use_picker());
    }
}

/// The game's rarity code determines its colour independently of localized names.
fn rarity_of(rarity: u8) -> ItemRarity {
    match rarity {
        9 => ItemRarity::Legendary,
        8 => ItemRarity::Set,
        7 => ItemRarity::Unique,
        0 => ItemRarity::Normal,
        1 | 2 => ItemRarity::Magic,
        _ => ItemRarity::Rare,
    }
}

/// A value's identity for the view, from its row's uid and its roll. Only a change matters, so the bits that fit suffice.
fn uid_of(row: u64, roll: usize) -> i32 {
    (row as i32).wrapping_mul(64).wrapping_add(roll as i32)
}

#[allow(clippy::too_many_arguments)]
fn value_cell(cell: CellRef, uid: i32, editor: &ValueEditor, roll: u8, game: Option<u8>, draft: bool, was: String, editable: bool, tip: String) -> ValueCell {
    let fraction = editor.fraction(roll) as f32;
    ValueCell {
        cell,
        uid,
        label: editor.label().into(),
        display: editor.display(roll).into(),
        was: was.into(),
        draft,
        mapped: editor.has_scale(),
        fixed: editor.is_fixed(),
        editable,
        fraction,
        game_fraction: game.map_or(fraction, |game| editor.fraction(game) as f32),
        game_tick: editor.has_scale() && game.is_some_and(|game| game != roll),
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
