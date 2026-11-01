//! What the view's callbacks mean as intents, and the names of what never changes.

use super::show::cell_of;
use crate::ForgeState;
use eg_trainer::screens::forge::feature::{Forge, Intent, State};
use eg_trainer::screens::forge::item::Seal;
use eg_trainer::screens::forge::picker::{self, KindFilter};
use slint::{Model, ModelRc, SharedString, VecModel};
use spiderland::{Store, callbacks};
use std::rc::Rc;

fn index(value: i32) -> usize {
    value.max(0) as usize
}

pub(super) fn connect(forge: &ForgeState, store: &Store<Forge>) {
    /// Sends what a callback means and answers the view with what the reducer said; `$gone` while it can't be asked.
    macro_rules! ask {
        ($on:ident, $gone:expr, || $intent:expr) => {{
            let store = store.clone();
            forge.$on(move || {
                store.send($intent);
                store.try_read(State::answer).unwrap_or($gone)
            });
        }};
        ($on:ident, $gone:expr, |$($arg:ident),*| $intent:expr) => {{
            let store = store.clone();
            forge.$on(move |$($arg),*| {
                store.send($intent);
                store.try_read(State::answer).unwrap_or($gone)
            });
        }};
    }
    callbacks!(forge, store, {
        on_select_affix: |row| Intent::SelectAffix(usize::try_from(row).ok()),
        on_select_unique: |row| Intent::SelectUnique(usize::try_from(row).ok()),
        on_press: |cell| Intent::Press(cell_of(&cell)),
        on_scrub: |cell, fraction| Intent::Scrub(cell_of(&cell), f64::from(fraction)),
        on_end_scrub: |cell, cancelled| Intent::EndScrub(cell_of(&cell), cancelled),
        on_step: |cell, steps| Intent::Step(cell_of(&cell), steps),
        on_step_to_end: |cell, maximum| Intent::StepToEnd(cell_of(&cell), maximum),
        on_input_edited: |cell, text| Intent::InputEdited(cell_of(&cell), text.into()),
        on_cancel_input: |cell| Intent::CancelInput(cell_of(&cell)),
        on_finish_input: |cell| Intent::FinishInput(cell_of(&cell)),
        on_pick_tier: |row, tier| Intent::PickTier(index(row), (tier + 1).clamp(1, 8) as u8),
        on_pick_seal: |row, seal| Intent::PickSeal(index(row), Seal::ALL[seal.clamp(0, 3) as usize]),
        on_toggle_flag: |row, on| Intent::ToggleFlag(index(row), on),
        on_pick_rarity: |choice| Intent::PickRarity(choice == 1),
        on_pick_base: |choice| Intent::PickBase(index(choice)),
        on_pick_identity: |choice| Intent::PickIdentity(index(choice)),
        on_identity_search_edited: |text| Intent::IdentitySearch(text.into()),
        on_base_allow: |allow| Intent::BaseAllow(allow),
        on_picker_search_edited: |text| Intent::PickerSearch(text.into()),
        on_picker_filter_changed: |filter| Intent::PickerFilter(match filter {
        1 => KindFilter::Prefixes,
        2 => KindFilter::Suffixes,
        _ => KindFilter::All,
    }),
        on_picker_allow_changed: |allow| Intent::PickerAllow(allow),
        on_picker_category_changed: |choice| Intent::PickerCategory(index(choice)),
        on_picker_class_changed: |choice| Intent::PickerClass(index(choice)),
        on_picker_slot_changed: |choice| Intent::PickerSlot(index(choice)),
        on_picker_level_changed: |choice| Intent::PickerLevel(index(choice)),
        on_picker_source_changed: |choice| Intent::PickerSource(index(choice)),
        on_picker_tag_toggled: |choice, on| Intent::PickerTag(index(choice), on),
        on_picker_chip_removed: |chip| Intent::PickerRemoveChip(index(chip)),
        on_picker_clear_filters: || Intent::PickerClearFilters,
        on_picker_show_uniques: || Intent::PickerShowUniques,
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

    let names = |names: &mut dyn Iterator<Item = &str>| ModelRc::from(Rc::new(VecModel::from(names.map(SharedString::from).collect::<Vec<_>>())));
    forge.set_picker_sides(names(&mut picker::PICKER_SIDES.iter().map(|side| side.get())));
    forge.set_picker_categories(names(&mut picker::PICKER_CATEGORIES.iter().map(|(name, _)| name.get())));
    forge.set_picker_classes(names(&mut picker::PICKER_CLASSES.iter().map(|class| class.get())));
}
