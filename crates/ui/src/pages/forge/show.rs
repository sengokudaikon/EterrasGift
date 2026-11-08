//! The forge's state on the `ForgeState` global. Single values are set on every render, and Slint ignores those that didn't
//! change. Each list is a region, remade only when its key moves, and updated row by row so a row's cells, popups and drags
//! survive. A region's key is what its rows read: a test build makes the rows anyway and fails if a key misses an input.

use super::picker_rows::{Listed, PickerRows};
use crate::{
    AffixRowView, AppliedEntryView, CellKind, CellRef, DraftEntryView, FieldView, ForgeState, ItemRarity, MenuOptionView, MessageTone, ModifierRowView, PickerView,
    TemplateRowView, TierOption, ValueCell,
};
use eg_trainer::screens::forge::editor::{AppliedEntry, Cell, ForgeEditor, MessageKind, Picker, tr};
use eg_trainer::screens::forge::feature::State;
use eg_trainer::screens::forge::game::{self, Lookup};
use eg_trainer::screens::forge::item::{ForgeItem, Seal};
use eg_trainer::screens::forge::picker::{self, KindFilter, SourceChoice};
use eg_trainer::screens::forge::rows::{AffixRow, FieldKind, ValueEditor};
use slint::{ModelRc, SharedString};
use spiderland::{CheckedSlice, Slice, Tracked, Version};
use spiderland_slint::{InnerRows, Rows};
use std::rc::Rc;

pub(super) fn cell_of(cell: &CellRef) -> Cell {
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
pub(super) fn reference_of(cell: Cell) -> CellRef {
    let reference = |kind, row: usize, roll: usize| CellRef {
        kind,
        row: row as i32,
        roll: roll as i32,
    };
    match cell {
        Cell::Affix(row) => reference(CellKind::Affix, row, 0),
        Cell::Unique(row, roll) => reference(CellKind::Unique, row, roll),
        Cell::Implicit(row, roll) => reference(CellKind::Implicit, row, roll),
        Cell::Field(row) => reference(CellKind::Field, row, 0),
    }
}

/// What every row's editable state reads: whether drafts can change at all, and the worker's latest word, with what it allows.
#[derive(Clone, Copy, PartialEq)]
struct Gates {
    can_mutate: bool,
    game: u64,
}

type RowVersions = [Version; 7];
/// Rows each made with the list inside them apart, so the rows can be compared, and their lists kept in models of their own.
type WithLists<R, T> = Vec<(R, Vec<T>)>;
type Lines = WithLists<ModifierRowView, ValueCell>;
/// A menu's options, each with its facts.
type Options = WithLists<MenuOptionView, SharedString>;
/// What the picker's rows are built from: the catalog, what is listed, the slot and the base their values are scaled for.
type PickerKey = (u64, Vec<usize>, Option<u16>, Option<(u16, u16)>);

/// The lists remade by key.
#[derive(Default)]
struct Regions {
    drafted: CheckedSlice<(RowVersions, u64), Option<ForgeItem>>,
    fields: CheckedSlice<(Version, Gates), Vec<FieldView>>,
    implicits: CheckedSlice<(Version, Version, Gates), Lines>,
    uniques: CheckedSlice<(Version, Version, Gates), Lines>,
    affixes: CheckedSlice<(RowVersions, Gates), WithLists<AffixRowView, TierOption>>,
    drafts: CheckedSlice<(RowVersions, u64), Vec<DraftEntryView>>,
    history: Slice<Version>,
    identity_options: CheckedSlice<(u64, i64, String), Options>,
    base_options: CheckedSlice<(u64, i64), Options>,
    /// What they say of each template comes from the catalog.
    template_rows: CheckedSlice<(Version, u64), WithLists<TemplateRowView, SharedString>>,
    input: Slice<Version>,
    picker: Slice<PickerKey>,
}

/// The models the view holds for its whole life, updated in place so its repeated rows aren't rebuilt under the pointer.
#[derive(Default)]
struct Models {
    affixes: Rows<AffixRowView>,
    tiers: InnerRows<TierOption>,
    uniques: Rows<ModifierRowView>,
    unique_rolls: InnerRows<ValueCell>,
    implicits: Rows<ModifierRowView>,
    implicit_rolls: InnerRows<ValueCell>,
    implicit_preview: Rows<SharedString>,
    summary: Rows<SharedString>,
    fields: Rows<FieldView>,
    drafts: Rows<DraftEntryView>,
    history: Rows<AppliedEntryView>,
    rarity_options: Rows<MenuOptionView>,
    base_options: Rows<MenuOptionView>,
    base_facts: InnerRows<SharedString>,
    identity_options: Rows<MenuOptionView>,
    identity_facts: InnerRows<SharedString>,
    template_rows: Rows<TemplateRowView>,
    template_facts: InnerRows<SharedString>,
    picker_rows: Rc<PickerRows>,
    picker_sides: Rows<SharedString>,
    picker_categories: Rows<SharedString>,
    picker_classes: Rows<SharedString>,
    picker_slots: Rows<SharedString>,
    picker_levels: Rows<SharedString>,
    picker_sources: Rows<SharedString>,
    picker_tags: Rows<SharedString>,
    picker_tags_on: Rows<bool>,
    picker_chips: Rows<SharedString>,
}

pub(super) struct View {
    models: Models,
    regions: Regions,
    /// The item as Apply would leave it, which most of what follows reads.
    drafted: Option<ForgeItem>,
}

impl View {
    pub fn new(forge: &ForgeState) -> View {
        let models = Models::default();
        forge.set_affixes(models.affixes.model());
        forge.set_unique_modifiers(models.uniques.model());
        forge.set_implicits(models.implicits.model());
        forge.set_implicit_preview(models.implicit_preview.model());
        forge.set_summary(models.summary.model());
        forge.set_fields(models.fields.model());
        forge.set_drafts(models.drafts.model());
        forge.set_history(models.history.model());
        forge.set_rarity_options(models.rarity_options.model());
        forge.set_base_options(models.base_options.model());
        forge.set_identity_options(models.identity_options.model());
        forge.set_template_rows(models.template_rows.model());
        // The picker's lists are its own, given again whenever it shows; these three name what never changes.
        models.picker_sides.sync(picker::PICKER_SIDES.iter().map(|side| SharedString::from(side.get())));
        models
            .picker_categories
            .sync(picker::PICKER_CATEGORIES.iter().map(|(name, _)| SharedString::from(name.get())));
        models.picker_classes.sync(picker::PICKER_CLASSES.iter().map(|class| SharedString::from(class.get())));
        View {
            models,
            regions: Regions::default(),
            drafted: None,
        }
    }

    pub fn show(&mut self, forge: &ForgeState, state: &State) {
        let editor = state.editor();
        let (regions, models, drafted) = (&mut self.regions, &mut self.models, &mut self.drafted);
        let rows = editor.row_versions();
        let game = editor.game_generation();
        regions.drafted.update("drafted", (rows, game), || editor.drafted_item(), |item| *drafted = item);
        let gates = Gates {
            can_mutate: editor.can_mutate(),
            game,
        };
        let drafted = drafted.as_ref();

        show_templates(forge, state, regions, models);
        show_input(forge, state, regions);
        show_header(forge, editor, drafted, regions, models);

        regions
            .fields
            .update("fields", (editor.fields.version(), gates), || field_rows(editor), |rows| models.fields.sync(rows));

        forge.set_has_implicits(editor.has_implicits());
        forge.set_implicit_rows_shown(!editor.shows_implicit_preview());
        forge.set_implicit_note(editor.implicit_note().into());
        forge.set_implicit_preview_draft(editor.is_base_drafted());
        models.implicit_preview.sync(editor.implicit_preview().into_iter().map(SharedString::from));
        regions.implicits.update(
            "implicits",
            (editor.implicits.version(), editor.base.version(), gates),
            || lines(editor, CellKind::Implicit),
            |rows| models.implicits.sync(models.implicit_rolls.fill(rows, |row, rolls| row.rolls = rolls)),
        );

        regions.affixes.update(
            "affixes",
            (rows, gates),
            || affix_rows(editor),
            |rows| models.affixes.sync(models.tiers.fill(rows, |row, tiers| row.tiers = tiers)),
        );
        forge.set_selected_affix(editor.selected_affix().map_or(-1, |index| index as i32));
        forge.set_can_change_affix_list(editor.can_change_affix_list());
        forge.set_add_affix_tip(editor.add_affix_tip().into());
        forge.set_can_max_tiers(editor.can_max_tiers());
        forge.set_max_tiers_tip(editor.max_tiers_tip().into());
        forge.set_can_max_rolls(editor.can_max_rolls());
        forge.set_max_rolls_tip(editor.max_rolls_tip().into());
        forge.set_affix_list_note(editor.free_slots_text_of(drafted).into());
        forge.set_ladder_note(editor.tier_ladder_note().into());

        let unique = editor.item().and_then(|item| item.backing());
        forge.set_has_unique(unique.is_some());
        regions.uniques.update(
            "uniques",
            (editor.uniques.version(), editor.item_version(), gates),
            || lines(editor, CellKind::Unique),
            |rows| models.uniques.sync(models.unique_rolls.fill(rows, |row, rolls| row.rolls = rolls)),
        );
        forge.set_selected_unique(editor.selected_unique().map_or(-1, |index| index as i32));
        forge.set_preview_status(editor.preview_status.clone().into());
        forge.set_preview_failed(editor.preview_failed);
        forge.set_preview_stale(editor.preview_stale);
        let formatting = unique.map(|unique| &unique.formatting);
        forge.set_formatting_loading(matches!(formatting, Some(Lookup::Loading)));
        forge.set_formatting_error(
            match formatting {
                Some(Lookup::Loading) => game::UniqueLoading.get(),
                Some(Lookup::Unavailable(reason)) => reason.as_str(),
                Some(Lookup::Ready(())) | None => "",
            }
            .into(),
        );

        regions.drafts.update("drafts", (rows, game), || draft_rows(editor), |rows| models.drafts.sync(rows));
        forge.set_draft_count_text(editor.draft_count_text().into());
        regions.history.show(&editor.history, history_rows, |rows| models.history.sync(rows));

        let (message, kind) = editor.message();
        forge.set_message(message.into());
        forge.set_message_tone(match kind {
            MessageKind::Info => MessageTone::Note,
            MessageKind::Confirmed => MessageTone::Confirmed,
            MessageKind::Error => MessageTone::Error,
        });
        forge.set_changes_text(editor.changes_text().into());
        forge.set_undo_text(editor.undo_text().into());
        forge.set_undo_tip(editor.undo_tip().into());
        forge.set_can_undo(editor.can_undo());
        forge.set_discard_text(editor.discard_text().into());
        forge.set_can_discard(editor.can_discard_all());
        forge.set_apply_text(editor.apply_text().into());
        forge.set_can_apply(editor.can_apply());

        show_picker(forge, editor, drafted, regions, models);
    }
}

/// The kept templates, read again from their folder when something says to, and the name typed for a new one.
fn show_templates(forge: &ForgeState, state: &State, regions: &mut Regions, models: &mut Models) {
    let editor = state.editor();
    forge.set_has_templates(editor.keeps_templates);
    forge.set_template_message(editor.template_message.clone().into());
    regions.template_rows.update(
        "templates",
        (editor.templates.version(), editor.menu_generation()),
        || {
            editor
                .templates
                .iter()
                .map(|template| {
                    let detail = editor.template_detail(template);
                    let row = TemplateRowView {
                        name: template.name.clone().into(),
                        label: format!("{}\n{}", template.name, detail.join(", ")).into(),
                        facts: ModelRc::default(),
                    };
                    (row, facts(&detail))
                })
                .collect()
        },
        |rows| models.template_rows.sync(models.template_facts.fill(rows, |row, facts| row.facts = facts)),
    );
    forge.set_template_name(editor.template_name.clone().into());
    forge.set_template_replaces(editor.template_replaces().unwrap_or_default().into());
}

/// The typing popover: what it opened on, and how the typing stands.
fn show_input(forge: &ForgeState, state: &State, regions: &mut Regions) {
    let editor = state.editor();
    // The label, text and range are the ones the popover opened with.
    let mut shown = forge.get_cell_input();
    regions.input.update(state.opened_input().version(), || {
        if let Some(input) = state.opened_input().and_then(|cell| editor.input_of(cell)) {
            shown.label = input.label().into();
            shown.text = input.input.clone().into();
            shown.range = format!("{} – {}", input.minimum_text(), input.maximum_text()).into();
        }
    });
    shown.error = editor.typing().map(ValueEditor::input_error).unwrap_or_default().into();
    let typing = editor.typing_cell();
    shown.typing = typing.is_some();
    shown.cell = typing.map_or(
        CellRef {
            kind: CellKind::Affix,
            row: -1,
            roll: -1,
        },
        reference_of,
    );
    forge.set_cell_input(shown);
}

fn show_header(forge: &ForgeState, editor: &ForgeEditor, drafted: Option<&ForgeItem>, regions: &mut Regions, models: &mut Models) {
    let generation = editor.menu_generation();
    forge.set_has_item(editor.has_item());
    forge.set_write_recovery_required(editor.needs_write_recovery());
    forge.set_item_name(editor.item_name().into());
    forge.set_has_identity(editor.has_identity());
    forge.set_identity_drafted(editor.is_identity_drafted());
    forge.set_can_change_identity(editor.can_change_identity());
    forge.set_identity_tip(editor.identity_tip().into());
    forge.set_identity_search(editor.identity_search.clone().into());
    let held = editor.identity.as_ref().map_or(-1, |identity| identity.value());
    regions.identity_options.update(
        "identity options",
        (generation, held, editor.identity_search.clone()),
        || {
            editor
                .identity_choices()
                .into_iter()
                .map(|option| {
                    let row = MenuOptionView {
                        name: option.name.clone().into(),
                        facts: ModelRc::default(),
                        note: option.note.clone().into(),
                        implicits: SharedString::new(),
                        in_game: option.in_game,
                        available: option.available,
                        current: option.value == held,
                        rarity: rarity_of(option.rarity),
                    };
                    (row, facts(&option.facts))
                })
                .collect()
        },
        |rows| models.identity_options.sync(models.identity_facts.fill(rows, |row, facts| row.facts = facts)),
    );
    // Unique, set and legendary come from the item before exalted does, so a legendary with a T6+ affix keeps its own colour.
    let rarity = editor.rarity_code();
    forge.set_name_rarity(if !matches!(rarity, 7..=9) && editor.name_is_exalted() {
        ItemRarity::Exalted
    } else {
        rarity_of(rarity)
    });
    forge.set_rarity_text(editor.rarity_text_of(drafted).into());
    forge.set_rarity_drafted(editor.is_rarity_drafted_of(drafted));
    forge.set_can_change_rarity(editor.can_change_rarity());
    forge.set_rarity_tip(editor.rarity_tip().into());
    let rarities: Vec<MenuOptionView> = editor
        .rarity_options_of(drafted)
        .into_iter()
        .map(|option| MenuOptionView {
            name: option.name.into(),
            facts: ModelRc::default(),
            note: option.note.into(),
            implicits: SharedString::new(),
            in_game: option.in_game,
            available: option.available,
            current: option.current,
            rarity: ItemRarity::Normal,
        })
        .collect();
    forge.set_rarity_caption_lines(rarities.iter().filter(|option| !option.note.is_empty()).count() as i32);
    models.rarity_options.sync(rarities);
    forge.set_base_text(editor.base_text().into());
    forge.set_base_drafted(editor.is_base_drafted());
    forge.set_can_change_base(editor.can_change_base());
    forge.set_base_tip(editor.base_tip().into());
    let base_held = editor.base.as_ref().map_or(-1, |base| base.value());
    regions.base_options.update(
        "base options",
        (generation, base_held),
        || {
            let Some(base) = editor.base.as_ref() else {
                return Vec::new();
            };
            base.options
                .iter()
                .map(|option| {
                    let row = MenuOptionView {
                        name: option.name.clone().into(),
                        facts: ModelRc::default(),
                        note: option.note.clone().into(),
                        implicits: option.implicits.clone().into(),
                        in_game: option.in_game,
                        available: option.available,
                        current: option.value == base.value(),
                        rarity: ItemRarity::Normal,
                    };
                    (row, facts(&option.facts))
                })
                .collect()
        },
        |rows| models.base_options.sync(models.base_facts.fill(rows, |row, facts| row.facts = facts)),
    );
    forge.set_base_allow_incompatible(editor.base.as_ref().is_some_and(|base| base.allow_incompatible));
    forge.set_base_can_edit(editor.can_edit_base_field());
    models.summary.sync(editor.summary().into_iter().map(SharedString::from));
}

fn field_rows(editor: &ForgeEditor) -> Vec<FieldView> {
    editor
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let cell = reference_of(Cell::Field(index));
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
        .collect()
}

/// The affix rows, each with the tiers its menu lists, scaled for the base the item would have.
fn affix_rows(editor: &ForgeEditor) -> WithLists<AffixRowView, TierOption> {
    editor
        .affixes
        .iter()
        .enumerate()
        .map(|(index, row)| (affix_row(editor, index, row), tier_options(editor, index)))
        .collect()
}

fn tier_options(editor: &ForgeEditor, row: usize) -> Vec<TierOption> {
    editor
        .tier_options(row)
        .into_iter()
        .map(|(tier, range, in_game)| TierOption {
            label: tr::Tier.with(&[("tier", &tier)]).into(),
            range: range.into(),
            in_game,
        })
        .collect()
}

fn affix_row(editor: &ForgeEditor, index: usize, row: &AffixRow) -> AffixRowView {
    AffixRowView {
        value: value_cell(
            reference_of(Cell::Affix(index)),
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
        range: editor.range_text(index).into(),
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
        tiers: ModelRc::default(),
    }
}

/// The implicit or unique lines, each with its roll cells apart.
fn lines(editor: &ForgeEditor, kind: CellKind) -> Lines {
    let rows = if kind == CellKind::Unique { &editor.uniques } else { &editor.implicits };
    rows.iter()
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
            let row = ModifierRowView {
                rolls: ModelRc::default(),
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
            };
            (row, cells)
        })
        .collect()
}

fn draft_rows(editor: &ForgeEditor) -> Vec<DraftEntryView> {
    editor
        .drafts()
        .into_iter()
        .map(|entry| DraftEntryView {
            name: entry.name.into(),
            summary: entry.summary.into(),
            effect: entry.effect.into(),
            stale: entry.stale,
        })
        .collect()
}

fn history_rows(history: &Tracked<Vec<AppliedEntry>>) -> Vec<AppliedEntryView> {
    history
        .iter()
        .map(|entry| AppliedEntryView {
            name: entry.name.clone().into(),
            time: entry.time.clone().into(),
            summary: entry.summary.clone().into(),
            effect: entry.effect.clone().into(),
            outcome: entry.outcome.clone().into(),
        })
        .collect()
}

/// The picker as one value: Slint passes it on only when something in it changed, and its lists are the same models each
/// time, so an unchanged list keeps its rows.
fn show_picker(forge: &ForgeState, editor: &ForgeEditor, drafted: Option<&ForgeItem>, regions: &mut Regions, models: &mut Models) {
    let Some(picker) = editor.picker.as_ref() else {
        forge.set_picker(PickerView {
            open: false,
            ..forge.get_picker()
        });
        return;
    };
    let filters = &picker.filters;
    // Choosing an entry shows again with the same list; its rows are rebuilt only when the list changes.
    let listed = editor.picker_indices();
    let base = editor.value_base();
    regions.picker.update((editor.menu_generation(), listed.to_vec(), filters.slot, base), || {
        models.picker_rows.replace(Listed {
            indices: listed.to_vec(),
            catalog: Some(editor.shared_catalog()),
            slot: filters.slot,
            own_types: picker.own_types.clone(),
            base,
        });
    });
    let empty = editor.picker_empty();
    let mut view = PickerView {
        open: true,
        title: editor.picker_title().into(),
        subtitle: editor.picker_subtitle_of(drafted).into(),
        search: picker.search.clone().into(),
        sides: models.picker_sides.model(),
        side: match filters.side {
            KindFilter::All => 0,
            KindFilter::Prefixes => 1,
            KindFilter::Suffixes => 2,
        },
        categories: models.picker_categories.model(),
        category: filters.category as i32,
        classes: models.picker_classes.model(),
        class: filters.class as i32,
        allow: picker.allow_incompatible,
        allow_enabled: editor.can_mutate(),
        count: picker::count_text(listed.len()).into(),
        hidden: if picker.hidden_uniques > 0 {
            picker::hidden_text(picker.hidden_uniques).into()
        } else {
            SharedString::new()
        },
        empty_reason: empty.as_ref().map_or_else(SharedString::new, |empty| empty.reason.as_str().into()),
        empty_clear_search: empty.as_ref().is_some_and(|empty| empty.clear_search),
        empty_clear_filters: empty.as_ref().is_some_and(|empty| empty.clear_filters),
        empty_allow: empty.as_ref().is_some_and(|empty| empty.allow),
        rows: ModelRc::from(models.picker_rows.clone()),
        selected: picker
            .selected
            .and_then(|id| editor.picker_list().position(|definition| definition.id == id))
            .map_or(-1, |index| index as i32),
        status: editor.picker_status().into(),
        action: editor.picker_action().into(),
        can_use: editor.can_use_picker(),
        ..PickerView::default()
    };
    show_picker_choices(&mut view, editor, picker, models);
    forge.set_picker(view);
}

/// The filter bar's choices and what each holds. The models are updated in place, so a list open over the picker survives a
/// change.
fn show_picker_choices(view: &mut PickerView, editor: &ForgeEditor, picker: &Picker, models: &Models) {
    let filters = &picker.filters;
    let shared = |text: &str| SharedString::from(text);
    let catalog = editor.catalog();

    let slots = editor.picker_slots();
    let mut choices = vec![shared(picker::AnySlot.get())];
    choices.extend(slots.iter().map(|(_, name)| shared(name)));
    models.picker_slots.sync(choices);
    view.slots = models.picker_slots.model();
    view.slot = filters
        .slot
        .map_or(0, |slot| slots.iter().position(|(id, _)| *id == slot).map_or(-1, |index| index as i32 + 1));
    view.slot_text = filters.slot.map_or_else(SharedString::new, |slot| picker::slot_name(catalog, slot).into());
    view.slot_active = filters.slot != picker.own_slot();

    let levels = picker.level_choices();
    models.picker_levels.sync(levels.iter().map(|(_, text)| shared(text)));
    view.levels = models.picker_levels.model();
    view.level = levels.iter().position(|(level, _)| *level == filters.level).map_or(0, |index| index as i32);

    let mut sources: Vec<SharedString> = picker::source_words().iter().map(|word| shared(word)).collect();
    sources.extend(picker.source_uniques.iter().map(|(_, name)| shared(name)));
    models.picker_sources.sync(sources);
    view.sources = models.picker_sources.model();
    let source = filters.effective_source();
    view.source = match source {
        SourceChoice::Game => 0,
        SourceChoice::Uniques => 1,
        SourceChoice::All => 2,
        SourceChoice::Unique(unique) => picker.source_uniques.iter().position(|(id, _)| *id == unique).map_or(-1, |index| index as i32 + 3),
    };
    view.source_text = if let SourceChoice::Unique(unique) = source {
        catalog.source_name(unique).into()
    } else {
        SharedString::new()
    };
    view.source_active = source != SourceChoice::Game;

    let tags = editor.picker_tags();
    models.picker_tags.sync(tags.iter().map(|tag| shared(picker::STAT_TAGS[*tag].name.get())));
    models.picker_tags_on.sync(tags.iter().map(|tag| filters.tags & (1u64 << tag) != 0));
    view.tags = models.picker_tags.model();
    view.tags_on = models.picker_tags_on.model();
    view.tags_text = picker::tags_summary(filters).into();
    view.tags_active = filters.tags != 0;

    models.picker_chips.sync(editor.picker_chips().into_iter().map(|(_, text)| SharedString::from(text)));
    view.chips = models.picker_chips.model();
}

/// Facts shown side by side, each its own text.
fn facts(parts: &[String]) -> Vec<SharedString> {
    parts.iter().map(|part| part.as_str().into()).collect()
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
