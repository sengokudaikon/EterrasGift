//! What the view's callbacks mean as intents, and the names of what never changes.

use super::show::cell_of;
use crate::{CellAction, ForgeState, PickerFilter};
use eg_trainer::screens::forge::editor::Cell;
use eg_trainer::screens::forge::feature::{Forge, Intent, State};
use eg_trainer::screens::forge::item::Seal;
use eg_trainer::screens::forge::picker::KindFilter;
use slint::Model;
use spiderland::{Store, callbacks};

fn index(value: i32) -> usize {
    value.max(0) as usize
}

pub(super) fn connect(forge: &ForgeState, store: &Store<Forge>) {
    /// Sends what a callback means and answers the view with what the reducer said; `$gone` while it can't be asked.
    macro_rules! ask {
        ($on:ident, $gone:expr, || $intent:expr) => {{
            let store = store.clone();
            forge.$on(move || store.ask($intent, State::answer).unwrap_or($gone));
        }};
        ($on:ident, $gone:expr, |$($arg:ident),*| $intent:expr) => {{
            let store = store.clone();
            forge.$on(move |$($arg),*| store.ask($intent, State::answer).unwrap_or($gone));
        }};
    }
    callbacks!(forge, store, {
        on_select_affix: |row| Intent::SelectAffix(usize::try_from(row).ok()),
        on_select_unique: |row| Intent::SelectUnique(usize::try_from(row).ok()),
        on_cell_acted: |cell, action, amount| cell_action(cell_of(&cell), action, amount),
        on_input_edited: |cell, text| Intent::InputEdited(cell_of(&cell), text.into()),
        on_pick_tier: |row, tier| Intent::PickTier(index(row), (tier + 1).clamp(1, 8) as u8),
        on_pick_seal: |row, seal| Intent::PickSeal(index(row), Seal::ALL[seal.clamp(0, 3) as usize]),
        on_toggle_flag: |row, on| Intent::ToggleFlag(index(row), on),
        on_pick_rarity: |choice| Intent::PickRarity(choice == 1),
        on_pick_base: |choice| Intent::PickBase(index(choice)),
        on_pick_identity: |choice| Intent::PickIdentity(index(choice)),
        on_identity_search_edited: |text| Intent::IdentitySearch(text.into()),
        on_base_allow: |allow| Intent::BaseAllow(allow),
        on_picker_search_edited: |text| Intent::PickerSearch(text.into()),
        on_picker_filter_changed: |filter, choice, on| picker_filter(filter, choice, on),
        on_picker_select: |entry| Intent::PickerSelect(index(entry)),
        on_use_picker: || Intent::UsePicker,
        on_cancel_picker: || Intent::CancelPicker,
        on_discard_row: |row| Intent::DiscardRow(index(row)),
        on_remove_affix: |row| Intent::RemoveAffix(index(row)),
        on_discard_entry: |entry| Intent::DiscardEntry(index(entry)),
        on_discard_all: || Intent::DiscardAll,
        on_max_tiers: || Intent::MaxTiers,
        on_max_rolls: || Intent::MaxRolls,
        on_templates_opened: || Intent::TemplatesOpened,
        on_template_name_edited: |name| Intent::TemplateName(name.into()),
        on_template_save: |name, whole| Intent::TemplateSave(name.into(), whole),
        on_template_apply: |template| Intent::TemplateApply(index(template)),
        on_template_delete: |template| Intent::TemplateDelete(index(template)),
        on_apply: || Intent::Apply,
        on_undo: || Intent::Undo,
        on_undo_draft: || Intent::UndoDraft,
        on_move_row: |row, delta, part| Intent::MoveRow { row: index(row), delta, part },
        on_move_unique: |row, delta, roll| Intent::MoveUnique {
        row: index(row),
        delta,
        roll: index(roll),
    },
        on_move_implicit: |row, delta, roll| Intent::MoveImplicit {
        row: index(row),
        delta,
        roll: index(roll),
    },
        on_show_draft: |entry| Intent::ShowDraft(index(entry)),
    });
    ask!(on_begin_input, false, |cell| Intent::BeginInput(cell_of(&cell)));
    // A window already gone has no popover to keep open.
    ask!(on_commit_input, true, |cell| Intent::CommitInput(cell_of(&cell)));
    ask!(on_open_replace, false, |row| Intent::OpenReplace(index(row)));
    ask!(on_open_add, false, || Intent::OpenAdd);

    // A menu's keyboard highlight starts on the held entry and skips the entries that can't be picked. These read only the
    // list they are handed, so they need no state.
    forge.on_option_start(|options| {
        let available = |index: usize| options.row_data(index).is_some_and(|option| option.available);
        (0..options.row_count())
            .find(|&index| available(index) && options.row_data(index).is_some_and(|option| option.current))
            .or_else(|| (0..options.row_count()).find(|&index| available(index)))
            .map_or(-1, |index| index as i32)
    });
    forge.on_option_step(|options, from, delta| {
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

/// What a value cell's gesture asks: `amount` is a drag's share of the cell's span, or a number of steps.
fn cell_action(cell: Cell, action: CellAction, amount: f32) -> Intent {
    match action {
        CellAction::Press => Intent::Press(cell),
        CellAction::Scrub => Intent::Scrub(cell, f64::from(amount)),
        CellAction::EndScrub => Intent::EndScrub(cell, false),
        CellAction::CancelScrub => Intent::EndScrub(cell, true),
        CellAction::Step => Intent::Step(cell, amount.round() as i32),
        CellAction::ToStart => Intent::StepToEnd(cell, false),
        CellAction::ToEnd => Intent::StepToEnd(cell, true),
        CellAction::CancelInput => Intent::CancelInput(cell),
        CellAction::FinishInput => Intent::FinishInput(cell),
    }
}

/// What a change to the picker's filters asks: a choice box's `choice`, a stat tag's or "Allow incompatible affixes"' `on`.
fn picker_filter(filter: PickerFilter, choice: i32, on: bool) -> Intent {
    match filter {
        PickerFilter::Side => Intent::PickerFilter(match choice {
            1 => KindFilter::Prefixes,
            2 => KindFilter::Suffixes,
            _ => KindFilter::All,
        }),
        PickerFilter::Category => Intent::PickerCategory(index(choice)),
        PickerFilter::Class => Intent::PickerClass(index(choice)),
        PickerFilter::Slot => Intent::PickerSlot(index(choice)),
        PickerFilter::Level => Intent::PickerLevel(index(choice)),
        PickerFilter::Source => Intent::PickerSource(index(choice)),
        PickerFilter::Tag => Intent::PickerTag(index(choice), on),
        PickerFilter::Allow => Intent::PickerAllow(on),
        PickerFilter::Chip => Intent::PickerRemoveChip(index(choice)),
        PickerFilter::Clear => Intent::PickerClearFilters,
        PickerFilter::Uniques => Intent::PickerShowUniques,
    }
}
