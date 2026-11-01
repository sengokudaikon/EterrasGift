//! The header: rarity, base and the actions on the whole item.

use super::*;

impl ForgeEditor {
    pub(super) fn is_crafted(&self) -> bool {
        self.item.as_ref().is_some_and(|item| !item::is_unique_backed(item.rarity))
    }

    /// Normal, magic, rare, exalted, unique, set or legendary: what colours the title.
    pub fn rarity_code(&self) -> u8 {
        self.item.as_ref().map_or(0, |item| self.drafted_identity().map_or(item.rarity, |(_, _, rarity)| rarity))
    }

    pub fn name_is_exalted(&self) -> bool {
        self.item.as_ref().is_some_and(ForgeItem::is_exalted)
    }

    /// A crafted rarity in the game's words, by the byte the unsealed affix count gives it.
    pub(super) fn count_rarity_name(&self, rarity: u8) -> String {
        self.catalog().rarity_name(rarity).unwrap_or_else(|| item::crafted_name(rarity).into())
    }

    /// The crafted rarity the drafted affixes give the item, `drafted` being the item as Apply would leave it.
    pub(super) fn drafted_count_rarity(&self, drafted: Option<&ForgeItem>) -> String {
        drafted.map(|item| self.count_rarity_name(item.rarity)).unwrap_or_default()
    }

    pub(super) fn is_draft_exalted(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_exalted)
    }

    /// The rarity in the game's words; on crafted items it follows the drafted tiers and affixes.
    pub fn rarity_text(&self) -> String {
        self.rarity_text_of(self.drafted_item().as_ref())
    }

    /// `rarity_text` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn rarity_text_of(&self, drafted: Option<&ForgeItem>) -> String {
        let Some(item) = self.item.as_ref() else {
            return String::new();
        };
        if !self.is_crafted() {
            return self.rarity_label(self.drafted_identity().map_or(item.rarity, |(_, _, rarity)| rarity));
        }
        if self.is_draft_exalted() {
            tr::Exalted.into()
        } else {
            self.drafted_count_rarity(drafted)
        }
    }

    /// Whether the drafts change the rarity, `drafted` being the item as Apply would leave it (`drafted_item`).
    pub fn is_rarity_drafted_of(&self, drafted: Option<&ForgeItem>) -> bool {
        if self.is_crafted() {
            self.is_draft_exalted() != self.item.as_ref().is_some_and(ForgeItem::is_exalted)
                || self
                    .item
                    .as_ref()
                    .is_some_and(|item| self.drafted_count_rarity(drafted) != self.count_rarity_name(item.rarity))
        } else {
            self.drafted_identity()
                .is_some_and(|(_, _, rarity)| Some(rarity) != self.item.as_ref().map(|item| item.rarity))
        }
    }

    pub fn can_change_rarity(&self) -> bool {
        self.is_crafted() && !self.affixes.is_empty() && self.can_change_affix()
    }

    pub fn rarity_tip(&self) -> String {
        if !self.is_crafted() {
            String::new()
        } else if self.can_change_rarity() {
            phrases! { Switch: "Switching to or from exalted changes affix tiers. Normal, magic and rare follow the unsealed affix count." }
            Switch.into()
        } else {
            phrases! { Follows: "Normal, magic and rare follow the unsealed affix count. Exalted needs an affix at T6 or higher." }
            Follows.into()
        }
    }

    /// The base the item's affix values are shown for: the one drafted, else its own.
    pub fn value_base(&self) -> Option<(u16, u16)> {
        let item = self.item.as_ref()?;
        self.drafted_base().or(Some((item.item_type, item.sub_type)))
    }

    /// The base a draft would give the item (item type, sub-type): a drafted base, or the base a drafted unique comes on
    /// when it differs.
    pub(super) fn drafted_base(&self) -> Option<(u16, u16)> {
        let item = self.item.as_ref()?;
        match (&*self.base, self.drafted_identity()) {
            (Some(base), _) if base.is_dirty() => Some(base_of(base.value())),
            (_, Some((_, sub_type, _))) if sub_type != item.sub_type => Some((item.item_type, sub_type)),
            _ => None,
        }
    }

    pub fn base_text(&self) -> String {
        match &*self.item {
            Some(item) => {
                let (item_type, sub_type) = self.drafted_base().unwrap_or((item.item_type, item.sub_type));
                self.catalog().base(item_type, sub_type).map(|base| base.name.clone()).unwrap_or_default()
            }
            None => String::new(),
        }
    }

    pub fn is_base_drafted(&self) -> bool {
        self.drafted_base().is_some()
    }

    pub fn can_change_base(&self) -> bool {
        self.base.as_ref().is_some_and(FieldDraft::is_editable)
            && self.can_mutate()
            && !self.has_implicit_drafts()
            && self.game.supports(&ItemEdit::BaseType(crate::item::edit::BaseTypeEdit {
                sub_type: 0,
                allow_incompatible: false,
                item_type: None,
            }))
    }

    pub fn base_tip(&self) -> String {
        phrases! {
            KeptBase: "Uniques, set items and legendaries keep their base. Change the unique to change it."
            ImplicitsFirst: "Apply or discard the implicit roll drafts first. A new base replaces the implicits."
            ChangeBase: "Change the base. Implicits and requirements follow it."
        }
        match (&*self.base, &*self.item) {
            (None, Some(item)) if item::is_unique_backed(item.rarity) => KeptBase.into(),
            (None, _) => String::new(),
            (Some(base), _) if !base.is_editable() => base.read_only_reason.clone(),
            _ if self.has_implicit_drafts() => ImplicitsFirst.into(),
            _ => ChangeBase.into(),
        }
    }

    /// The facts after the rarity and base, each its own text: the level requirement where it is no cell, then what the
    /// item holds.
    pub fn summary(&self) -> Vec<String> {
        let Some(item) = self.item.as_ref() else {
            return Vec::new();
        };
        let mut parts = Vec::new();
        if let Some(status) = item.status
            && status.level_requirement > 0
            && !self.fields.iter().any(|field| field.field == Some(StatusField::LevelRequirement))
        {
            parts.push(tr::Level.with(&[("level", &status.level_requirement)]));
        }
        phrases! { AndModifiers: "{affixes}, {modifiers}" }
        let affixes = tr::Affixes.count(self.affixes.iter().filter(|row| !row.is_removed()).count());
        parts.push(
            if item
                .backing()
                .is_some_and(|unique| matches!(unique.formatting, crate::screens::forge::game::Lookup::Ready(())))
            {
                AndModifiers.with(&[("affixes", &affixes), ("modifiers", &tr::UniqueModifiers.count(self.uniques.len()))])
            } else {
                affixes
            },
        );
        parts
    }

    /// The two rarities tiers can reach on a crafted item with affixes.
    pub fn rarity_options(&self) -> Vec<RarityOption> {
        self.rarity_options_of(self.drafted_item().as_ref())
    }

    /// `rarity_options` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn rarity_options_of(&self, drafted: Option<&ForgeItem>) -> Vec<RarityOption> {
        if !self.is_crafted() || self.affixes.is_empty() {
            return Vec::new();
        }
        let exalted = self.is_draft_exalted();
        let game_exalted = self.item.as_ref().is_some_and(ForgeItem::is_exalted);
        [false, true]
            .into_iter()
            .map(|to_exalted| {
                let plan = if to_exalted == exalted { Vec::new() } else { self.exalted_plan(to_exalted) };
                let name = |index: usize| self.affixes[index].name();
                phrases! {
                    NoT6: "No affix on this item has a T6."
                    Raises: "Raises {name} to T6."
                    Restores: "Restores {name} to T{tier}."
                    RestoresAll: "Restores {n} affixes to their tiers in game."
                    Lowers: "Lowers {name} to T{tier}."
                    LowersAll: "Lowers {n} affixes below T6."
                }
                let note = match (to_exalted, plan.as_slice()) {
                    _ if to_exalted == exalted => String::new(),
                    (_, []) => NoT6.into(),
                    (true, [(only, tier, _)]) if *tier == 6 && self.affixes[*only].state.tier < 6 => Raises.with(&[("name", &name(*only))]),
                    (true, [(only, tier, _)]) => Restores.with(&[("name", &name(*only)), ("tier", tier)]),
                    (true, _) => RestoresAll.with(&[("n", &plan.len())]),
                    (false, [(only, tier, _)]) => Lowers.with(&[("name", &name(*only)), ("tier", tier)]),
                    _ => LowersAll.with(&[("n", &plan.len())]),
                };
                RarityOption {
                    name: if to_exalted { tr::Exalted.into() } else { self.drafted_count_rarity(drafted) },
                    exalted: to_exalted,
                    note,
                    available: to_exalted == exalted || !plan.is_empty(),
                    in_game: to_exalted == game_exalted,
                    current: to_exalted == exalted,
                }
            })
            .collect()
    }

    /// The tier drafts that make the item exalted or not: row, tier, seal.
    pub(super) fn exalted_plan(&self, exalted: bool) -> Vec<(usize, u8, Seal)> {
        // Rows Apply removes take no part; an added affix has no tier in the game.
        let game_tier = |row: &AffixRow| if !row.added && row.draft_id() == row.state.id { row.state.tier } else { 0 };
        let plan: Vec<(usize, u8, Seal)> = if !exalted {
            self.affixes
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_exalted())
                .map(|(index, row)| {
                    let own = game_tier(row);
                    let tier = if (1..=5).contains(&own) { own } else { 5 };
                    (index, tier, if row.seal() == Seal::Primordial { Seal::Normal } else { row.seal() })
                })
                .collect()
        } else {
            let restore: Vec<(usize, u8, Seal)> = self
                .affixes
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_editable() && game_tier(row) >= 6)
                .map(|(index, row)| {
                    let seal = if row.seal() == Seal::Normal && row.state.seal == Seal::Primordial {
                        Seal::Primordial
                    } else {
                        row.seal()
                    };
                    (index, row.state.tier, seal)
                })
                .collect();
            if restore.is_empty() {
                let mut candidates: Vec<(usize, &AffixRow)> = self.affixes.iter().enumerate().filter(|(_, row)| row.is_editable() && row.max_tier() >= 6).collect();
                candidates.sort_by_key(|(_, row)| (row.seal() != Seal::Normal, std::cmp::Reverse(row.tier())));
                candidates.into_iter().take(1).map(|(index, row)| (index, 6, row.seal())).collect()
            } else {
                restore
            }
        };
        plan.into_iter().filter(|(index, tier, _)| *tier <= self.affixes[*index].max_tier()).collect()
    }

    /// Drafts the tier changes that make the item exalted or not, one affix draft each.
    pub fn pick_rarity(&mut self, exalted: bool) {
        if exalted == self.is_draft_exalted() || !self.can_change_rarity() {
            return;
        }
        for (index, tier, seal) in self.exalted_plan(exalted) {
            if self.affixes[index].draft_tier(tier, seal) {
                self.edited(RowRef::Affix(self.affixes[index].uid));
            }
        }
        self.resolve_tables();
    }

    /// The tier drafts "Max tiers" makes: every affix the item keeps up to its highest tier below the primordial T8 (row,
    /// tier). None is lowered, and a primordial is left as it is: T8 needs its seal, and the game keeps one primordial.
    pub(super) fn max_tier_plan(&self) -> Vec<(usize, u8)> {
        self.affixes
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_editable() && row.seal() != Seal::Primordial)
            .filter_map(|(index, row)| {
                let target = row.max_tier().min(7);
                (row.tier() < target).then_some((index, target))
            })
            .collect()
    }

    pub fn can_max_tiers(&self) -> bool {
        self.can_change_affix() && !self.max_tier_plan().is_empty()
    }

    pub fn max_tiers_tip(&self) -> String {
        phrases! {
            TiersAtBest: "Every affix is at its highest tier below T8."
            Tiers: "Draft the affix up to its highest tier, at most T7. Primordial affixes stay." | "Draft {n} affixes up to their highest tier, at most T7. Primordial affixes stay."
        }
        match self.max_tier_plan().len() {
            0 => TiersAtBest.into(),
            n => Tiers.count(n),
        }
    }

    /// Drafts every affix up to its highest tier below T8, one tier draft each.
    pub fn max_tiers(&mut self) {
        if !self.can_max_tiers() {
            return;
        }
        for (index, tier) in self.max_tier_plan() {
            let seal = self.affixes[index].seal();
            if self.affixes[index].draft_tier(tier, seal) {
                self.edited(RowRef::Affix(self.affixes[index].uid));
            }
        }
        self.resolve_tables();
    }

    /// The cells "Max rolls" drafts: every roll the forge can edit now (affixes, unique lines, implicits) that is below
    /// roll 255, the game's perfect roll. A roll unique lines share is drafted once; the others follow it.
    pub(super) fn max_roll_plan(&self) -> Vec<Cell> {
        let mut cells: Vec<Cell> = (0..self.affixes.len()).map(Cell::Affix).collect();
        let mut shared = std::collections::BTreeSet::new();
        for (row, line) in self.uniques.iter().enumerate() {
            for (position, roll) in line.rolls.iter().enumerate() {
                if shared.insert(roll.index) {
                    cells.push(Cell::Unique(row, position));
                }
            }
        }
        for (row, line) in self.implicits.iter().enumerate() {
            cells.extend((0..line.rolls.len()).map(|position| Cell::Implicit(row, position)));
        }
        cells
            .into_iter()
            .filter(|&cell| self.can_edit_value(cell) && self.editor_of(cell).is_some_and(|(_, roll, _)| roll != u8::MAX))
            .collect()
    }

    pub fn can_max_rolls(&self) -> bool {
        !self.max_roll_plan().is_empty()
    }

    pub fn max_rolls_tip(&self) -> String {
        phrases! {
            RollsAtBest: "Every roll that can change is at its best."
            Rolls: "Draft the roll up to 255, the game's perfect roll." | "Draft {n} rolls up to 255, the game's perfect roll."
        }
        match self.max_roll_plan().len() {
            0 => RollsAtBest.into(),
            n => Rolls.count(n),
        }
    }

    /// Drafts every roll the forge can edit to roll 255, one roll draft each.
    pub fn max_rolls(&mut self) {
        for cell in self.max_roll_plan() {
            self.move_cell(cell, Some(u8::MAX));
        }
    }
}
