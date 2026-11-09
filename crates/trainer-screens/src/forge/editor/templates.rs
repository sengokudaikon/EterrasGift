//! Item templates: saved from the item, and drafted onto it as ordinary drafts.

use super::*;
use eg_trainer::item::template;

impl ForgeEditor {
    /// A template's caption, each its own text: what it holds, e.g. ["Whole item", "4 affixes", "Health, Movement Speed, …"].
    pub fn template_detail(&self, template: &ItemTemplate) -> Vec<String> {
        let names: Vec<String> = template.affixes.iter().take(3).map(|affix| self.catalog().definition(affix.id).name).collect();
        phrases! {
            WholeItem: "Whole item"
            AffixesOnly: "Affixes"
        }
        let more = if template.affixes.len() > 3 { ", …" } else { "" };
        let kind = if template.item.is_some() { WholeItem } else { AffixesOnly };
        let mut facts = vec![kind.into(), tr::Affixes.count(template.affixes.len())];
        if !names.is_empty() {
            facts.push(format!("{}{more}", names.join(", ")));
        }
        facts
    }

    /// The kept template that saving under the typed name would replace: the same name in any case, or one whose file name
    /// the typed one shares.
    pub fn template_replaces(&self) -> Option<String> {
        template::taken_by(&self.templates, &self.template_name)
    }

    /// The item as Apply would leave it, as a template to keep under `name`, with the kept one it would replace: its affixes,
    /// or with `whole` the item too (base, unique and its rolls, implicit rolls, status). Or why it can't be kept.
    pub fn template_to_save(&self, name: &str, whole: bool) -> Result<(ItemTemplate, Option<String>), String> {
        phrases! {
            NotKept: "Templates are not kept in this run."
            Unnamed: "Name the template first."
        }
        if !self.keeps_templates {
            return Err(NotKept.into());
        }
        let name = ItemTemplate::clean_name(name);
        if name.is_empty() {
            return Err(Unnamed.into());
        }
        let item = self.drafted_item().or_else(|| (*self.item).clone()).ok_or(tr::NoItem.get())?;
        let replaced = template::taken_by(&self.templates, &name);
        let affixes = item
            .affixes
            .iter()
            .filter(|affix| affix.unknown.is_none())
            .map(|affix| TemplateAffix {
                id: affix.id,
                tier: affix.tier.clamp(1, 8),
                seal: affix.seal as u8,
                roll: affix.roll,
            })
            .collect();
        let item = whole.then(|| {
            let status = item.status.unwrap_or_default();
            TemplateItem {
                item_type: u8::try_from(item.item_type).unwrap_or(u8::MAX),
                sub_type: item.sub_type,
                identity: item.backing().map(|identity| (identity.id, identity.rolls.clone())),
                implicit_rolls: item.implicit_rolls.clone(),
                status: TemplateStatus {
                    forging_potential: status.forging_potential,
                    legendary_potential: status.legendary_potential,
                    weavers_will: status.weavers_will,
                    weavers_touch: status.weavers_touch,
                    corrupted: status.corrupted,
                    ruined: status.ruined,
                },
            }
        });
        Ok((ItemTemplate { name, affixes, item }, replaced))
    }

    /// A save came back: `error` says why it failed. Saved, the typed name is done with.
    pub fn template_saved(&mut self, name: &str, replaced: Option<String>, error: Option<String>) {
        phrases! {
            NotSaved: "The template could not be saved: {error}"
            Replaced: "Replaced \"{name}\"."
            SavedOver: "Saved \"{name}\" in place of \"{kept}\"."
            Saved: "Saved \"{name}\"."
        }
        // A name typed since the save was asked for is the next one, not this.
        if error.is_none() && self.template_name == name {
            self.template_name.clear();
        }
        self.template_message = match (error, replaced) {
            (Some(error), _) => NotSaved.with(&[("error", &error)]),
            (None, Some(kept)) if kept == name => Replaced.with(&[("name", &name)]),
            (None, Some(kept)) => SavedOver.with(&[("name", &name), ("kept", &kept)]),
            (None, None) => Saved.with(&[("name", &name)]),
        };
    }

    /// A delete came back: `error` says why it failed.
    pub fn template_deleted(&mut self, name: &str, error: Option<String>) {
        phrases! {
            Deleted: "Deleted \"{name}\"."
            NotDeleted: "The template could not be deleted: {error}"
        }
        self.template_message = match error {
            None => Deleted.with(&[("name", &name)]),
            Some(error) => NotDeleted.with(&[("error", &error)]),
        };
    }

    /// Drafts a template onto the item, as ordinary drafts checked like any other; what does not fit is left out and
    /// named. A whole-item template whose unique differs drafts that unique alone (a new unique brings its own
    /// modifiers): apply the template again after it for the rest.
    pub fn apply_template(&mut self, index: usize) {
        let Some(template) = self.templates.get(index).cloned() else {
            return;
        };
        let (message, kind) = match self.applied_template(&template) {
            Ok(message) => (message, MessageKind::Info),
            Err(problem) => (problem, MessageKind::Error),
        };
        self.say(message.clone(), kind);
        self.template_message = message;
        self.resolve_tables();
    }

    pub(super) fn applied_template(&mut self, template: &ItemTemplate) -> Result<String, String> {
        phrases! {
            Writing: "Wait for the write in progress to finish, then draft the template."
            NotReady: "The item can't take drafts until the game is ready."
            HasDrafts: "Apply or discard the drafts first: a template drafts the whole item."
            OtherKind: "That template is for another kind of item."
            UniqueOnly: "Drafted only the unique from \"{name}\": a new unique brings its own modifiers. Apply it, then draft the template again for the rest."
            UniqueMisfits: "That template's unique does not fit this item."
            NotBacked: "That template holds a unique, set or legendary item, and this item is none of them."
            Base: "Base"
            Matches: "\"{name}\" matches the item already."
            Drafted: "Drafted \"{name}\": {n} draft. Review it, then Apply." | "Drafted \"{name}\": {n} drafts. Review them, then Apply."
            LeftOut: "Left out: {items}."
        }
        let item = (*self.item).clone().ok_or(tr::NoItem.get())?;
        if self.writes.is_running() {
            return Err(Writing.into());
        }
        if !self.can_change_draft() {
            return Err(NotReady.into());
        }
        if self.draft_count() > 0 {
            return Err(HasDrafts.into());
        }
        let mut left_out = Vec::new();
        if let Some(whole) = &template.item {
            if u16::from(whole.item_type) != item.item_type {
                return Err(OtherKind.into());
            }
            match (&whole.identity, item.backing()) {
                (Some((id, _)), Some(current)) if *id != current.id => {
                    self.identity_search.clear();
                    let choice = self.identity_choices().iter().position(|option| option.value == i64::from(*id) && option.available);
                    return match choice {
                        Some(choice) => {
                            self.pick_identity(choice);
                            Ok(UniqueOnly.with(&[("name", &template.name)]))
                        }
                        None => Err(UniqueMisfits.into()),
                    };
                }
                (Some(_), None) => return Err(NotBacked.into()),
                _ => {}
            }
            if item.backing().is_none() && whole.sub_type != item.sub_type {
                let key = base_key(item.item_type, whole.sub_type);
                match self
                    .base
                    .as_ref()
                    .and_then(|base| base.options.iter().position(|option| option.value == key && option.available))
                {
                    Some(option) => self.pick_base(option),
                    None => left_out.push(Base.into()),
                }
            }
            let status = whole.status;
            for index in 0..self.fields.len() {
                let field = &self.fields[index];
                let value = match field.field {
                    Some(StatusField::ForgingPotential) => i64::from(status.forging_potential),
                    Some(StatusField::LegendaryPotential) => i64::from(status.legendary_potential),
                    Some(StatusField::WeaversWill) => i64::from(status.weavers_will),
                    Some(StatusField::WeaversTouch) => i64::from(status.weavers_touch),
                    Some(StatusField::Corrupted) => i64::from(status.corrupted),
                    Some(StatusField::Ruined) => i64::from(status.ruined),
                    _ => continue,
                };
                if field.value() == value {
                    continue;
                }
                if !self.can_change_field(index) {
                    left_out.push(field.name.clone());
                    continue;
                }
                if self.fields[index].set(value) {
                    let uid = self.fields[index].uid;
                    self.edited(RowRef::Field(uid));
                }
            }
            if let Some((_, rolls)) = &whole.identity {
                let mut cells = Vec::new();
                for (row, line) in self.uniques.iter().enumerate() {
                    for (position, roll) in line.rolls.iter().enumerate() {
                        if let Some(value) = rolls.get(roll.index).filter(|value| **value != roll.value) {
                            cells.push((Cell::Unique(row, position), *value));
                        }
                    }
                }
                for (cell, value) in cells {
                    if self.can_edit_value(cell) {
                        self.set_cell(cell, value);
                    }
                }
            }
            // A drafted base brings its own implicits: their rolls carry over when it applies.
            if !self.is_base_drafted() {
                let mut cells = Vec::new();
                for (row, line) in self.implicits.iter().enumerate() {
                    for (position, roll) in line.rolls.iter().enumerate() {
                        if let Some(value) = whole.implicit_rolls.get(roll.index).filter(|value| **value != roll.value) {
                            cells.push((Cell::Implicit(row, position), *value));
                        }
                    }
                }
                for (cell, value) in cells {
                    if self.can_edit_value(cell) {
                        self.set_cell(cell, value);
                    }
                }
            }
        }
        self.template_affixes(&template.affixes, &mut left_out);
        let drafted = self.draft_count();
        let mut message = match drafted {
            0 => Matches.with(&[("name", &template.name)]),
            n => Drafted.with(n, &[("name", &template.name)]),
        };
        if !left_out.is_empty() {
            message.push(' ');
            message.push_str(&LeftOut.with(&[("items", &left_out.join("; "))]));
        }
        Ok(message)
    }

    /// The template's affixes as drafts: an affix both hold takes the template's tier, seal and roll; one only the item
    /// holds is removed; one only the template holds is added where it may be (removals first, so their slots are free).
    pub(super) fn template_affixes(&mut self, affixes: &[TemplateAffix], left_out: &mut Vec<String>) {
        for index in 0..self.affixes.len() {
            let row = &self.affixes[index];
            if row.added {
                continue;
            }
            let wanted = affixes.iter().find(|affix| affix.id == row.state.id);
            if !row.is_editable() {
                if wanted.is_none_or(|affix| affix.tier != row.state.tier || affix.roll != row.state.roll || affix.seal != row.state.seal as u8) {
                    phrases! { ReadOnly: "{name} (read only)" }
                    left_out.push(ReadOnly.with(&[("name", &row.name())]));
                }
                continue;
            }
            match wanted {
                Some(affix) => {
                    let row = &mut self.affixes[index];
                    let seal = Seal::ALL[usize::from(affix.seal.min(3))];
                    let tier = affix.tier.min(row.max_tier()).max(1);
                    let mut changed = row.draft_tier(tier, seal);
                    changed |= row.set_roll(affix.roll);
                    if changed {
                        let uid = row.uid;
                        self.edited(RowRef::Affix(uid));
                    }
                }
                None => self.remove_affix(index),
            }
        }
        for affix in affixes {
            if self.affixes.iter().any(|row| !row.added && row.state.id == affix.id) {
                continue;
            }
            let definition = self.catalog().definition(affix.id);
            let Some(drafted) = self.drafted_item() else {
                return;
            };
            let seal = Seal::ALL[usize::from(affix.seal.min(3))];
            let tier = affix.tier.min(definition.max_tier.clamp(1, 8));
            let allow = self.unrestricted();
            match self.addition(&drafted, &definition, tier, affix.roll, seal, allow) {
                Ok(ItemEdit::AddAffix(add)) => {
                    let uid = self.uid();
                    self.affixes.push(AffixRow::added(uid, definition, add.tier as u8, add.roll as u8, add.seal, allow));
                    self.edited(RowRef::Affix(uid));
                }
                Ok(_) => {}
                Err(problem) => {
                    phrases! { NotAdded: "{name} ({problem})" }
                    left_out.push(NotAdded.with(&[("name", &definition.name), ("problem", &problem)]));
                }
            }
        }
    }
}
