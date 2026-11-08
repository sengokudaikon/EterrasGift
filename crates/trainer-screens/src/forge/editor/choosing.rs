//! The affix picker: its filters, its list and what using an entry does.

use super::*;

impl ForgeEditor {
    pub(super) fn slot_state(&self) -> Option<item::AffixState> {
        self.selected_affix.and_then(|index| self.affixes.get(index)).map(|row| row.state.clone())
    }

    pub(super) fn compatibility_of(&self, definition: &AffixDefinition, slot: Option<&item::AffixState>) -> Compatibility {
        match self.item.as_ref() {
            Some(item) => self.game.compatibility(item, definition, slot, self.unrestricted()),
            None => Compatibility::Unknown,
        }
    }

    /// Opens the picker to replace a row's affix: a fresh search, the default filters, nothing chosen. An added affix is
    /// chosen again by removing it and adding another; one Apply removes has nothing to replace.
    pub fn open_replace(&mut self, row: usize) -> bool {
        self.select_affix(Some(row));
        if !self.can_change_affix() || !self.affixes.get(row).is_some_and(|row| !row.added && row.is_editable()) {
            return false;
        }
        self.open_picker(false);
        true
    }

    pub fn open_add(&mut self) -> bool {
        if !self.can_change_affix_list() {
            return false;
        }
        self.open_picker(true);
        true
    }

    pub(super) fn open_picker(&mut self, adding: bool) {
        let (own_types, item_level) = self.picker_item();
        *self.picker = Some(Picker {
            adding,
            filters: Filters::for_slot(own_types.first().copied()),
            own_types,
            item_level,
            ..Picker::default()
        });
        self.filter_picker(true);
    }

    /// The item's slots, its own first, and the level it requires.
    pub(super) fn picker_item(&self) -> (Vec<u16>, Option<u8>) {
        let Some(item) = self.item.as_ref() else {
            return (Vec::new(), None);
        };
        let shared = u8::try_from(item.item_type).map_or(&[][..], |item_type| eg_trainer::item::edit::shared_slots(item_type, item.sub_type));
        let own = std::iter::once(item.item_type).chain(shared.iter().filter_map(|slot| u16::try_from(*slot).ok())).collect();
        (own, item.status.map(|status| status.level_requirement))
    }

    pub fn cancel_picker(&mut self) {
        *self.picker = None;
    }

    pub fn picker_search(&mut self, search: &str) {
        if let Some(picker) = self.picker.as_mut() {
            picker.search = search.into();
        }
        self.filter_picker(false);
    }

    /// Changes the filters and lists again; `slot_changed` also takes Source's uniques again, which follow the slot.
    pub(super) fn change_filters(&mut self, change: impl FnOnce(&mut Picker)) {
        if let Some(picker) = self.picker.as_mut() {
            let slot = picker.filters.slot;
            change(picker);
            if picker.filters.slot != slot {
                self.refresh_source_uniques();
            }
        }
        self.filter_picker(false);
    }

    pub(super) fn refresh_source_uniques(&mut self) {
        let definitions = &self.game.catalog().definitions;
        if let Some(picker) = self.picker.as_mut() {
            picker.source_uniques = picker::source_uniques(&picker.candidates, definitions, picker.filters.slot, &picker.own_types);
        }
    }

    pub fn picker_filter(&mut self, filter: KindFilter) {
        self.change_filters(|picker| picker.filters.side = filter);
    }

    pub fn picker_category(&mut self, choice: usize) {
        self.change_filters(|picker| picker.filters.category = choice.min(PICKER_CATEGORIES.len() - 1));
    }

    pub fn picker_class(&mut self, choice: usize) {
        self.change_filters(|picker| picker.filters.class = choice.min(PICKER_CLASSES.len() - 1));
    }

    /// Slot's choice: 0 any slot, then the index's slots in order.
    pub fn picker_slot(&mut self, choice: usize) {
        let slot = choice.checked_sub(1).and_then(|index| self.picker_index.slots.get(index)).map(|(slot, _)| *slot);
        self.change_filters(|picker| picker.filters.slot = slot);
    }

    pub fn picker_level(&mut self, choice: usize) {
        self.change_filters(|picker| {
            if let Some((level, _)) = picker.level_choices().get(choice) {
                picker.filters.level = *level;
            }
        });
    }

    /// Source's choice: the three fixed ones, then the uniques it lists.
    pub fn picker_source(&mut self, choice: usize) {
        self.change_filters(|picker| {
            let source = match choice {
                0 => SourceChoice::Game,
                1 => SourceChoice::Uniques,
                2 => SourceChoice::All,
                unique => match picker.source_uniques.get(unique - 3) {
                    Some((unique, _)) => SourceChoice::Unique(*unique),
                    None => return,
                },
            };
            picker.filters.set_source(source);
        });
    }

    /// Turns a stat tag on or off, by its place among the tags in use.
    pub fn picker_tag(&mut self, choice: usize, on: bool) {
        let Some(tag) = self.picker_index.tags.get(choice).copied() else {
            return;
        };
        self.change_filters(|picker| {
            if on {
                picker.filters.tags |= 1u64 << tag;
            } else {
                picker.filters.tags &= !(1u64 << tag);
            }
        });
    }

    /// Clears the filter behind an active-filter chip, by its place in the row.
    pub fn picker_remove_chip(&mut self, chip: usize) {
        self.change_filters(|picker| {
            let own = picker.own_slot();
            if let Some(found) = picker::chips(&picker.filters, own).get(chip).copied() {
                picker::clear_chip(&mut picker.filters, found, own);
            }
        });
    }

    pub fn picker_clear_filters(&mut self) {
        self.change_filters(|picker| picker.filters = Filters::for_slot(picker.own_slot()));
    }

    /// Lists the unique modifiers the default Source holds back, as well as what it lists.
    pub fn picker_show_uniques(&mut self) {
        self.change_filters(|picker| picker.filters.set_source(SourceChoice::All));
    }

    /// What the view shows for each active filter, in the row's order.
    pub fn picker_chips(&self) -> Vec<(Chip, String)> {
        let Some(picker) = self.picker.as_ref() else {
            return Vec::new();
        };
        picker::chips(&picker.filters, picker.own_slot())
            .into_iter()
            .map(|chip| (chip, picker::chip_text(chip, &picker.filters, self.catalog())))
            .collect()
    }

    /// The slots Slot offers, by the index's order.
    pub fn picker_slots(&self) -> &[(u16, String)] {
        &self.picker_index.slots
    }

    /// The stat tags in use, by their place among [`picker::STAT_TAGS`].
    pub fn picker_tags(&self) -> &[usize] {
        &self.picker_index.tags
    }

    /// Why an open picker lists nothing, and the ways out; none while it lists something.
    pub fn picker_empty(&self) -> Option<picker::Empty> {
        let picker = self.picker.as_ref()?;
        if !picker.list.is_empty() {
            return None;
        }
        let search = picker.search.to_lowercase();
        let query = picker.query(&search);
        let definitions = &self.catalog().definitions;
        // Only an empty list asks, so this rare pass over the whole catalog costs nothing while typing.
        let more_if_allowed = !picker.allow_incompatible && {
            let all: Vec<usize> = (0..definitions.len()).collect();
            !picker::run(&query, &all, definitions, &self.picker_index.keys).list.is_empty()
        };
        Some(picker::empty(&query, picker.own_slot(), picker.candidates.len(), more_if_allowed, self.unrestricted()))
    }

    /// Every tier's value range of the picked affix; empty where the game data has none.
    pub(super) fn picker_ladder(&self) -> String {
        self.picked()
            .map_or_else(String::new, |definition| picker::ladder(definition, self.catalog(), self.value_base()))
    }

    /// "Allow incompatible affixes" is for what this slot can't roll, so it opens the Slot filter while that sits on its
    /// default, and closing it puts the default back.
    pub fn picker_allow(&mut self, allow: bool) {
        if let Some(picker) = self.picker.as_mut() {
            picker.allow_incompatible = allow;
            let own = picker.own_slot();
            if allow && picker.filters.slot == own {
                picker.filters.slot = None;
            } else if !allow && picker.filters.slot.is_none() {
                picker.filters.slot = own;
            }
        }
        self.filter_picker(true);
    }

    pub fn picker_select(&mut self, index: usize) {
        let definitions = &self.game.catalog().definitions;
        if let Some(picker) = self.picker.as_mut() {
            picker.selected = picker.list.get(index).and_then(|&index| definitions.get(index)).map(|definition| definition.id);
        }
    }

    /// What the open picker lists, as indices into the catalog's definitions: the list changed when these did.
    pub fn picker_indices(&self) -> &[usize] {
        self.picker.as_ref().map_or(&[], |picker| picker.list.as_slice())
    }

    /// The definitions the open picker lists, in order.
    pub fn picker_list(&self) -> impl Iterator<Item = &AffixDefinition> {
        let definitions = &self.catalog().definitions;
        self.picker.iter().flat_map(|picker| picker.list.iter()).filter_map(|&index| definitions.get(index))
    }

    /// Lists the definitions the search and the filters leave of the candidates; `rejudge` first judges again which
    /// definitions fit the item, as when the picker opens.
    pub(super) fn filter_picker(&mut self, rejudge: bool) {
        if rejudge {
            // The item can change under an open picker; a Slot on its default follows it.
            let (own_types, item_level) = self.picker_item();
            if let Some(picker) = self.picker.as_mut() {
                if picker.filters.slot == picker.own_slot() {
                    picker.filters.slot = own_types.first().copied();
                }
                picker.own_types = own_types;
                picker.item_level = item_level;
            }
        }
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let definitions = &self.game.catalog().definitions;
        if self.picker_index.keys.len() != definitions.len() {
            self.picker_index = Index::build(self.game.catalog());
        }
        let candidates = if rejudge {
            let slot = if picker.adding { None } else { self.slot_state() };
            // An addition joins the item as the other drafts leave it; a replacement takes the game's slot.
            let drafted = if picker.adding { self.drafted_item() } else { None };
            let item = if picker.adding { drafted.as_ref() } else { self.item.as_ref() };
            let unrestricted = self.unrestricted();
            let compatible = |definition: &AffixDefinition| item.is_some_and(|item| self.game.compatibility(item, definition, slot.as_ref(), unrestricted).is_allowed());
            let mut candidates: Vec<usize> = (0..definitions.len())
                .filter(|&index| picker.allow_incompatible || compatible(&definitions[index]))
                .collect();
            candidates.sort_by(|a, b| definitions[*a].name.cmp(&definitions[*b].name));
            Some(candidates)
        } else {
            None
        };
        let search = picker.search.to_lowercase();
        let outcome = picker::run(
            &picker.query(&search),
            candidates.as_ref().unwrap_or(&picker.candidates),
            definitions,
            &self.picker_index.keys,
        );
        let selected = picker.selected.filter(|id| outcome.list.iter().any(|&index| definitions[index].id == *id));
        let rejudged = candidates.is_some();
        let picker = self.picker.as_mut().expect("the picker is open");
        if let Some(candidates) = candidates {
            picker.candidates = candidates;
        }
        picker.selected = selected;
        picker.list = outcome.list;
        picker.hidden_uniques = outcome.hidden_uniques;
        if rejudged {
            self.refresh_source_uniques();
        }
    }

    pub fn picker_title(&self) -> &'static str {
        phrases! { ReplaceAffix: "Replace affix" }
        if self.picker.as_ref().is_some_and(|picker| picker.adding) {
            tr::AddAffix.get()
        } else {
            ReplaceAffix.get()
        }
    }

    pub fn picker_subtitle(&self) -> String {
        self.picker_subtitle_of(self.drafted_item().as_ref())
    }

    /// `picker_subtitle` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn picker_subtitle_of(&self, drafted: Option<&ForgeItem>) -> String {
        match (&*self.picker, self.selected_affix.and_then(|index| self.affixes.get(index))) {
            (Some(picker), _) if picker.adding => self.free_slots_text_of(drafted),
            (_, Some(row)) => {
                phrases! { Currently: "Currently {name}" }
                Currently.with(&[("name", &row.original_name())])
            }
            _ => String::new(),
        }
    }

    pub fn picker_action(&self) -> &'static str {
        phrases! { UseAffix: "Use this affix" }
        if self.picker.as_ref().is_some_and(|picker| picker.adding) {
            tr::AddAffix.get()
        } else {
            UseAffix.get()
        }
    }

    pub(super) fn picked(&self) -> Option<&AffixDefinition> {
        let selected = self.picker.as_ref()?.selected?;
        self.picker_list().find(|definition| definition.id == selected)
    }

    pub(super) fn replacement_problem(&self, compatibility: Compatibility, allow_incompatible: bool) -> Option<String> {
        if !compatibility.allows_edit(allow_incompatible) {
            phrases! { ReplacementIncompatible: "This affix is incompatible with the selected item or slot." }
            return Some(if compatibility == Compatibility::Unknown {
                tr::Unconfirmed.into()
            } else {
                ReplacementIncompatible.into()
            });
        }
        (!compatibility.is_allowed() && !self.unrestricted()).then(|| tr::OverrideRequired.into())
    }

    /// The addition the picker drafts for a definition, to the item as the other drafts leave it, or why not.
    /// It starts at tier 5 (or the affix's highest below it) and the middle roll, corrupted where the affix is. An item
    /// holds one corrupted affix: a corrupted affix takes that slot, and the row holding it (`Some(row)`) is drafted back
    /// to a normal affix first.
    pub(super) fn add_edit(&self, definition: &AffixDefinition) -> Result<(ItemEdit, Option<usize>), String> {
        let Some(item) = self.drafted_item() else {
            phrases! { Unverified: "This affix has no verified definition in the current build." }
            return Err(Unverified.into());
        };
        let corrupted = definition.category == Category::Corrupted;
        let seal = if corrupted { Seal::Corrupted } else { Seal::Normal };
        let allow = self.picker.as_ref().is_some_and(|picker| picker.allow_incompatible);
        let tier = 5.min(definition.max_tier.clamp(1, 8));
        let held = if corrupted {
            self.affixes.iter().position(|row| !row.is_removed() && row.seal() == Seal::Corrupted)
        } else {
            None
        };
        let item = match (held, item.affixes.iter().find(|affix| affix.seal == Seal::Corrupted)) {
            (Some(row), Some(affix)) => ItemEdit::Affix(eg_trainer::item::edit::AffixEdit {
                slot: affix.slot as i32,
                id: i32::from(affix.id),
                tier: i32::from(affix.tier),
                roll: i32::from(affix.roll),
                seal: Seal::Normal,
                allow_incompatible: false,
            })
            .project(&item)
            .map_err(|problem| {
                phrases! { CannotFree: "{name} cannot become a normal affix to free the corrupted slot: {problem}" }
                CannotFree.with(&[("name", &self.affixes[row].name()), ("problem", &problem)])
            })?,
            _ => item,
        };
        self.addition(&item, definition, tier, 128, seal, allow).map(|edit| (edit, held))
    }

    /// The edit that adds an affix to `item`, or why it may not. Where the game would not roll it, or past the free slots
    /// (a sealed or corrupted affix takes none), it needs "I know what I'm doing", and an incompatible one the picker's
    /// "Allow incompatible affixes" too.
    pub(super) fn addition(&self, item: &ForgeItem, definition: &AffixDefinition, tier: u8, roll: u8, seal: Seal, allow_incompatible: bool) -> Result<ItemEdit, String> {
        if !definition.writable {
            return Err(tr::Unwritable.into());
        }
        let compatibility = self.game.compatibility(item, definition, None, self.unrestricted());
        let (prefixes, suffixes) = item.free_slots();
        let slot_free = seal != Seal::Normal || (if definition.kind == AffixKind::Prefix { prefixes } else { suffixes }) > 0;
        let needs_override = compatibility != Compatibility::Allowed || !slot_free;
        let allow = allow_incompatible && self.unrestricted();
        if !compatibility.allows_edit(allow) {
            return Err(if self.unrestricted() { tr::Incompatible.into() } else { tr::NotAddedLocked.into() });
        }
        if needs_override && !self.unrestricted() {
            const NO_FREE_SLOT: [Phrase; 2] = [
                Phrase::new("No free prefix slot. Adding more needs \"I know what I'm doing\" in Settings."),
                Phrase::new("No free suffix slot. Adding more needs \"I know what I'm doing\" in Settings."),
            ];
            return Err(if slot_free {
                tr::NotAddedLocked.into()
            } else {
                NO_FREE_SLOT[definition.kind as usize].into()
            });
        }
        let edit = ItemEdit::AddAffix(eg_trainer::item::edit::AddAffixEdit {
            id: (definition.id) as i32,
            tier: i32::from(tier),
            roll: i32::from(roll),
            seal,
            kind: definition.kind,
            category: Some((definition.category).as_raw()),
            allow_incompatible: needs_override,
        });
        edit.prepare_view(item).map_err(|e| e.to_string())?;
        Ok(edit)
    }

    /// The picker's status caption for the chosen entry.
    /// The picked affix's verdict, then its tier ladder.
    pub fn picker_status(&self) -> String {
        let verdict = self.picker_verdict();
        let ladder = self.picker_ladder();
        if ladder.is_empty() || verdict.is_empty() {
            verdict
        } else {
            format!("{verdict}\n{ladder}")
        }
    }

    pub(super) fn picker_verdict(&self) -> String {
        let (Some(picker), Some(definition)) = (self.picker.as_ref(), self.picked()) else {
            return String::new();
        };
        if picker.adding {
            return match self.add_edit(definition) {
                Ok((edit, held)) => {
                    phrases! {
                        Overridden: "Added by \"I know what I'm doing\". The game would not add it here."
                        Fits: "Fits this item."
                        TakesSlot: "{fits} It takes the corrupted slot: {name} becomes a normal affix."
                    }
                    let fits: String = if edit.needs_override() { Overridden.into() } else { Fits.into() };
                    match held {
                        Some(row) => TakesSlot.with(&[("fits", &fits), ("name", &self.affixes[row].name())]),
                        None => fits,
                    }
                }
                Err(problem) => problem,
            };
        }
        let Some(slot) = self.slot_state() else {
            phrases! { SelectSlot: "Select an affix slot." }
            return SelectSlot.into();
        };
        phrases! {
            AllowedByOverride: "Allowed by \"I know what I'm doing\". The game doesn't roll it here."
            Compatible: "Compatible with this slot"
            SlotType: "Incompatible: this affix belongs in the other slot (prefix or suffix)."
            AlreadyPresent: "Incompatible: the item already has this affix."
            ClassRequirement: "Incompatible: this affix requires another class."
            EquipmentType: "Incompatible: this affix rolls on another kind of item."
            SpecialCategory: "Incompatible: this kind of affix (experimental, personal, set) stays in its own slots."
            IncompleteData: "The game data is incomplete for this affix, so where it rolls can't be checked."
        }
        match self.compatibility_of(definition, Some(&slot)) {
            Compatibility::Override => AllowedByOverride.into(),
            Compatibility::Allowed => Compatible.into(),
            Compatibility::Rejected(_) if !self.unrestricted() => tr::IncompatibleLocked.into(),
            Compatibility::Rejected(reason) => match reason {
                item::Rejection::SlotType => SlotType.into(),
                item::Rejection::AlreadyPresent => AlreadyPresent.into(),
                item::Rejection::ClassRequirement => ClassRequirement.into(),
                item::Rejection::EquipmentType => EquipmentType.into(),
                item::Rejection::SpecialCategory => SpecialCategory.into(),
                item::Rejection::IncompleteData => IncompleteData.into(),
            },
            Compatibility::Unknown if !self.unrestricted() => tr::UnconfirmedLocked.into(),
            Compatibility::Unknown => tr::Unconfirmed.into(),
        }
    }

    pub fn can_use_picker(&self) -> bool {
        let (Some(picker), Some(definition)) = (self.picker.as_ref(), self.picked()) else {
            return false;
        };
        if picker.adding {
            return self.can_change_affix_list() && self.add_edit(definition).is_ok();
        }
        self.can_mutate()
            && self.slot_state().is_some_and(|slot| {
                self.replacement_problem(self.compatibility_of(definition, Some(&slot)), picker.allow_incompatible)
                    .is_none()
            })
    }

    /// Uses the chosen entry: a replacement drafts the row's affix; an addition drafts a row of its own after the game's.
    pub fn use_picker(&mut self) {
        if !self.can_use_picker() {
            return;
        }
        let (Some((adding, allow_incompatible)), Some(definition)) = (self.picker.as_ref().map(|picker| (picker.adding, picker.allow_incompatible)), self.picked().cloned()) else {
            return;
        };
        if adding {
            let addition = self.add_edit(&definition);
            *self.picker = None;
            if let Ok((ItemEdit::AddAffix(add), held)) = addition {
                if let Some(row) = held {
                    let affix = &mut self.affixes[row];
                    let tier = affix.tier();
                    if affix.draft_tier(tier, Seal::Normal) {
                        let uid = affix.uid;
                        self.edited(RowRef::Affix(uid));
                    }
                }
                let uid = self.uid();
                self.affixes
                    .push(AffixRow::added(uid, definition, add.tier as u8, add.roll as u8, add.seal, allow_incompatible));
                self.selected_unique = None;
                self.selected_affix = Some(self.affixes.len() - 1);
                self.edited(RowRef::Affix(uid));
                self.resolve_tables();
            }
            return;
        }
        *self.picker = None;
        if let Some(index) = self.selected_affix
            && self.affixes[index].select_definition(definition, allow_incompatible)
        {
            let uid = self.affixes[index].uid;
            self.edited(RowRef::Affix(uid));
            self.resolve_tables();
        }
    }

    /// The row menu's "Remove affix" and Del: a draft, which Apply writes with the others; the same entry ("Keep affix"),
    /// Ctrl+Z or discarding it keeps the affix. On an added affix it drops the addition.
    pub fn remove_affix(&mut self, row: usize) {
        if !self.can_change_affix_list() {
            return;
        }
        let Some(affix) = self.affixes.get_mut(row) else {
            return;
        };
        let uid = affix.uid;
        if affix.added {
            self.discard_row_ref(RowRef::Affix(uid));
        } else {
            // A removed row takes no other draft, so its removal is its latest step.
            let changed = if affix.is_removed() { affix.undo_draft() } else { affix.draft_removal() };
            if changed {
                self.track(RowRef::Affix(uid));
            }
        }
        self.resolve_tables();
    }
}
