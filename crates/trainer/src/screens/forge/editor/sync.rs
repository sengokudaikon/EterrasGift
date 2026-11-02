//! The item as the game reports it: rows follow, and untouched ones move with it.

use super::*;

impl ForgeEditor {
    pub(super) fn update_item(&mut self, snapshot: Option<ForgeItem>, moves: Option<&Moves>) {
        let replaced = self.item.as_ref().map(|item| &item.token) != snapshot.as_ref().map(|item| &item.token);
        let discarded = if replaced { self.draft_count() } else { 0 };
        self.tables.clear();
        if replaced {
            *self.picker = None;
            self.applied = None;
            self.history.clear();
            self.draft_order.clear();
            self.affixes.clear();
            self.uniques.clear();
            self.implicits.clear();
            self.selected_unique = None;
            self.selected_affix = None;
            self.say_preview("", false, false);
        }
        *self.item = snapshot.clone();
        let states = snapshot.as_ref().map(|item| item.affixes.clone()).unwrap_or_default();
        // Rows are slots. After this editor's own write each row follows its affix to the slot the write put it in. Added
        // rows wait for Apply after the game's rows, whatever the game reads.
        let selected_uid = self.selected_affix.and_then(|index| self.affixes.get(index)).map(|row| row.uid);
        let (added, mut old): (Vec<AffixRow>, Vec<AffixRow>) = std::mem::take(&mut *self.affixes).into_iter().partition(|row| row.added);
        if let Some(moves) = moves {
            for row in &mut old {
                let slot = *moves.get(&row.slot()).unwrap_or(&usize::MAX);
                row.move_to(slot);
            }
        }
        for state in &states {
            let definition = self.catalog().definition(state.id);
            match old.iter().position(|row| row.state.slot == state.slot) {
                Some(index) => {
                    let mut row = old.remove(index);
                    let preserve = row.is_dirty() || row.can_undo() || row.value.typing;
                    row.update_state(state.clone(), definition, preserve);
                    self.affixes.push(row);
                }
                None => {
                    let uid = self.uid();
                    self.affixes.push(AffixRow::new(uid, state.clone(), definition));
                }
            }
        }
        for row in old {
            self.forget(RowRef::Affix(row.uid));
        }
        self.affixes.extend(added);
        self.selected_affix = selected_uid.and_then(|uid| self.affixes.iter().position(|row| row.uid == uid));

        let unique = snapshot.as_ref().and_then(|item| item.backing().cloned());
        let (modifiers, rolls) = unique.map(|unique| (unique.modifiers, unique.rolls)).unwrap_or_default();
        self.update_lines(RollTarget::Unique, &modifiers, &rolls);
        let implicit_rolls = snapshot.as_ref().map(|item| item.implicit_rolls.clone()).unwrap_or_default();
        let implicits = snapshot.as_ref().and_then(|item| item.implicits.clone()).unwrap_or_default();
        self.update_lines(RollTarget::Implicit, &implicits, &implicit_rolls);
        let lost = self.update_fields(!replaced);

        if self.selected_unique.is_some_and(|index| index >= self.uniques.len()) {
            self.selected_unique = None;
        }
        if self.selected_unique.is_none() && self.selected_affix.is_none() {
            self.selected_affix = (!self.affixes.is_empty()).then_some(0);
        }
        if discarded > 0 {
            phrases! { Changed: "The forge item changed, so its draft was discarded." | "The forge item changed, so its {n} drafts were discarded." }
            self.say(Changed.count(discarded), MessageKind::Info);
        } else if !lost.is_empty() {
            phrases! {
                Lost: "The item changed in the game and no longer has {missing}, so that draft was discarded." | "The item changed in the game and no longer has {missing}, so those drafts were discarded."
            }
            self.say(Lost.with(lost.len(), &[("missing", &list(&lost, Conjunction::Or))]), MessageKind::Info);
        }
        // What fits depends on the item, so an open picker judges its list again.
        self.filter_picker(true);
        self.resolve_tables();
        self.refresh_unique_preview();
    }

    pub(super) fn update_lines(&mut self, target: RollTarget, states: &[item::ModifierSnapshot], rolls: &[u8]) {
        let mut rows = std::mem::take(if target == RollTarget::Unique { &mut *self.uniques } else { &mut *self.implicits });
        let mut kept = Vec::new();
        for state in states {
            match rows.iter().position(|row| row.state.index == state.index) {
                Some(index) => {
                    let mut row = rows.remove(index);
                    let preserve = row.is_dirty() || row.can_undo();
                    row.update(state.clone(), rolls, preserve);
                    kept.push(row);
                }
                None => {
                    let uid = self.uid();
                    kept.push(RollRow::new(uid, target, state.clone(), rolls));
                }
            }
        }
        for row in rows {
            self.forget(if target == RollTarget::Unique {
                RowRef::Unique(row.uid)
            } else {
                RowRef::Implicit(row.uid)
            });
        }
        if target == RollTarget::Unique {
            *self.uniques = kept;
        } else {
            *self.implicits = kept;
        }
    }

    /// Rebuilds the item-level cells when what applies changes; otherwise they follow the game and keep their drafts.
    /// Returns the names of drafted cells the item no longer has.
    pub(super) fn update_fields(&mut self, same_item: bool) -> Vec<String> {
        let Some(item) = (*self.item).clone() else {
            self.fields.clear();
            *self.base = None;
            *self.identity = None;
            self.identity_keys.clear();
            self.fields_key.clear();
            self.options_source = None;
            self.menu_generation += 1;
            return Vec::new();
        };
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{:?}",
            item.token,
            item.status.is_none(),
            item.rarity,
            item.item_type,
            self.catalog().limits.is_some(),
            self.unrestricted(),
            item.identity_id()
        );
        if key == self.fields_key
            && let Some(status) = item.status
        {
            for field in self.fields.iter_mut() {
                if let Some(which) = field.field {
                    field.update(i64::from(status.read(which)));
                }
            }
            if let Some(base) = self.base.as_mut() {
                base.update(base_key(item.item_type, item.sub_type));
            }
            // The options follow the item, the catalog and the override, and nothing else: the same three make the same
            // options, so they are not made again for every read that changed something else. Making them prepares an
            // edit for each unique of the item's type.
            let made_from = OptionsSource::of(self, &item);
            if self.options_source.as_ref() == Some(&made_from) {
                if let Some(identity) = self.identity.as_mut() {
                    identity.update(item.identity_id().map_or(0, i64::from));
                }
                return Vec::new();
            }
            self.options_source = Some(made_from);
            // The menus are rebuilt only when the options changed.
            let options = self.base_options(&item);
            if let Some(base) = self.base.as_mut()
                && base.options != options
            {
                base.options = options;
                self.menu_generation += 1;
            }
            let options = self.identity_options(&item);
            if let Some(identity) = self.identity.as_mut() {
                identity.update(item.identity_id().map_or(0, i64::from));
                if identity.options != options {
                    identity.options = options;
                    self.menu_generation += 1;
                    self.index_identities();
                }
            }
            return Vec::new();
        }
        self.fields_key = key;
        self.options_source = Some(OptionsSource::of(self, &item));
        let drafted: Vec<FieldDraft> = if same_item {
            self.all_fields().filter(|field| field.is_dirty()).cloned().collect()
        } else {
            Vec::new()
        };
        for field in self.all_fields().map(|field| field.uid).collect::<Vec<_>>() {
            self.forget(RowRef::Field(field));
        }
        self.rebuild_fields(&item);
        self.index_identities();
        self.menu_generation += 1;
        let mut lost = Vec::new();
        for old in drafted {
            match self.fields_mut().find(|field| field.field == old.field && field.kind == old.kind) {
                Some(rebuilt) => rebuilt.carry_stale(&old),
                None => lost.push(old.name),
            }
        }
        lost
    }

    pub(super) fn rebuild_fields(&mut self, item: &ForgeItem) {
        self.fields.clear();
        *self.base = None;
        *self.identity = None;
        self.identity_search.clear();
        let Some(status) = item.status else {
            return;
        };
        let limits = self.catalog().limits;
        let backed = item::is_unique_backed(item.rarity);
        if !backed {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::ForgingPotential,
                tr::ForgingPotential.get(),
                status.forging_potential,
                limits.map(|l| l.forging_potential),
                "",
                "",
                "",
                false,
            ));
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::LevelRequirement,
                tr::LevelRequirement.get(),
                status.level_requirement,
                limits.map(|l| l.level_requirement),
                "",
                tr::LevelRequirementNote.get(),
                "",
                false,
            ));
        } else if item.rarity == item::UNIQUE {
            let unique = item.unique.as_ref().and_then(|unique| self.catalog().unique(unique.id)).map(|unique| unique.legendary_type);
            let kind = unique.or(if status.weavers_will > 0 {
                Some(LegendaryType::WeaversWill)
            } else if status.legendary_potential > 0 {
                Some(LegendaryType::LegendaryPotential)
            } else {
                None
            });
            phrases! { NotInData: "This unique isn't in the game data. Read the game files again in Settings." }
            let reason = if unique.is_none() { NotInData.get() } else { "" };
            let uid = self.uid();
            let unrestricted = self.unrestricted();
            match kind {
                Some(LegendaryType::WeaversWill) => self.fields.push(FieldDraft::number(
                    uid,
                    StatusField::WeaversWill,
                    tr::WeaversWill.get(),
                    status.weavers_will,
                    limits.map(|l| l.weavers_will),
                    reason,
                    "",
                    "",
                    unrestricted,
                )),
                Some(LegendaryType::LegendaryPotential) => {
                    let mixed = reason.is_empty() && status.weavers_will > 0;
                    self.fields.push(FieldDraft::number(
                        uid,
                        StatusField::LegendaryPotential,
                        tr::LegendaryPotential.get(),
                        status.legendary_potential,
                        limits.map(|l| l.legendary_potential),
                        if mixed { tr::MixedReason.get() } else { reason },
                        "",
                        if mixed { tr::MixedHeading.get() } else { "" },
                        false,
                    ))
                }
                None => {}
            }
        } else if item.rarity == item::LEGENDARY && status.legendary_potential > 0 {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::LegendaryPotential,
                tr::LegendaryPotential.get(),
                status.legendary_potential,
                limits.map(|l| l.legendary_potential),
                tr::LegendaryFixed.get(),
                "",
                "",
                false,
            ));
        }
        self.add_override_fields(item);
        let uid = self.uid();
        self.fields.push(FieldDraft::flag(
            uid,
            StatusField::Corrupted,
            tr::Corruption.get(),
            tr::NotCorrupted.get(),
            tr::Corrupted.get(),
            status.corrupted,
        ));
        let uid = self.uid();
        self.fields.push(FieldDraft::flag(
            uid,
            StatusField::Ruined,
            tr::Ruin.get(),
            tr::NotRuined.get(),
            tr::Ruined.get(),
            status.ruined,
        ));
        if !backed {
            let uid = self.uid();
            *self.base = Some(FieldDraft::base(uid, base_key(item.item_type, item.sub_type), self.base_options(item)));
        }
        if backed && let Some(identity) = item.identity_id() {
            let uid = self.uid();
            *self.identity = Some(FieldDraft::identity(uid, i64::from(identity), self.identity_options(item)));
        }
    }

    /// Values only "I know what I'm doing" writes.
    pub(super) fn add_override_fields(&mut self, item: &ForgeItem) {
        let Some(status) = item.status else {
            return;
        };
        let enabled = self.unrestricted();
        let locked: String = tr::ChangeLocked.into();
        let limits = self.catalog().limits;
        let reason = |mixed: bool| {
            if !enabled {
                locked.clone()
            } else if mixed {
                tr::MixedReason.into()
            } else {
                String::new()
            }
        };
        let unique_type = item.unique.as_ref().and_then(|unique| self.catalog().unique(unique.id)).map(|unique| unique.legendary_type);
        if item.rarity == item::UNIQUE && unique_type == Some(LegendaryType::LegendaryPotential) && (enabled || status.weavers_will > 0) {
            let mixed = enabled && status.legendary_potential > 0;
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversWill,
                tr::WeaversWill.get(),
                status.weavers_will,
                limits.map(|l| l.weavers_will),
                &reason(mixed),
                tr::ForcedWillNote.get(),
                if mixed { tr::MixedHeading.get() } else { "" },
                true,
            ));
        }
        if item.rarity == item::LEGENDARY && (enabled || status.weavers_will > 0) {
            let mixed = enabled && status.legendary_potential > 0 && status.weavers_will == 0;
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversWill,
                tr::WeaversWill.get(),
                status.weavers_will,
                limits.map(|l| l.weavers_will),
                &reason(mixed),
                tr::LegendaryWillNote.get(),
                if mixed { tr::MixedHeading.get() } else { "" },
                true,
            ));
        }
        if item.idol && (enabled || status.weavers_touch > 0) {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversTouch,
                tr::WeaversTouch.get(),
                status.weavers_touch,
                // An idol already holding more than the game's maximum stays editable, to bring it down.
                limits.map(|l| (l.weavers_touch.0, l.weavers_touch.1.max(status.weavers_touch))),
                if enabled { "" } else { &locked },
                tr::WeaversTouchNote.get(),
                "",
                true,
            ));
        }
    }

    /// The item type's bases, each with its level, the implicits it grants and how many of the item's affixes it would make
    /// incompatible. An idol lists every idol type's bases after its own, which only "I know what I'm doing" can pick.
    pub(super) fn base_options(&self, item: &ForgeItem) -> Vec<FieldOption> {
        let mut types: Vec<u16> = vec![item.item_type];
        if item.idol {
            types.extend(
                self.catalog()
                    .item_types
                    .iter()
                    .map(|t| t.item_type)
                    .filter(|t| (25..=33).contains(t) && *t != item.item_type),
            );
        }
        let unrestricted = self.unrestricted();
        types
            .into_iter()
            .filter_map(|item_type| self.catalog().item_type(item_type))
            .flat_map(|definition| definition.bases.iter().map(move |base| (definition, base)))
            .map(|(definition, base)| {
                let other_type = definition.item_type != item.item_type;
                let mut facts = vec![tr::Level.with(&[("level", &base.level)])];
                if other_type {
                    facts.insert(0, definition.name.clone());
                }
                if base.corrupted {
                    phrases! { CorruptedBase: "Corrupted base" }
                    facts.push(CorruptedBase.into());
                }
                let broken = self.broken_by_base(item, definition.item_type, base.sub_type);
                if broken > 0 {
                    facts.push(tr::IncompatibleAffixes.count(broken));
                }
                FieldOption {
                    value: base_key(definition.item_type, base.sub_type),
                    name: base.name.clone(),
                    facts,
                    note: if other_type && !unrestricted { tr::NeedsOverride.into() } else { String::new() },
                    in_game: !other_type && base.sub_type == item.sub_type,
                    implicits: base.implicits.join("\n"),
                    available: !other_type || unrestricted,
                    rarity: 0,
                }
            })
            .collect()
    }
}

/// What the base and identity options are made from: the item, the catalog (by address: it is replaced, never changed)
/// and the override.
#[derive(PartialEq)]
pub(super) struct OptionsSource {
    item: ForgeItem,
    catalog: usize,
    unrestricted: bool,
}

impl OptionsSource {
    pub(super) fn of(editor: &ForgeEditor, item: &ForgeItem) -> Self {
        Self {
            item: item.clone(),
            catalog: std::ptr::from_ref(editor.catalog()) as usize,
            unrestricted: editor.unrestricted(),
        }
    }
}

impl ForgeEditor {
    /// How many of the item's affixes a change to `item_type`'s `sub_type` would make incompatible, judged with the class
    /// requirement that base carries.
    pub(super) fn broken_by_base(&self, item: &ForgeItem, item_type: u16, sub_type: u16) -> usize {
        let class_requirement = self.catalog().base(item_type, sub_type).map(|base| base.class_requirement);
        item::broken_by_base(item, item_type, sub_type, class_requirement).len()
    }
}
