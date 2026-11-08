//! Unique modifiers' preview and the value tables each cell reads.

use super::*;

impl ForgeEditor {
    /// Shared rolls are one game value: every line that reads them shows the same draft.
    pub(super) fn mirror_shared(&mut self, row: usize) {
        let rolls: Vec<(usize, u8)> = self.uniques[row].rolls.iter().map(|roll| (roll.index, roll.value)).collect();
        for (index, other) in self.uniques.iter_mut().enumerate() {
            if index != row {
                for (roll, value) in &rolls {
                    other.mirror(*roll, *value);
                }
            }
        }
    }

    pub(super) fn unique_edit(&self) -> Option<ItemEdit> {
        let mut changes: Vec<(usize, u8)> = Vec::new();
        for row in self.uniques.iter().filter(|row| row.is_dirty()) {
            for change in row.changes() {
                if !changes.iter().any(|(index, _)| *index == change.0) {
                    changes.push(change);
                }
            }
        }
        (!changes.is_empty()).then_some(ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
            target: RollTarget::Unique,
            changes: item::roll_changes(changes),
            allow_incompatible: self.unrestricted(),
        }))
    }

    pub(super) fn say_preview(&mut self, status: &str, failed: bool, stale: bool) {
        self.preview_status = status.into();
        self.preview_failed = failed;
        self.preview_stale = stale;
        if status.is_empty() {
            if self.unique_edit().is_none() {
                self.game.clear_unique_preview();
            }
            for row in self.uniques.iter_mut() {
                row.show_preview(None);
            }
        }
    }

    /// The adapter coalesces drafts for 150 ms and publishes only the exact current preview.
    pub(super) fn refresh_unique_preview(&mut self) {
        self.say_preview("", false, false);
        let (Some(edit), Some(item)) = (self.unique_edit(), self.item.as_ref()) else {
            return;
        };
        match self.game.preview_unique(item, &edit) {
            Lookup::Ready(modifiers) => {
                for state in modifiers {
                    if let Some(row) = self.uniques.iter_mut().find(|row| row.state.index == state.index) {
                        row.show_preview(Some(&state));
                    }
                }
                phrases! { DraftPreview: "Draft preview" }
                self.preview_status = DraftPreview.into();
            }
            Lookup::Loading => self.say_preview(crate::forge::game::PreviewLoading.get(), false, false),
            Lookup::Unavailable(details) => {
                phrases! { PreviewUnavailable: "Preview unavailable: {details}" }
                self.say_preview(&PreviewUnavailable.with(&[("details", &details)]), true, false)
            }
        }
    }

    /// The other lines that read one of this line's rolls, as they show now.
    pub fn linked_with(&self, row: usize) -> String {
        let Some(line) = self.uniques.get(row) else {
            return String::new();
        };
        let others: Vec<String> = self
            .uniques
            .iter()
            .enumerate()
            .filter(|(index, other)| *index != row && other.rolls.iter().any(|o| line.rolls.iter().any(|r| r.index == o.index)))
            .map(|(_, other)| format!("\"{}\"", other.text))
            .collect();
        if others.is_empty() {
            String::new()
        } else {
            phrases! { Linked: "Linked with {names}" }
            Linked.with(&[("names", &list(&others, Conjunction::And))])
        }
    }

    // ----- Value tables -----

    /// Asks the game for each cell's table in its draft's context; a context already asked is not asked again.
    pub(super) fn resolve_tables(&mut self) {
        if self.writes.is_running() {
            return;
        }
        let changes = match self.unique_edit() {
            Some(ItemEdit::Roll(eg_trainer::item::edit::RollEdit { changes, .. })) => changes,
            _ => Vec::new(),
        };
        let unrestricted = self.unrestricted();
        let ForgeEditor {
            game,
            item,
            tables,
            affixes,
            uniques,
            implicits,
            ..
        } = self;
        let game = &*game;
        let Some(item) = item.as_ref() else {
            game.retain_mappings(&[]);
            return;
        };
        let mut requests = Vec::new();
        for row in affixes.iter_mut() {
            // The game reads values for affixes it holds; an added one waits for Apply.
            if row.added {
                continue;
            }
            let (slot, id, tier, seal) = (row.slot(), row.draft_id(), row.tier(), row.seal());
            let key = TableKey::Affix {
                slot,
                id,
                tier,
                seal: seal.index(),
            };
            let request = RollRequest {
                target: RollTarget::Affix,
                index: slot,
                draft: (id != row.state.id || tier != row.state.tier || seal != row.state.seal).then_some(ItemEdit::Affix(eg_trainer::item::edit::AffixEdit {
                    slot: (slot) as i32,
                    id: (id) as i32,
                    tier: (tier) as i32,
                    roll: 0,
                    seal,
                    allow_incompatible: row.allows_incompatible(),
                })),
                line: None,
            };
            let game_context = row.game_roll().is_some();
            set_table(&mut row.value, table(tables, game, key, item, &request), game_context);
            requests.push(request);
        }
        for line in uniques.iter_mut() {
            let line_index = line.state.index as usize;
            for roll in &mut line.rolls {
                let index = roll.index;
                let dependencies: Vec<(usize, u8)> = changes.iter().filter(|c| c.index != index as i32).map(|c| (c.index as usize, c.value as u8)).collect();
                let request = RollRequest {
                    target: RollTarget::Unique,
                    index,
                    draft: (!dependencies.is_empty()).then(|| {
                        ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
                            target: RollTarget::Unique,
                            changes: item::roll_changes(dependencies.iter().copied()),
                            allow_incompatible: unrestricted,
                        })
                    }),
                    line: Some(line_index),
                };
                let key = TableKey::Unique {
                    index,
                    line: line_index,
                    dependencies,
                };
                set_table(&mut roll.editor, table(tables, game, key, item, &request), true);
                requests.push(request);
            }
        }
        for line in implicits.iter_mut() {
            for roll in &mut line.rolls {
                let request = RollRequest {
                    target: RollTarget::Implicit,
                    index: roll.index,
                    draft: None,
                    line: None,
                };
                set_table(&mut roll.editor, table(tables, game, TableKey::Implicit(roll.index), item, &request), true);
                requests.push(request);
            }
        }
        game.retain_mappings(&requests);
    }
}
