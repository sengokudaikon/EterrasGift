//! The editor's draft ledger, Apply and Undo on the fixture game, driven through the review scenarios.

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
fn authored_writes_and_undo_enforce_the_canonical_roll_override_flag() {
    use super::game::{ForgeGame, WriteOutcome};
    use eg_trainer::item::edit::{ItemEdit, RollChange, RollEdit, RollTarget};
    let mut game = FixtureGame::boots(false);
    let before = game.read().unwrap();
    let edit = ItemEdit::Roll(RollEdit {
        target: RollTarget::Affix,
        changes: vec![RollChange { index: 0, value: 42 }],
        allow_incompatible: true,
    });
    assert!(matches!(game.write(&before, &edit), WriteOutcome::NotApplied { .. }));
    assert!(game.state.borrow().writes.is_empty());
    game.state.borrow_mut().unrestricted = true;
    assert!(matches!(game.write(&before, &edit), WriteOutcome::Confirmed(_)));
    let after = game.read().unwrap();
    game.state.borrow_mut().unrestricted = false;
    assert!(matches!(game.undo(&after), WriteOutcome::NotApplied { .. }));
    assert_eq!(game.state.borrow().writes.len(), 1);
}

#[test]
fn capture_compatibility_uses_captured_class_requirements() {
    use super::{
        game::ForgeGame,
        item::{Compatibility, Rejection},
    };
    use std::sync::Arc;
    let game = FixtureGame::boots(false);
    let mut item = game.read().unwrap();
    let mut context = Arc::unwrap_or_clone(item.context.take().unwrap());
    let mut catalog = Arc::unwrap_or_clone(context.catalog);
    catalog.affixes.get_mut(&4002).unwrap().class_requirement = 4;
    let mut snapshot = context.expected.snapshot().clone();
    snapshot.class_requirement = 1;
    context.expected = eg_trainer::item::edit::ExpectedObservation::new(
        context.expected.generation(),
        context.expected.process().clone(),
        context.expected.catalog_identity().clone(),
        snapshot,
    )
    .unwrap();
    context.catalog = Arc::new(catalog);
    item.context = Some(Arc::new(context));
    assert_eq!(
        game.compatibility(&item, &game.catalog().definition(4002), None, false),
        Compatibility::Rejected(Rejection::ClassRequirement)
    );
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
    // Drafts no longer hold the affix list still: it shows the free slots.
    assert_eq!(editor.free_slots_text(), "Free: 1 prefix, 1 suffix");
    assert!(editor.can_change_affix_list());
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
fn max_tiers_drafts_every_affix_up_to_t7_and_leaves_a_primordial() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::boots(false)));
    editor.pick_tier(0, 8);
    editor.pick_seal(0, Seal::Primordial);
    assert!(editor.can_max_tiers());
    editor.max_tiers();
    let tiers: Vec<u8> = editor.affixes.iter().map(|row| row.tier()).collect();
    assert_eq!(tiers, [8, 7, 7, 7]);
    assert_eq!(editor.affixes[0].seal(), Seal::Primordial);
    assert!(!editor.can_max_tiers());
    assert_eq!(editor.max_tiers_tip(), "Every affix is at its highest tier below T8.");
}

#[test]
fn an_idol_takes_another_idol_type_only_with_the_override() {
    use eg_trainer::item::edit::{BaseTypeEdit, ItemEdit};
    let editor = ForgeEditor::new(Box::new(FixtureGame::idol()));
    let options = editor.base.as_ref().unwrap().options.clone();
    assert_eq!(
        options.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        ["Small Lagonian Idol", "Small Eterran Idol", "Humble Eterran Idol"]
    );
    assert!(!options[2].available);
    assert!(options[2].detail.starts_with("Humble Idol"));
    let game = FixtureGame::idol();
    game.state.borrow_mut().unrestricted = true;
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    editor.pick_base(2);
    assert_eq!(editor.base_text(), "Humble Eterran Idol");
    editor.apply();
    assert_eq!(
        state.borrow().writes.last(),
        Some(&ItemEdit::BaseType(BaseTypeEdit {
            sub_type: 0,
            allow_incompatible: false,
            item_type: Some(26),
        }))
    );
    assert_eq!(state.borrow().item.as_ref().unwrap().item_type, 26);
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

/// Each base says how many affixes it would make incompatible, judged with the class requirement the base carries, and
/// Apply refuses such a base unless the base picker allows it.
#[test]
fn a_base_that_breaks_affixes_says_so_and_needs_allowing() {
    use eg_trainer::item::edit::{BaseTypeEdit, ItemEdit};
    let game = FixtureGame::class_ring();
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    let details: Vec<String> = editor.base.as_ref().unwrap().options.iter().map(|option| option.detail.clone()).collect();
    assert_eq!(
        details,
        ["Level 20", "Level 28 · 2 incompatible affixes", "Level 40 · corrupted base · 1 incompatible affix"]
    );

    // Refused whole, with the way out in words; nothing is written.
    editor.pick_base(1);
    editor.apply();
    assert_eq!(
        editor.message(),
        (
            "The new base would make affixes incompatible. Allow incompatible affixes in the base picker to apply it anyway.",
            MessageKind::Error
        )
    );
    assert!(state.borrow().writes.is_empty());
    // With another draft pending, the refusal says nothing was applied.
    editor.begin_input(Cell::Field(0));
    editor.set_input(Cell::Field(0), "60");
    editor.commit_input(Cell::Field(0));
    editor.apply();
    assert!(editor.message().0.ends_with("apply it anyway. Nothing was applied."));
    assert!(state.borrow().writes.is_empty());
    editor.discard_all();

    // Allowed, the base goes past what the game allows: it needs "I know what I'm doing".
    editor.pick_base(1);
    editor.set_base_allow_incompatible(true);
    editor.apply();
    assert_eq!(editor.message(), (super::editor::OVERRIDE_REQUIRED, MessageKind::Error));
    assert!(state.borrow().writes.is_empty());
    state.borrow_mut().unrestricted = true;
    editor.apply();
    assert_eq!(editor.message().1, MessageKind::Confirmed);
    assert!(matches!(
        state.borrow().writes.as_slice(),
        [ItemEdit::BaseType(BaseTypeEdit {
            sub_type: 5,
            allow_incompatible: true,
            item_type: None
        })]
    ));
    assert_eq!(state.borrow().item.as_ref().unwrap().sub_type, 5);
}

/// Incompatibility the item already carries is not the new base's doing, and "Allow incompatible affixes" on a base that
/// breaks nothing needs no override.
#[test]
fn only_affixes_the_base_breaks_count() {
    use eg_trainer::item::edit::{BaseTypeEdit, ItemEdit};
    // On the Sapphire Ring (any class) both class affixes are already incompatible; back to the Gold Ring breaks none.
    let game = FixtureGame::class_ring();
    game.change(|item| item.sub_type = 5);
    let editor = ForgeEditor::new(Box::new(game));
    let details: Vec<String> = editor.base.as_ref().unwrap().options.iter().map(|option| option.detail.clone()).collect();
    assert_eq!(details, ["Level 20", "Level 28", "Level 40 · corrupted base"]);

    let game = FixtureGame::ring(false);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    editor.pick_base(1);
    editor.set_base_allow_incompatible(true);
    editor.apply();
    assert_eq!(editor.message().1, MessageKind::Confirmed, "{}", editor.message().0);
    assert!(matches!(
        state.borrow().writes.as_slice(),
        [ItemEdit::BaseType(BaseTypeEdit { allow_incompatible: false, .. })]
    ));
}

/// Additions and removals are drafts in the ledger like the others: Apply writes removals first and additions last, and
/// one Undo apply takes the whole batch back.
#[test]
fn adding_and_removing_are_drafts_applied_in_order_and_undone_together() {
    use eg_trainer::item::edit::{AddAffixEdit, ItemEdit, RemoveAffixEdit};
    let game = FixtureGame::boots(true);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    editor.clock = || "23:54".into();
    let before = ids(&editor);
    assert_eq!(editor.free_slots_text(), "Free: 1 prefix, 1 suffix");

    // An addition is a row of its own after the game's, selected, with no value until the game holds it.
    assert!(editor.open_add());
    editor.picker_search("armour");
    editor.picker_select(0);
    assert_eq!(editor.picker_status(), "Fits this item.");
    editor.use_picker();
    assert!(state.borrow().writes.is_empty());
    assert_eq!(editor.affixes.len(), 5);
    assert!(editor.affixes[4].added);
    assert_eq!(editor.selected_affix(), Some(4));
    assert_eq!(editor.drafts()[0].summary, "Added · Tier 5");
    assert!(!editor.can_edit_value(Cell::Affix(4)));
    // Its range comes from game data, the tier's.
    assert_eq!(editor.range_text(4), editor.catalog().tier_range(editor.affixes[4].state.id, 5));
    assert!(!editor.range_text(4).is_empty());
    assert_ne!(editor.free_slots_text(), "Free: 1 prefix, 1 suffix");
    // Its tier is a draft like any other, and Ctrl+Z steps it back before taking the addition away.
    editor.pick_tier(4, 3);
    assert_eq!(editor.drafts()[0].summary, "Added · Tier 3");
    editor.undo_draft_only();
    assert_eq!(editor.drafts()[0].summary, "Added · Tier 5");
    editor.undo_draft_only();
    assert_eq!(editor.affixes.len(), 4);
    assert!(editor.drafts().is_empty());

    // A removal keeps its row, which takes no other draft until the affix is kept again.
    editor.remove_affix(0);
    assert!(editor.affixes[0].is_removed());
    assert_eq!(editor.drafts()[0].summary, "Removed");
    assert_eq!(editor.summary(), "3 affixes");
    editor.pick_tier(0, 2);
    assert!(!editor.can_edit_value(Cell::Affix(0)));
    assert_eq!(editor.drafts().len(), 1);
    editor.remove_affix(0);
    assert!(!editor.affixes[0].is_dirty());
    editor.remove_affix(0);

    // The freed suffix takes another addition; both wait for one Apply, removal first.
    assert!(editor.open_add());
    editor.picker_search("armour");
    editor.picker_select(0);
    editor.use_picker();
    let added = editor.affixes[4].state.id;
    assert_eq!(editor.apply_text(), "Apply 2 drafts");
    editor.apply();
    assert_eq!(
        editor.message(),
        ("2 drafts applied. Edit readback confirmed for each: Health, Increased Armour.", MessageKind::Confirmed)
    );
    assert!(matches!(
        state.borrow().writes.as_slice(),
        [ItemEdit::RemoveAffix(RemoveAffixEdit { slot: 0, id: 25 }), ItemEdit::AddAffix(AddAffixEdit { tier: 5, .. })]
    ));
    assert!(editor.drafts().is_empty());
    assert!(editor.affixes.iter().all(|row| !row.added));
    assert!(editor.affixes[row(&editor, added)].confirmed);
    assert_eq!(editor.selected_affix(), Some(row(&editor, added)));
    assert_eq!(editor.history.iter().map(|entry| entry.summary.as_str()).collect::<Vec<_>>(), ["Added · Tier 5", "Removed"]);

    // One Undo apply restores the item as it was.
    editor.undo();
    assert_eq!(editor.message(), ("Previous values restored; readback confirmed.", MessageKind::Confirmed));
    let mut restored: Vec<u16> = state.borrow().item.as_ref().unwrap().affixes.iter().map(|affix| affix.id).collect();
    let mut before = before;
    restored.sort();
    before.sort();
    assert_eq!(restored, before);
}

#[test]
fn the_picker_replaces_as_a_draft() {
    let game = FixtureGame::boots(false);
    let state = game.state.clone();
    let mut editor = ForgeEditor::new(Box::new(game));
    assert!(editor.open_replace(3));
    editor.picker_search("res");
    editor.picker_allow(true);
    assert_eq!(
        editor.picker_list().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["Cold Resistance", "Fire Resistance", "Sentinel's Resolve Reforged"]
    );
    editor.picker_select(0);
    assert_eq!(editor.picker_status(), "Compatible with this slot");
    assert_eq!(editor.picker_subtitle(), "Currently Fire Resistance");
    editor.use_picker();
    assert_eq!(editor.drafts()[0].summary, "Fire Resistance → Cold Resistance");
    editor.discard_all();
    assert_eq!(editor.message().0, "Draft discarded.");
    assert!(state.borrow().writes.is_empty());
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
        undoing: bool,
    }
    struct Delayed {
        game: FixtureGame,
        control: Rc<RefCell<Control>>,
    }
    impl ForgeGame for Delayed {
        fn begin_batch(&mut self) -> Result<(), GameError> {
            self.game.begin_batch()
        }
        fn undo_depth(&self) -> usize {
            self.game.undo_depth()
        }
        fn undo_needs_override(&self) -> bool {
            self.game.undo_needs_override()
        }
        fn undo_intent(&self) -> Option<ItemEdit> {
            self.game.undo_intent()
        }
        fn undo(&mut self, expected: &ForgeItem) -> EditOutcome {
            let Some(edit) = self.undo_intent() else {
                return refused("Undo unavailable.");
            };
            let result = self.write(expected, &edit);
            self.control.borrow_mut().undoing = true;
            result
        }
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
        fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> crate::forge::game::Lookup<ValueTable> {
            self.game.resolve(item, request)
        }
        fn preview_unique(&self, item: &ForgeItem, edit: &ItemEdit) -> crate::forge::game::Lookup<Vec<ModifierState>> {
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
            let undoing = std::mem::take(&mut c.undoing);
            Some(match result {
                ResultKind::Confirmed => {
                    if undoing {
                        self.game.undo(&expected)
                    } else {
                        self.game.write(&expected, &edit)
                    }
                }
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
            undoing: false,
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
    fn a_pending_identity_write_does_not_allow_another_identity_pick() {
        let control = Rc::new(RefCell::new(Control {
            released: false,
            busy: false,
            outcomes: VecDeque::new(),
            pending: None,
            undoing: false,
        }));
        let mut editor = ForgeEditor::new(Box::new(Delayed {
            game: FixtureGame::unique_ring(),
            control: control.clone(),
        }));
        editor.pick_identity(2);
        let name = editor.item_name();
        editor.apply();
        assert!(control.borrow().pending.is_some());
        editor.pick_identity(3);
        assert_eq!(editor.item_name(), name);
        assert!(!editor.can_change_identity());
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
        // A part-applied batch names the step the game did not apply.
        assert_eq!(
            e.message(),
            (
                "1 of 2 drafts applied and confirmed; Corruption was not applied. Refused before any change.",
                MessageKind::Error
            )
        );
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
            assert_eq!(e.history.len(), 1);
            assert_eq!(e.history[0].outcome, "Confirmed");
            assert!(!e.can_undo());
            assert_eq!(state.borrow().writes.len(), 1);
            assert_eq!(e.has_item(), matches!(result, ResultKind::NotKept));
            let said = if matches!(result, ResultKind::NotKept) {
                "did not keep this edit"
            } else {
                "could not be confirmed"
            };
            assert!(e.message().0.contains(said));
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
        assert!(e.message().0.contains("game confirmed the edit"));
        assert!(!e.message().0.contains("could not be confirmed"));
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
        state.borrow_mut().item.as_mut().unwrap().unique = Some(forge::item::UniqueState {
            id: 70,
            rolls: vec![100, 7],
            modifiers: vec![],
            formatting: crate::forge::game::Lookup::Ready(()),
        });
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

#[test]
fn pending_unique_preview_preserves_drafts_refreshes_only_current_text_and_clears_on_discard() {
    use super::{
        game::{Catalog, EditOutcome, ForgeGame, PREVIEW_LOADING},
        item::{ForgeItem, ItemEdit, ModifierState, RollRequest, ValueTable},
    };
    use std::{cell::Cell as State, rc::Rc};
    struct Delayed {
        inner: FixtureGame,
        ready: Rc<State<bool>>,
        pending: Rc<State<bool>>,
        failed: Rc<State<bool>>,
    }
    impl ForgeGame for Delayed {
        fn catalog(&self) -> &Catalog {
            self.inner.catalog()
        }
        fn read(&self) -> Option<ForgeItem> {
            self.inner.read()
        }
        fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> crate::forge::game::Lookup<ValueTable> {
            self.inner.resolve(item, request)
        }
        fn write(&mut self, item: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
            self.inner.write(item, edit)
        }
        fn clear_unique_preview(&self) {
            self.pending.set(false);
        }
        fn preview_unique(&self, item: &ForgeItem, edit: &ItemEdit) -> crate::forge::game::Lookup<Vec<ModifierState>> {
            self.pending.set(true);
            if self.failed.get() {
                return crate::forge::game::Lookup::Unavailable(PREVIEW_LOADING.into());
            }
            if !self.ready.get() {
                return crate::forge::game::Lookup::Loading;
            }
            self.inner.preview_unique(item, edit)
        }
    }
    let ready = Rc::new(State::new(false));
    let pending = Rc::new(State::new(false));
    let failed = Rc::new(State::new(false));
    let mut editor = ForgeEditor::new(Box::new(Delayed {
        inner: FixtureGame::unique(),
        ready: ready.clone(),
        pending: pending.clone(),
        failed: failed.clone(),
    }));
    let cell = Cell::Unique(0, 0);
    assert!(editor.begin_input(cell));
    editor.set_input(cell, "24");
    assert!(editor.commit_input(cell));
    assert!(pending.get());
    assert_eq!(editor.preview_status, PREVIEW_LOADING);
    assert!(!editor.preview_failed);
    assert_eq!(editor.drafts().len(), 1);
    editor.refresh();
    assert_eq!(editor.preview_status, PREVIEW_LOADING);
    assert_eq!(editor.drafts()[0].summary, "Value 18% → 24%");
    assert!(editor.begin_input(cell));
    editor.set_input(cell, "25");
    assert!(editor.commit_input(cell));
    ready.set(true);
    editor.refresh();
    assert_eq!(editor.preview_status, "Draft preview");
    assert_eq!(editor.uniques[0].text, "+25% Movement Speed");
    assert_eq!(editor.uniques[2].text, "Your minions gain 25% Movement Speed");
    failed.set(true);
    editor.refresh();
    assert!(editor.preview_failed, "an unavailable reason equal to loading copy is still a terminal failure");
    assert_eq!(editor.preview_status, format!("Preview unavailable: {PREVIEW_LOADING}"));
    editor.discard_all();
    assert!(!pending.get());
    assert_eq!(editor.drafts().len(), 0);
    assert_eq!(editor.preview_status, "");
}

#[test]
fn keeping_legendary_potential_an_identity_never_holds_needs_the_override_and_warns() {
    let game = FixtureGame::unique_ring();
    game.change(|item| item.status.as_mut().unwrap().legendary_potential = 2);
    let mut editor = ForgeEditor::new(Box::new(game));
    let option = |editor: &ForgeEditor, name: &str| {
        editor
            .identity_choices()
            .iter()
            .find(|option| option.name == name)
            .map(|option| (option.detail.clone(), option.available))
            .unwrap()
    };
    assert_eq!(option(&editor, "Fixture Ember Unique"), ("Unique · Gold Ring · legendary potential".into(), true));
    assert_eq!(
        option(&editor, "Fixture Woven Unique"),
        (
            "Unique · Gold Ring · Weaver's Will · keeps legendary potential it never holds, which may crash the game and reset the item state; needs \"I know what I'm doing\""
                .into(),
            false
        )
    );
    let (set, available) = option(&editor, "Fixture Set Ring");
    assert!(set.contains("keeps legendary potential it never holds"), "{set}");
    assert!(!available);
    editor.pick_identity(3);
    assert!(!editor.is_identity_drafted(), "an entry that needs the override can't be picked without it");
}

#[test]
fn the_popover_knows_which_cell_types_and_a_rebuilt_row_ends_the_drag_it_began() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::boots(false)));
    assert_eq!(editor.typing_cell(), None);
    assert!(editor.begin_input(Cell::Affix(1)));
    assert_eq!(editor.typing_cell(), Some(Cell::Affix(1)));
    editor.cancel_input(Cell::Affix(1));
    assert_eq!(editor.typing_cell(), None);
    // A drag on row 0 whose release arrives for another place (the row moved or was rebuilt under the pointer) still ends
    // the drag it began, as one Undo step.
    editor.press(Cell::Affix(0));
    editor.scrub(Cell::Affix(0), 0.1);
    editor.end_scrub(Cell::Affix(2), false);
    assert!(!editor.affixes[0].value.dragging);
    assert_eq!(editor.drafts().len(), 1);
}

#[test]
fn typed_value_cannot_commit_after_identity_draft_locks_the_row() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::unique_ring()));
    // Integer status fields have mappings even though unique roll mapping is unsupported.
    assert!(editor.begin_input(Cell::Field(0)));
    editor.set_input(Cell::Field(0), "60");
    editor.pick_identity(2);
    assert!(!editor.can_edit_value(Cell::Field(0)));
    assert!(editor.commit_input(Cell::Field(0)));
    assert_eq!(editor.drafts().len(), 1);
    assert!(!editor.fields[0].is_dirty());
}

#[test]
fn loading_a_mapping_during_a_drag_still_closes_the_row_undo_group() {
    let mut editor = ForgeEditor::new(Box::new(FixtureGame::boots(false)));
    let original = editor.affixes[0].roll();
    editor.press(Cell::Affix(0));
    editor.scrub(Cell::Affix(0), 0.1);
    editor.affixes[0].value.set_mapping(super::game::Lookup::Loading, true);
    editor.end_scrub(Cell::Affix(0), false);
    assert!(!editor.affixes[0].value.dragging);
    editor.undo();
    assert_eq!(editor.affixes[0].roll(), original);
}
