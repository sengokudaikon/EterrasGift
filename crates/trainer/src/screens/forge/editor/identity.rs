//! The unique, set or legendary behind an item's name.

use super::*;

impl ForgeEditor {
    pub fn has_identity(&self) -> bool {
        self.identity.is_some()
    }

    pub fn is_identity_drafted(&self) -> bool {
        self.identity.as_ref().is_some_and(FieldDraft::is_dirty)
    }

    /// The drafted identity's unique, base and rarity.
    pub(super) fn drafted_identity(&self) -> Option<(u16, u16, u8)> {
        let item = self.item.as_ref()?;
        let identity = self.identity.as_ref().filter(|identity| identity.is_dirty())?;
        match self.identity_edit(item, identity.value() as u16) {
            ItemEdit::UniqueIdentity(crate::item::edit::UniqueIdentityEdit { unique_id, sub_type, rarity, .. }) => {
                Some((u16::try_from(unique_id).ok()?, u16::try_from(sub_type).ok()?, u8::try_from(rarity).ok()?))
            }
            _ => None,
        }
    }

    /// The item's name as the game holds it; a drafted unique shows its own until applied or discarded.
    pub fn item_name(&self) -> String {
        match self.drafted_identity().and_then(|(id, _, _)| self.catalog().unique(id)) {
            Some(unique) => unique.name.clone(),
            None => self.item.as_ref().map(|item| item.name.clone()).unwrap_or_default(),
        }
    }

    pub fn can_change_identity(&self) -> bool {
        self.identity.as_ref().is_some_and(FieldDraft::is_editable)
            && self.can_change_draft()
            && !self.has_other_drafts()
            && self.game.supports(&ItemEdit::UniqueIdentity(crate::item::edit::UniqueIdentityEdit {
                unique_id: 0,
                sub_type: 0,
                rarity: 7,
                unrestricted: false,
            }))
    }

    pub fn identity_tip(&self) -> String {
        phrases! {
            OtherDraftsFirst: "Apply or discard the other drafts first. A new unique brings its own modifiers."
            ChangeIdentity: "Change which unique or set item this is"
        }
        match &*self.identity {
            None => String::new(),
            Some(identity) if !identity.is_editable() => identity.read_only_reason.clone(),
            _ if self.has_other_drafts() => OtherDraftsFirst.into(),
            _ => ChangeIdentity.into(),
        }
    }

    /// The game's name for a unique-backed rarity.
    pub(super) fn rarity_label(&self, rarity: u8) -> String {
        phrases! {
            Legendary: "rarity" => "Legendary"
            Set: "rarity" => "Set"
            Unique: "rarity" => "Unique"
        }
        self.catalog().rarity_name(rarity).unwrap_or_else(|| {
            match rarity {
                item::LEGENDARY => Legendary,
                item::SET => Set,
                _ => Unique,
            }
            .into()
        })
    }

    /// The edit that makes the item this unique or set item: the unique's base (the item's own when the unique comes on
    /// it), and the rarity the game gives it. A legendary the game never makes of this unique needs the override, as does
    /// legendary potential or Weaver's Will kept on a unique or set item that does not hold it.
    pub(super) fn identity_edit(&self, item: &ForgeItem, unique_id: u16) -> ItemEdit {
        let unique = self.catalog().unique(unique_id);
        let sub_type = match unique {
            Some(unique) if !unique.sub_types.is_empty() && !unique.sub_types.contains(&item.sub_type) => unique.sub_types[0],
            _ => item.sub_type,
        };
        let rarity = item::identity_rarity(item, unique.is_some_and(|unique| unique.is_set));
        let unrestricted = unique.is_some_and(|unique| (rarity == item::LEGENDARY && !unique.can_drop_as_legendary) || !Self::legendary_fits(item, unique));
        ItemEdit::UniqueIdentity(crate::item::edit::UniqueIdentityEdit {
            unique_id: (unique_id) as i32,
            sub_type: (sub_type) as i32,
            rarity: (rarity) as i32,
            unrestricted,
        })
    }

    /// Whether the unique or set item holds the legendary values the item has (the trainer's own rule).
    pub(super) fn legendary_fits(item: &ForgeItem, unique: &crate::screens::forge::game::UniqueDefinition) -> bool {
        item.context
            .as_ref()
            .is_none_or(|context| crate::item::edit::legendary_fits(context.expected.snapshot(), unique.is_set, i32::from(unique.legendary_type == LegendaryType::WeaversWill)))
    }

    /// What the item keeps that this unique or set item never holds, or None when it holds what the item has: the forge's
    /// risk wording, cut to fit a menu caption.
    pub(super) fn foreign_legendary(item: &ForgeItem, unique: &crate::screens::forge::game::UniqueDefinition) -> Option<String> {
        if Self::legendary_fits(item, unique) {
            return None;
        }
        let status = item.status.unwrap_or_default();
        phrases! {
            Potential: "Keeps legendary potential it never holds, which may crash the game and reset the item."
            Will: "Keeps Weaver's Will it never holds, which may crash the game and reset the item."
            Both: "Keeps legendary potential and Weaver's Will it never holds, which may crash the game and reset the item."
        }
        let kept = match (unique.is_set, status.legendary_potential > 0, status.weavers_will > 0) {
            (false, _, _) if unique.legendary_type == LegendaryType::WeaversWill => Potential,
            (false, _, _) => Will,
            (_, true, true) => Both,
            (_, true, false) => Potential,
            _ => Will,
        };
        Some(kept.into())
    }

    /// The uniques and set items of the item's type, uniques first, each with the rarity it would end with. A
    /// legendary the game never makes, or legendary potential or Weaver's Will kept where the new identity never holds it,
    /// needs the override.
    pub(super) fn identity_options(&self, item: &ForgeItem) -> Vec<FieldOption> {
        let mut uniques: Vec<&crate::screens::forge::game::UniqueDefinition> = self.catalog().uniques.iter().filter(|unique| unique.item_type == item.item_type).collect();
        uniques.sort_by(|a, b| (a.is_set, &a.name).cmp(&(b.is_set, &b.name)));
        uniques
            .into_iter()
            .map(|unique| {
                let game = Some(unique.id) == item.identity_id();
                let edit = self.identity_edit(item, unique.id);
                let (sub_type, rarity) = match edit {
                    ItemEdit::UniqueIdentity(crate::item::edit::UniqueIdentityEdit { sub_type, rarity, .. }) => (sub_type as u16, rarity as u8),
                    _ => (item.sub_type, item.rarity),
                };
                let rarity = if game { item.rarity } else { rarity };
                let mut facts = vec![self.rarity_label(rarity)];
                let mut notes: Vec<String> = Vec::new();
                if let Some(base) = self.catalog().base(item.item_type, sub_type) {
                    facts.push(base.name.clone());
                }
                if !unique.is_set {
                    facts.push(
                        if unique.legendary_type == LegendaryType::WeaversWill {
                            tr::WeaversWill
                        } else {
                            tr::LegendaryPotential
                        }
                        .into(),
                    );
                }
                let mut available = true;
                if !game && unique.is_set && !item.affixes.is_empty() {
                    phrases! { HoldsNone: "Holds no affixes. Remove them first." }
                    notes.push(HoldsNone.into());
                    available = false;
                } else if !game {
                    let mut overrides = Vec::new();
                    if rarity == item::LEGENDARY && !unique.can_drop_as_legendary {
                        phrases! { NeverLegendary: "Never legendary in the game." }
                        overrides.push(NeverLegendary.into());
                    }
                    overrides.extend(Self::foreign_legendary(item, unique));
                    if !overrides.is_empty() {
                        notes.extend(overrides);
                        if !self.unrestricted() {
                            notes.push(tr::NeedsOverride.into());
                        }
                        available = self.unrestricted();
                    }
                    if !edit.can_reverse(item) {
                        phrases! { OneWay: "Undo apply can't reverse it." }
                        notes.push(OneWay.into());
                    }
                }
                // Anything else the trainer would refuse says why, once the override rules above have had their say.
                if !game
                    && available
                    && let Err(error) = edit.prepare_view_with_override(item, self.unrestricted())
                {
                    available = false;
                    notes.push(error.to_string());
                }
                FieldOption {
                    value: i64::from(unique.id),
                    name: unique.name.clone(),
                    facts,
                    note: notes.join(" "),
                    in_game: game,
                    implicits: String::new(),
                    available,
                    rarity,
                }
            })
            .collect()
    }

    /// The identity options the search matches, by name or ID.
    pub fn identity_choices(&self) -> Vec<&FieldOption> {
        let search = self.identity_search.to_lowercase();
        self.identity
            .iter()
            .flat_map(|identity| identity.options.iter())
            .enumerate()
            .filter(|(index, _)| search.is_empty() || self.identity_keys.get(*index).is_some_and(|key| key.contains(&search)))
            .map(|(_, option)| option)
            .collect()
    }

    /// Lowercases the identity options' names once, for the search; the options change only with the item or catalog.
    pub(super) fn index_identities(&mut self) {
        self.identity_keys = self
            .identity
            .iter()
            .flat_map(|identity| identity.options.iter())
            // The separator never comes from a search box, so no match spans the two.
            .map(|option| format!("{}\0{}", option.name.to_lowercase(), option.value))
            .collect();
    }

    pub fn set_identity_search(&mut self, search: &str) {
        self.identity_search = search.into();
    }

    /// Picks a unique from the searched choices; it is drafted alone.
    pub fn pick_identity(&mut self, choice: usize) {
        if !self.can_change_identity() {
            return;
        }
        let Some(value) = self.identity_choices().get(choice).filter(|option| option.available).map(|option| option.value) else {
            return;
        };
        if let Some(identity) = self.identity.as_mut()
            && identity.set(value)
        {
            let uid = identity.uid;
            self.edited(RowRef::Field(uid));
        }
        self.resolve_tables();
    }
}
