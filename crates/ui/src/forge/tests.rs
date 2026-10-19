//! The editor's draft ledger, Apply and Undo on the fixture game, as the C# review scenarios drive them
//! (spec/item-forge.md §10, spec/forge-fixture.json `captureScenarios`).

use super::editor::{Cell, ForgeEditor, MessageKind};
use super::item::Seal;
use crate::forge;

#[path = "../../tests/forge_fixture/mod.rs"]
mod fixture;

use fixture::FixtureGame;

fn ids(editor: &ForgeEditor) -> Vec<u16> {
    editor.affixes.iter().map(|row| row.state.id).collect()
}

fn row(editor: &ForgeEditor, id: u16) -> usize {
    editor.affixes.iter().position(|row| row.state.id == id).unwrap()
}

#[test]
fn drafts_at_each_class_read_as_the_game_shows_them() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::boots(false)));
    editor.pick_tier(2, 4);
    editor.select_affix(Some(0));
    editor.begin_input(Cell::Affix(0));
    editor.set_input(Cell::Affix(0), "+37");
    assert!(editor.commit_input(Cell::Affix(0)));
    let drafts = editor.drafts();
    assert_eq!(drafts.len(), 2);
    assert_eq!(editor.discard_text(), "Discard 2 drafts");
    assert_eq!(editor.apply_text(), "Apply 2 drafts");
    assert_eq!(editor.changes_text(), "Changes (2)");
    assert_eq!(drafts[0].summary, "Value +34 → +37");
    assert_eq!(drafts[0].effect, "+37 Health");
    assert_eq!(drafts[1].summary, "Tier 1 → 4 · Value 4% → 6%");
    assert_eq!(drafts[1].effect, "6% increased Movement Speed");
    assert_eq!(editor.affixes[2].was(), "  was 4%");
    // The tier menu marks the tier the game holds while another is drafted.
    assert_eq!(
        editor
            .tier_options(2)
            .iter()
            .filter(|(_, _, in_game)| *in_game)
            .map(|(tier, _, _)| *tier)
            .collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(editor.affix_list_note(), "Apply or discard the drafts to add or remove an affix.");
    // A ledger entry can be discarded on its own; the other stays.
    editor.discard_entry(1);
    assert_eq!(editor.drafts().len(), 1);
    assert_eq!(editor.message().0, "Draft for Movement Speed discarded.");
}

#[test]
fn applying_two_drafts_confirms_each_and_undo_apply_restores_them() {
    let game = FixtureGame::boots(false);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    editor.clock = || "23:54".into();
    assert_eq!(editor.free_slots_text(), "Free: 1 prefix, 1 suffix");
    editor.begin_input(Cell::Affix(0));
    editor.set_input(Cell::Affix(0), "37");
    editor.commit_input(Cell::Affix(0));
    editor.pick_seal(3, Seal::Normal);
    // A seal-only draft leaves the value as the game's.
    assert!(editor.affixes[3].is_dirty());
    assert!(!editor.affixes[3].is_value_drafted());
    editor.apply();
    assert_eq!(
        editor.message(),
        ("2 drafts applied. Edit readback confirmed for each: Health, Fire Resistance.", MessageKind::Confirmed)
    );
    // Uncorrupting moved Fire Resistance where the game loads a normal affix; each draft landed on its own affix.
    assert_eq!(ids(&editor), [4000, 25, 1148, 4001]);
    assert!(editor.drafts().is_empty());
    assert!(editor.affixes[row(&editor, 25)].confirmed);
    assert!(editor.affixes[row(&editor, 4001)].confirmed);
    assert!(!editor.affixes[row(&editor, 1148)].confirmed);
    assert_eq!(editor.history.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["Fire Resistance", "Health"]);
    assert_eq!(editor.history[0].summary, "Corrupted → Normal");
    assert_eq!(editor.free_slots_text(), "Free: 1 prefix, 0 suffixes");
    assert_eq!(editor.undo_text(), "Undo apply");
    editor.undo();
    assert_eq!(editor.message(), ("Previous values restored; readback confirmed.", MessageKind::Confirmed));
    let item = state.borrow().item.clone().unwrap();
    assert_eq!(item.affixes.iter().find(|affix| affix.id == 25).unwrap().roll, 170);
    assert_eq!(item.affixes.iter().find(|affix| affix.id == 4001).unwrap().seal, Seal::Corrupted);
    assert_eq!(editor.history[0].outcome, "Restored");
    assert_eq!(editor.history[0].summary, "2 applied edits restored");
    assert!(editor.affixes.iter().all(|row| !row.confirmed));
}

#[test]
fn a_draft_the_game_changed_is_refused_with_a_reason() {
    let game = FixtureGame::boots(false);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    editor.step(Cell::Affix(0), 3);
    state.borrow_mut().item.as_mut().unwrap().affixes[0].roll = 120;
    editor.refresh();
    assert!(editor.affixes[0].is_stale());
    editor.apply();
    assert_eq!(
        editor.message(),
        (
            "Health changed in the game after you drafted it. Discard that draft and edit it again. Nothing was applied.",
            MessageKind::Error
        )
    );
    assert!(state.borrow().writes.is_empty());
}

#[test]
fn ctrl_z_steps_back_drafts_only_and_never_writes() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::boots(false)));
    editor.press(Cell::Affix(1));
    for _ in 0..3 {
        editor.scrub(Cell::Affix(1), 0.1);
    }
    editor.end_scrub(Cell::Affix(1), false);
    editor.pick_tier(0, 6);
    assert_eq!(editor.drafts().len(), 2);
    editor.undo_draft_only();
    assert_eq!(editor.selected_affix(), Some(0));
    assert!(!editor.affixes[0].is_dirty());
    editor.undo_draft_only();
    // The whole drag was one step.
    assert!(!editor.affixes[1].is_dirty());
    assert_eq!(editor.message().0, "Last draft change undone.");
    editor.undo_draft_only();
    assert_eq!(editor.message().0, "Last draft change undone.");
}

#[test]
fn a_cancelled_drag_leaves_nothing_to_undo() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::boots(false)));
    editor.press(Cell::Affix(0));
    editor.scrub(Cell::Affix(0), -0.5);
    assert!(editor.affixes[0].is_dirty());
    editor.end_scrub(Cell::Affix(0), true);
    assert!(!editor.affixes[0].is_dirty());
    assert!(!editor.can_undo());
}

#[test]
fn shared_unique_rolls_mirror_and_preview_in_place() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::unique()));
    assert_eq!(editor.summary(), "1 affix · 5 unique modifiers");
    assert_eq!(editor.rarity_text(), "Legendary");
    editor.select_unique(Some(0));
    editor.begin_input(Cell::Unique(0, 0));
    editor.set_input(Cell::Unique(0, 0), "24");
    editor.commit_input(Cell::Unique(0, 0));
    assert_eq!(editor.preview_status, "Draft preview");
    assert_eq!(editor.uniques[0].text, "+24% Movement Speed");
    assert_eq!(editor.uniques[2].text, "Your minions gain 24% Movement Speed");
    // Only the line edited is in the ledger; the linked one previews the change.
    assert_eq!(editor.drafts().len(), 1);
    assert_eq!(editor.drafts()[0].summary, "Value 18% → 24%");
    assert_eq!(editor.linked_with(0), "Linked with \"Your minions gain 24% Movement Speed\"");
    assert_eq!(editor.uniques[3].read_only_text(), "Fixed value");
    assert_eq!(editor.uniques[4].read_only_text(), "Roll unavailable");
}

#[test]
fn status_fields_flags_and_the_base_are_drafts_applied_level_requirement_last() {
    let game = FixtureGame::ring(false);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    assert_eq!(
        editor.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
        ["Forging potential", "Level requirement", "Corruption", "Ruin"]
    );
    assert_eq!(editor.rarity_text(), "Rare");
    assert_eq!(editor.base_text(), "Gold Ring");
    assert_eq!(editor.summary(), "1 affix");
    editor.begin_input(Cell::Field(0));
    editor.set_input(Cell::Field(0), "60");
    editor.commit_input(Cell::Field(0));
    editor.toggle_flag(2, true);
    editor.pick_base(1);
    let drafts = editor.drafts();
    assert_eq!(
        drafts.iter().map(|entry| entry.summary.as_str()).collect::<Vec<_>>(),
        ["30 → 60", "Not corrupted → Corrupted", "Gold Ring → Sapphire Ring"]
    );
    assert_eq!(editor.base_text(), "Sapphire Ring");
    editor.apply();
    assert_eq!(state.borrow().writes.len(), 3);
    let item = state.borrow().item.clone().unwrap();
    assert_eq!(item.status.unwrap().forging_potential, 60);
    assert!(item.status.unwrap().corrupted);
    assert_eq!(item.sub_type, 5);
    assert!(editor.fields[0].confirmed);
    // Raising an affix to T6 makes the item exalted, as a tier draft.
    let options = editor.rarity_options();
    assert_eq!(options[1].detail, "Raises Health to T6");
    editor.pick_rarity(true);
    assert_eq!(editor.rarity_text(), "Exalted");
    assert_eq!(editor.drafts()[0].summary, "Tier 5 → 6");
    assert_eq!(editor.rarity_options()[0].detail, "Lowers Health to T5");
    let _ = forge::item::EXALTED_TIER;
}

#[test]
fn the_picker_replaces_as_a_draft_and_adds_at_once() {
    let game = FixtureGame::boots(false);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    assert!(editor.open_replace(3));
    editor.picker_search("res");
    editor.picker_allow(true);
    assert_eq!(
        editor.picker.as_ref().unwrap().list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["Cold Resistance", "Fire Resistance", "Sentinel's Resolve Reforged"]
    );
    editor.picker_select(0);
    assert_eq!(editor.picker_status(), "Compatible with this slot");
    assert_eq!(editor.picker_subtitle(), "Currently Fire Resistance");
    editor.use_picker();
    assert_eq!(editor.drafts()[0].summary, "Fire Resistance → Cold Resistance");
    editor.discard_all();
    assert_eq!(editor.message().0, "Draft discarded.");
    assert!(editor.open_add());
    editor.picker_search("armour");
    editor.picker_select(0);
    assert_eq!(editor.picker_status(), "Fits this item.");
    editor.use_picker();
    assert_eq!(editor.message(), ("Add affix confirmed by readback: Increased Armour.", MessageKind::Confirmed));
    assert_eq!(editor.affixes.len(), 5);
    assert_eq!(state.borrow().writes.len(), 1);
    editor.undo();
    assert_eq!(editor.affixes.len(), 4);
}

#[test]
fn the_name_drafts_another_unique_alone_with_the_rarity_the_game_gives() {
    let game = FixtureGame::unique_ring();
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    assert!(editor.has_identity());
    assert!(editor.can_change_identity());
    let names: Vec<&str> = editor.identity_choices().iter().map(|option| option.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Fixture Ember Unique",
            "Fixture Potential Unique",
            "Fixture Sapphire Unique",
            "Fixture Woven Unique",
            "Fixture Set Ring"
        ]
    );
    let detail = |editor: &ForgeEditor, name: &str| editor.identity_choices().iter().find(|option| option.name == name).unwrap().detail.clone();
    assert_eq!(detail(&editor, "Fixture Sapphire Unique"), "Unique · Sapphire Ring · legendary potential");
    assert_eq!(detail(&editor, "Fixture Woven Unique"), "Unique · Gold Ring · Weaver's Will");
    assert_eq!(detail(&editor, "Fixture Set Ring"), "Set · Gold Ring");
    editor.set_identity_search("72");
    assert_eq!(editor.identity_choices().len(), 1);
    editor.set_identity_search("");
    editor.pick_identity(2);
    assert!(editor.is_identity_drafted());
    assert_eq!(editor.item_name(), "Fixture Sapphire Unique");
    assert_eq!(editor.base_text(), "Sapphire Ring");
    assert!(editor.is_base_drafted());
    assert_eq!(editor.drafts()[0].summary, "Fixture Potential Unique → Fixture Sapphire Unique");
    // Every other row and field holds still while an identity is drafted.
    assert!(!editor.can_mutate());
    editor.pick_identity(3);
    assert_eq!(editor.base_text(), "Gold Ring");
    editor.apply();
    assert_eq!(editor.message(), ("Edit readback confirmed: Identity.", MessageKind::Confirmed));
    assert_eq!(state.borrow().item.as_ref().unwrap().identity_id(), Some(71));
    assert_eq!(editor.item_name(), "Fixture Woven Unique");
    editor.undo();
    assert_eq!(state.borrow().item.as_ref().unwrap().identity_id(), Some(70));
}

// Delayed command completion exercises the editor between UI event-loop turns.
// Each request is accepted once; changing the observation never retries it.
mod async_writes {
    use super::*;
    use crate::forge::{
        game::{Catalog, EditOutcome, ForgeGame, GameError, WriteOutcome, refused},
        item::{ForgeItem, ItemEdit, ModifierState, RollRequest, ValueTable},
    };
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    #[derive(Clone, Copy)]
    enum ResultKind {
        Confirmed,
        Refused,
        NotKept,
        Uncertain,
        ConfirmedUnavailable,
    }
    struct Control {
        released: bool,
        busy: bool,
        outcomes: VecDeque<ResultKind>,
        pending: Option<(ForgeItem, ItemEdit)>,
    }
    struct Delayed {
        game: FixtureGame,
        control: Rc<RefCell<Control>>,
    }
    impl ForgeGame for Delayed {
        fn is_busy(&self) -> bool {
            self.control.borrow().busy
        }
        fn catalog(&self) -> &Catalog {
            self.game.catalog()
        }
        fn read(&self) -> Option<ForgeItem> {
            self.game.read()
        }
        fn unrestricted(&self) -> bool {
            self.game.unrestricted()
        }
        fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> Result<ValueTable, GameError> {
            self.game.resolve(item, request)
        }
        fn preview_unique(&self, item: &ForgeItem, edit: &ItemEdit) -> Result<Vec<ModifierState>, GameError> {
            self.game.preview_unique(item, edit)
        }
        fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
            let mut c = self.control.borrow_mut();
            assert!(c.pending.is_none());
            c.pending = Some((expected.clone(), edit.clone()));
            WriteOutcome::Pending
        }
        fn poll_write(&mut self) -> Option<EditOutcome> {
            let mut c = self.control.borrow_mut();
            if !c.released {
                return None;
            }
            c.released = false;
            let (expected, edit) = c.pending.take()?;
            let result = c.outcomes.pop_front().unwrap_or(ResultKind::Confirmed);
            Some(match result {
                ResultKind::Confirmed => self.game.write(&expected, &edit),
                ResultKind::ConfirmedUnavailable => {
                    assert!(matches!(self.game.write(&expected, &edit), WriteOutcome::Confirmed(_)));
                    self.game.state.borrow_mut().item = None;
                    WriteOutcome::Confirmed(None)
                }
                ResultKind::Refused => refused("Refused before any change."),
                ResultKind::NotKept => WriteOutcome::NotKept(self.game.read()),
                ResultKind::Uncertain => WriteOutcome::uncertain("Independent readback was lost."),
            })
        }
    }
    fn editor(outcomes: &[ResultKind]) -> (ForgeEditor, Rc<RefCell<Control>>, Rc<RefCell<fixture::FixtureState>>) {
        let game = FixtureGame::ring(false);
        let state = game.state.clone();
        let control = Rc::new(RefCell::new(Control {
            released: false,
            busy: false,
            outcomes: outcomes.iter().copied().collect(),
            pending: None,
        }));
        (ForgeEditor::new(Box::new(Delayed { game, control: control.clone() })), control, state)
    }
    fn release(editor: &mut ForgeEditor, control: &Rc<RefCell<Control>>) {
        control.borrow_mut().released = true;
        editor.refresh();
    }
    fn draft(editor: &mut ForgeEditor) {
        editor.begin_input(Cell::Field(0));
        editor.set_input(Cell::Field(0), "60");
        assert!(editor.commit_input(Cell::Field(0)));
        editor.toggle_flag(2, true);
    }
    #[test]
    fn mixed_draft_waits_between_edits_and_undo_reverses_only_confirmed_steps() {
        let (mut e, c, state) = editor(&[ResultKind::Confirmed, ResultKind::Confirmed]);
        draft(&mut e);
        e.apply();
        assert!(!e.can_apply());
        assert!(!e.can_discard_all());
        assert!(!e.can_undo());
        assert!(state.borrow().writes.is_empty());
        e.apply();
        e.undo();
        e.refresh();
        assert!(state.borrow().writes.is_empty());
        release(&mut e, &c);
        assert_eq!(state.borrow().writes.len(), 1);
        assert_eq!(e.drafts().len(), 1);
        let expected = c.borrow().pending.as_ref().unwrap().0.clone();
        assert_eq!(expected, state.borrow().item.clone().unwrap());
        release(&mut e, &c);
        assert!(e.drafts().is_empty());
        assert_eq!(e.history.len(), 2);
        assert!(e.can_undo());
        e.undo();
        release(&mut e, &c);
        release(&mut e, &c);
        assert_eq!(state.borrow().item.as_ref().unwrap().status.unwrap().forging_potential, 30);
        assert!(!state.borrow().item.as_ref().unwrap().status.unwrap().corrupted);
        assert!(!e.can_undo());
    }
    #[test]
    fn ordinary_refusal_stops_the_batch_and_preserves_confirmed_undo() {
        let (mut e, c, state) = editor(&[ResultKind::Confirmed, ResultKind::Refused]);
        draft(&mut e);
        e.apply();
        release(&mut e, &c);
        release(&mut e, &c);
        assert_eq!(state.borrow().writes.len(), 1);
        assert_eq!(e.drafts().len(), 1);
        assert!(e.message().0.contains("1 of 2"));
        e.discard_all();
        assert!(e.undo_writes_to_game());
        e.undo();
        release(&mut e, &c);
        assert!(!e.can_undo());
        assert_eq!(state.borrow().item.as_ref().unwrap().status.unwrap().forging_potential, 30);
    }
    #[test]
    fn not_kept_and_uncertain_clear_every_draft_and_undo_after_partial_confirmation() {
        for result in [ResultKind::NotKept, ResultKind::Uncertain] {
            let (mut e, c, state) = editor(&[ResultKind::Confirmed, result]);
            draft(&mut e);
            e.apply();
            release(&mut e, &c);
            release(&mut e, &c);
            assert!(e.drafts().is_empty());
            if matches!(result, ResultKind::Uncertain) {
                assert!(e.history.is_empty());
            } else {
                assert_eq!(e.history.len(), 1);
                assert_eq!(e.history[0].outcome, "Confirmed");
            }
            assert!(!e.can_undo());
            assert_eq!(state.borrow().writes.len(), 1);
            assert_eq!(e.has_item(), matches!(result, ResultKind::NotKept));
            assert!(e.message().0.contains(if matches!(result, ResultKind::NotKept) { "different values" } else { "uncertain" }));
        }
    }
    #[test]
    fn undo_refusal_keeps_the_unrestored_reverse_without_replaying_confirmed_reverses() {
        let (mut e, c, state) = editor(&[ResultKind::Confirmed, ResultKind::Confirmed, ResultKind::Confirmed, ResultKind::Refused]);
        draft(&mut e);
        e.apply();
        release(&mut e, &c);
        release(&mut e, &c);
        e.undo();
        release(&mut e, &c);
        release(&mut e, &c);
        assert!(e.undo_writes_to_game());
        assert_eq!(state.borrow().writes.len(), 3);
        e.undo();
        release(&mut e, &c);
        assert_eq!(state.borrow().writes.len(), 4);
        assert!(!e.can_undo());
    }
    #[test]
    fn a_monitor_read_between_confirmations_delays_the_next_step_without_refusing_or_replaying() {
        let (mut e, c, state) = editor(&[ResultKind::Confirmed, ResultKind::Confirmed]);
        draft(&mut e);
        e.apply();
        c.borrow_mut().busy = true;
        release(&mut e, &c);
        assert_eq!(state.borrow().writes.len(), 1);
        assert!(c.borrow().pending.is_none());
        e.refresh();
        assert_eq!(state.borrow().writes.len(), 1);
        assert!(!e.can_apply());
        c.borrow_mut().busy = false;
        e.refresh();
        assert!(c.borrow().pending.is_some());
        release(&mut e, &c);
        assert_eq!(state.borrow().writes.len(), 2);
        assert!(e.drafts().is_empty());
        assert!(e.undo_writes_to_game());
    }

    #[test]
    fn a_confirmed_but_undisplayable_readback_does_not_become_uncertain() {
        let (mut e, c, state) = editor(&[ResultKind::ConfirmedUnavailable]);
        draft(&mut e);
        e.apply();
        release(&mut e, &c);
        assert_eq!(state.borrow().writes.len(), 1);
        assert!(!e.has_item());
        assert!(e.drafts().is_empty());
        assert!(!e.can_undo());
        assert_eq!(e.history[0].outcome, "Confirmed");
        assert!(e.message().0.contains("edit was confirmed"));
        assert!(!e.message().0.contains("uncertain"));
    }

    #[test]
    fn a_new_observation_invalidates_applied_undo_instead_of_overwriting_external_changes() {
        let (mut e, c, state) = editor(&[ResultKind::Confirmed]);
        e.toggle_flag(2, true);
        e.apply();
        release(&mut e, &c);
        assert!(e.undo_writes_to_game());
        state.borrow_mut().item.as_mut().unwrap().status.as_mut().unwrap().forging_potential = 99;
        e.refresh();
        e.undo();
        assert!(c.borrow().pending.is_none());
        assert_eq!(state.borrow().writes.len(), 1);
        assert!(!e.can_undo());
    }

    #[test]
    fn override_revocation_between_confirmation_and_undo_refuses_before_submission() {
        let (mut e, c, state) = editor(&[ResultKind::Confirmed]);
        state.borrow_mut().unrestricted = true;
        state.borrow_mut().item.as_mut().unwrap().rarity = forge::item::LEGENDARY;
        e.refresh();
        let index = e.fields.iter().position(|f| f.unrestricted).unwrap();
        e.begin_input(Cell::Field(index));
        e.set_input(Cell::Field(index), "2");
        assert!(e.commit_input(Cell::Field(index)));
        e.apply();
        release(&mut e, &c);
        state.borrow_mut().unrestricted = false;
        e.refresh();
        e.undo();
        assert!(c.borrow().pending.is_none());
        assert_eq!(state.borrow().writes.len(), 1);
        assert!(e.message().0.contains("I know"));
    }
}
