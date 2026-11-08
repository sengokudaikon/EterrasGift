//! The implicits and the affix list: what can be added, and each row's tier and seal.

use super::*;

impl ForgeEditor {
    pub fn shows_implicit_preview(&self) -> bool {
        self.is_base_drafted() || self.item.as_ref().is_some_and(|item| item.implicits.is_none())
    }

    /// Implicit lines from game data: the drafted base's, or the current base's when the game did not describe them.
    pub fn implicit_preview(&self) -> Vec<String> {
        let Some(item) = self.item.as_ref() else {
            return Vec::new();
        };
        let (item_type, sub_type) = match self.drafted_base() {
            Some(base) => base,
            None if item.implicits.is_none() => (item.item_type, item.sub_type),
            None => return Vec::new(),
        };
        self.catalog().base(item_type, sub_type).map(|base| base.implicits.clone()).unwrap_or_default()
    }

    pub fn has_implicits(&self) -> bool {
        self.item.is_some()
            && if self.shows_implicit_preview() {
                !self.implicit_preview().is_empty()
            } else {
                !self.implicits.is_empty()
            }
    }

    pub fn implicit_note(&self) -> &'static str {
        phrases! {
            DraftedBase: "From the drafted base. Roll positions carry over when it applies."
            GameData: "From game data. The game didn't describe them, so they can't reroll."
            SetByBase: "Set by the base. Their values can reroll, and a new base replaces them."
        }
        if self.is_base_drafted() {
            DraftedBase.get()
        } else if self.item.as_ref().is_some_and(|item| item.implicits.is_none()) {
            GameData.get()
        } else {
            SetByBase.get()
        }
    }

    // ----- Affix list -----

    pub fn selected_affix(&self) -> Option<usize> {
        self.selected_affix
    }

    pub fn selected_unique(&self) -> Option<usize> {
        self.selected_unique
    }

    pub fn select_affix(&mut self, index: Option<usize>) {
        let index = index.filter(|index| *index < self.affixes.len());
        if index == self.selected_affix {
            return;
        }
        if index.is_some() {
            self.selected_unique = None;
        }
        self.selected_affix = index;
        *self.picker = None;
    }

    pub fn select_unique(&mut self, index: Option<usize>) {
        let index = index.filter(|index| *index < self.uniques.len());
        if index.is_some() {
            self.selected_affix = None;
        }
        self.selected_unique = index;
    }

    /// Adding and removing are drafts like the others: Apply writes them in order with the rest, each step naming the slots
    /// the earlier ones leave.
    pub fn can_change_affix_list(&self) -> bool {
        self.can_mutate()
            && self.item.is_some()
            && self.game.supports(&ItemEdit::RemoveAffix(eg_trainer::item::edit::RemoveAffixEdit {
                slot: 0,
                id: 0,
                restore_potential: None,
            }))
    }

    pub fn add_affix_tip(&self) -> String {
        match &*self.item {
            None => String::new(),
            Some(_) => {
                phrases! { AddTip: "Add an affix as a draft" }
                AddTip.into()
            }
        }
    }

    /// The free slots the drafted affixes leave.
    pub fn free_slots_text(&self) -> String {
        self.free_slots_text_of(self.drafted_item().as_ref())
    }

    /// `free_slots_text` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn free_slots_text_of(&self, drafted: Option<&ForgeItem>) -> String {
        let Some(item) = drafted else {
            return String::new();
        };
        if item.rarity == item::SET || (item.rarity == item::UNIQUE && item.idol) {
            const NO_SLOTS: [Phrase; 2] = [Phrase::new("No affix slots on a set item."), Phrase::new("No affix slots on a unique idol.")];
            const NO_SLOTS_LOCKED: [Phrase; 2] = [
                Phrase::new("No affix slots on a set item. Adding needs \"I know what I'm doing\"."),
                Phrase::new("No affix slots on a unique idol. Adding needs \"I know what I'm doing\"."),
            ];
            let kind = usize::from(item.idol);
            return if self.unrestricted() { NO_SLOTS[kind].into() } else { NO_SLOTS_LOCKED[kind].into() };
        }
        phrases! {
            Free: "Free: {prefixes}, {suffixes}"
            FreeUnique: "Free: {prefixes}, {suffixes}. An affix makes it legendary."
        }
        let (prefixes, suffixes) = item.free_slots();
        let values: [(&str, &dyn std::fmt::Display); 2] = [
            ("prefixes", &tr::Prefixes.count(prefixes.max(0) as usize)),
            ("suffixes", &tr::Suffixes.count(suffixes.max(0) as usize)),
        ];
        if item.rarity == item::UNIQUE { FreeUnique.with(&values) } else { Free.with(&values) }
    }

    pub fn tier_ladder_note(&self) -> &'static str {
        phrases! {
            NoAffixes: "This item has no affixes."
            SelectAffix: "Select an affix to see its tiers."
        }
        if self.affixes.is_empty() { NoAffixes.get() } else { SelectAffix.get() }
    }

    pub fn tier_options(&self, row: usize) -> Vec<(u8, String, bool)> {
        let Some(row) = self.affixes.get(row) else {
            return Vec::new();
        };
        (1..=row.max_tier())
            .map(|tier| {
                (
                    tier,
                    self.catalog().tier_range(row.draft_id(), tier, self.value_base()),
                    !row.added && row.draft_id() == row.state.id && tier == row.state.tier,
                )
            })
            .collect()
    }

    /// The range of the value as drafted, "min – max"; an added affix's, which the game has no values for yet, is its
    /// tier's range from game data.
    pub fn range_text(&self, row: usize) -> String {
        match self.affixes.get(row) {
            Some(row) if row.added => self.catalog().tier_range(row.draft_id(), row.tier(), self.value_base()),
            Some(row) if row.value.has_scale() => format!("{} – {}", row.value.minimum_text(), row.value.maximum_text()),
            _ => String::new(),
        }
    }

    pub fn can_change_affix(&self) -> bool {
        self.can_mutate()
            && self.game.supports(&ItemEdit::Affix(eg_trainer::item::edit::AffixEdit {
                slot: 0,
                id: 0,
                tier: 1,
                roll: 0,
                seal: Seal::Normal,
                allow_incompatible: false,
            }))
    }

    pub fn can_change_field(&self, index: usize) -> bool {
        self.can_mutate() && self.fields.get(index).is_some_and(|f| f.is_editable() && self.game.supports(&f.read_edit()))
    }

    pub fn pick_tier(&mut self, row: usize, tier: u8) {
        if !self.can_change_affix() {
            return;
        }
        if let Some(affix) = self.affixes.get_mut(row).filter(|affix| affix.is_editable()) {
            let seal = affix.seal();
            if affix.draft_tier(tier, seal) {
                let uid = affix.uid;
                self.edited(RowRef::Affix(uid));
                self.resolve_tables();
            }
        }
    }

    pub fn pick_seal(&mut self, row: usize, seal: Seal) {
        if !self.can_change_affix() {
            return;
        }
        if let Some(affix) = self.affixes.get_mut(row).filter(|affix| affix.is_editable()) {
            let tier = affix.tier();
            if affix.draft_tier(tier, seal) {
                let uid = affix.uid;
                self.edited(RowRef::Affix(uid));
                self.resolve_tables();
            }
        }
    }
}
