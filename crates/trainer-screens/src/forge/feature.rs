//! The Item Forge as a feature. The editor holds every rule; this says which of its methods each intent runs, and what the
//! worker's snapshots do to it. What the editor asks of the worker leaves as effects, and what comes back returns as
//! intents, so the reducer never touches the worker.

use super::editor::{Cell, ForgeEditor, RowRef};
use super::game::{Catalog, ForgeGame, Request};
use super::item::Seal;
use super::picker::KindFilter;
use super::project::{project_catalog, project_item};
use super::rows::FieldKind;
use eg_trainer::backend::{BackendError, BackendSnapshot, ForgeSnapshot, ForgeState, Phase};
use eg_trainer::definitions::catalog::GameCatalog;
use eg_trainer::item::template::ItemTemplate;
use phrases::phrases;
use spiderland::{Effects, Error, Feature, Pulse, Tracked};
use std::sync::Arc;

phrases! {
    SeeFaultLog: "{error} Details are in the fault log."
    UnexpectedFault: "something went wrong unexpectedly. Details are in the fault log."
}

/// The forge as a feature: see the module's documentation.
pub struct Forge;

/// What can be given the keyboard's focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Cell(Cell),
    /// A field of the header: the base or the identity, by its part.
    Header,
}

/// Where the keyboard's focus goes: `target`, as its `part`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub target: FocusTarget,
    pub part: i32,
}

/// Named for the fault trail (`eg_core::fault::TRAIL`).
#[derive(strum::IntoStaticStr)]
pub enum Intent {
    /// What the worker has published, and whether it still runs.
    Observed {
        snapshot: Arc<BackendSnapshot>,
        running: bool,
    },
    /// Whether the Forge page is the one showing.
    PageChanged {
        forge: bool,
    },
    WriteAnswered(Result<(), BackendError>),
    /// The worker took neither the write nor, with `batch`, the batch it opens.
    WriteRefused {
        batch: bool,
    },
    MappingAnswered {
        id: u64,
        error: Option<String>,
    },
    MappingRefused {
        id: u64,
    },
    PreviewDue,
    PreviewAnswered {
        error: Option<String>,
    },
    PreviewRefused,
    DescriptionsRefused,

    SelectAffix(Option<usize>),
    SelectUnique(Option<usize>),
    Press(Cell),
    Scrub(Cell, f64),
    EndScrub(Cell, bool),
    Step(Cell, i32),
    StepToEnd(Cell, bool),
    BeginInput(Cell),
    InputEdited(Cell, String),
    CommitInput(Cell),
    CancelInput(Cell),
    FinishInput(Cell),
    PickTier(usize, u8),
    PickSeal(usize, Seal),
    ToggleFlag(usize, bool),
    PickRarity(bool),
    PickBase(usize),
    PickIdentity(usize),
    IdentitySearch(String),
    BaseAllow(bool),
    PickerSearch(String),
    PickerFilter(KindFilter),
    PickerAllow(bool),
    PickerCategory(usize),
    PickerClass(usize),
    PickerSlot(usize),
    PickerLevel(usize),
    PickerSource(usize),
    PickerTag(usize, bool),
    PickerRemoveChip(usize),
    PickerClearFilters,
    PickerShowUniques,
    PickerSelect(usize),
    UsePicker,
    CancelPicker,
    OpenReplace(usize),
    OpenAdd,
    DiscardRow(usize),
    RemoveAffix(usize),
    DiscardEntry(usize),
    DiscardAll,
    MaxTiers,
    MaxRolls,
    /// Read the kept templates, so the flyout opens with them.
    ReadTemplates,
    TemplatesRead(Result<Vec<ItemTemplate>, Error>),
    TemplatesOpened,
    TemplateName(String),
    TemplateSave(String, bool),
    TemplateSaved {
        name: String,
        replaced: Option<String>,
        result: Result<Result<(), String>, Error>,
    },
    TemplateApply(usize),
    TemplateDelete(usize),
    TemplateDeleted {
        name: String,
        result: Result<Result<(), String>, Error>,
    },
    Apply,
    Undo,
    UndoDraft,
    /// Up and Down from a cell of an affix row.
    MoveRow {
        row: usize,
        delta: i32,
        part: i32,
    },
    /// Up and Down from a roll of a unique modifier.
    MoveUnique {
        row: usize,
        delta: i32,
        roll: usize,
    },
    /// Up and Down from a roll of an implicit.
    MoveImplicit {
        row: usize,
        delta: i32,
        roll: usize,
    },
    ShowDraft(usize),
    /// Whether the report of the item read at `generation` went into a fault log.
    UnshownReported {
        generation: u64,
        written: bool,
    },
}

pub enum Effect {
    Worker(Request),
    /// Look at what the worker has now, answered with [`Intent::Observed`]: it may have published before the forge began
    /// following it.
    Observe,
    /// Read the kept templates from their folder.
    ReadTemplates,
    /// Keep `template`, in place of the kept one named `replaced` if any.
    SaveTemplate {
        template: ItemTemplate,
        replaced: Option<String>,
    },
    DeleteTemplate(String),
    /// Record an item the forge cannot show, with the item as the game reported it, off the UI thread, answered with
    /// [`Intent::UnshownReported`].
    ReportUnshown {
        generation: u64,
        snapshot: Arc<ForgeSnapshot>,
        error: String,
    },
}

pub struct State {
    editor: ForgeEditor,
    catalog: (Option<Arc<GameCatalog>>, Arc<Catalog>),
    revision: Option<u64>,
    /// The forge is rebuilt only while its page shows.
    on_page: bool,
    /// A read arrived while the Forge page was not showing: the forge is rebuilt from the latest one when it shows again.
    stale: bool,
    /// Faults are recorded in this run, so the page can say where the details are.
    has_fault_log: bool,
    /// The last item the forge could not show and that went to the fault log, so each is recorded once.
    reported: Option<(u64, Arc<ForgeSnapshot>)>,
    /// Connected, but the item could not be shown: why.
    item_unavailable: Tracked<String>,
    /// What the last intent that answers the view said: whether a cell opened for typing, or its text was taken.
    answer: bool,
    /// The cell a typing popover last opened on, moving each time one opens.
    opened_input: Tracked<Option<Cell>>,
    /// Where the keyboard's focus was last sent; the same cell can be asked for again.
    focus: Pulse<Focus>,
}

impl State {
    pub fn new(snapshot: BackendSnapshot, running: bool, on_page: bool, has_fault_log: bool) -> State {
        State {
            editor: ForgeEditor::new(ForgeGame::new(snapshot, running)),
            catalog: (None, Arc::new(Catalog::default())),
            revision: None,
            on_page,
            stale: false,
            has_fault_log,
            reported: None,
            item_unavailable: Tracked::default(),
            answer: false,
            opened_input: Tracked::default(),
            focus: Pulse::new(Focus {
                target: FocusTarget::Header,
                part: 0,
            }),
        }
    }

    pub fn editor(&self) -> &ForgeEditor {
        &self.editor
    }

    /// The editor directly, to set up the first state before a store runs it.
    pub fn editor_mut(&mut self) -> &mut ForgeEditor {
        &mut self.editor
    }

    pub fn opened_input(&self) -> &Tracked<Option<Cell>> {
        &self.opened_input
    }

    pub fn focus(&self) -> &Pulse<Focus> {
        &self.focus
    }

    pub fn item_unavailable(&self) -> &Tracked<String> {
        &self.item_unavailable
    }

    pub fn answer(&self) -> bool {
        self.answer
    }
}

/// The next line in `delta`'s direction from `row` that has rolls to move to, and the roll in it nearest `roll`.
fn next_with_rolls(from: usize, delta: i32, count: usize, rolls_of: impl Fn(usize) -> Option<usize>, roll: usize) -> Option<(usize, usize)> {
    let step = delta.signum() as isize;
    let mut next = from as isize + step;
    while step != 0 && next >= 0 && (next as usize) < count {
        if let Some(rolls) = rolls_of(next as usize).filter(|rolls| *rolls > 0) {
            return Some((next as usize, roll.min(rolls - 1)));
        }
        next += step;
    }
    None
}

fn read_templates(editor: &ForgeEditor, effects: &mut Effects<Effect>) {
    if editor.keeps_templates {
        effects.push(Effect::ReadTemplates);
    }
}

/// Why work on a template's file failed, if it did.
fn failure_of(result: Result<Result<(), String>, Error>) -> Option<String> {
    match result {
        Ok(result) => result.err(),
        Err(_) => Some(UnexpectedFault.into()),
    }
}

/// Takes what came, and settles what it means for the editor: a new read is shown only while the page shows, and a write
/// on its way settles wherever the user is.
fn settle(state: &mut State, completed: bool, effects: &mut Effects<Effect>) {
    let game_ask = |state: &State| {
        let game = state.editor.game();
        game.ask_descriptions();
        game.send_preview();
    };
    game_ask(state);
    let (phase, revision, running) = {
        let game = state.editor.game();
        (game.snapshot().phase, game.snapshot().revision, game.is_running())
    };
    if !running && phase != Phase::Stopped {
        // An unexpected worker failure must not leave apparently live data.
        state.stale = false;
        state.editor.set_catalog(Arc::new(Catalog::default()));
        return;
    }
    if state.revision == Some(revision) {
        if completed && state.on_page {
            state.editor.refresh();
        } else {
            if completed {
                state.stale = true;
            }
            // Only the editor's receipts can have moved on.
            state.editor.poll_writes();
        }
        return;
    }
    state.revision = Some(revision);
    let (data, error) = {
        let snapshot = state.editor.game().snapshot();
        (snapshot.catalog.data().cloned(), project_item(snapshot).err())
    };
    if let Some(data) = data
        && state.catalog.0.as_ref().is_none_or(|old| !Arc::ptr_eq(old, &data))
    {
        state.catalog.1 = Arc::new(project_catalog(&data));
        state.catalog.0 = Some(data);
    }
    let unavailable = error.map(|error| report_unshown(state, error, effects)).unwrap_or_default();
    if state.on_page {
        state.stale = false;
        state.editor.set_catalog(state.catalog.1.clone());
    } else {
        // Rebuilding the forge for every read while another page shows is wasted work: it is rebuilt once when its page
        // shows. A write in flight still settles.
        state.stale = true;
        state.editor.poll_writes();
    }
    state.item_unavailable.set(unavailable);
}

/// Records an item the forge cannot show, once, so the refused value can be traced. Returns what the forge says in its place.
fn report_unshown(state: &mut State, error: String, effects: &mut Effects<Effect>) -> String {
    if !state.has_fault_log {
        return error;
    }
    let ForgeState::Item { generation, snapshot, .. } = &state.editor.game().snapshot().forge else {
        return error;
    };
    let (generation, snapshot) = (*generation, snapshot.clone());
    if state.reported.as_ref().is_none_or(|(g, s)| *g != generation || *s != snapshot) {
        state.reported = Some((generation, snapshot.clone()));
        effects.push(Effect::ReportUnshown {
            generation,
            snapshot,
            error: error.clone(),
        });
    }
    SeeFaultLog.with(&[("error", &error)])
}

/// What runs while the state wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscription {
    /// Following the worker's snapshots, as [`Intent::Observed`]. It lasts as long as the forge, which judges its drafts and
    /// receipts on the worker's latest state whether or not it shows.
    Worker,
}

impl Feature for Forge {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = spiderland::Never;
    type Subscription = Subscription;

    fn subscriptions(_: &State) -> impl IntoIterator<Item = Subscription> {
        [Subscription::Worker]
    }

    fn start(state: &mut State, effects: &mut Effects<Effect>) {
        effects.push(Effect::Observe);
        read_templates(&state.editor, effects);
    }

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        let editor = &mut state.editor;
        match intent {
            Intent::Observed { snapshot, running } => {
                editor.observe(snapshot, running);
                settle(state, false, effects);
            }
            Intent::PageChanged { forge } => {
                state.on_page = forge;
                if forge && std::mem::take(&mut state.stale) {
                    state.editor.set_catalog(state.catalog.1.clone());
                }
            }
            Intent::WriteAnswered(result) => {
                editor.game().write_answered(result);
                editor.poll_writes();
                editor.game().send_preview();
            }
            Intent::WriteRefused { batch } => {
                editor.game().write_refused(batch);
                editor.poll_writes();
            }
            Intent::MappingAnswered { id, error } => {
                let wanted = editor.game().mapping_answered(id, error);
                settle(state, wanted, effects);
            }
            Intent::MappingRefused { id } => {
                let wanted = editor.game().mapping_refused(id);
                settle(state, wanted, effects);
            }
            Intent::PreviewDue => editor.game().preview_due(),
            Intent::PreviewAnswered { error } => {
                editor.game().preview_answered(error);
                settle(state, true, effects);
            }
            Intent::PreviewRefused => {
                editor.game().preview_refused();
                settle(state, true, effects);
            }
            Intent::DescriptionsRefused => editor.game().descriptions_refused(),

            Intent::SelectAffix(index) => editor.select_affix(index),
            Intent::SelectUnique(index) => editor.select_unique(index),
            Intent::Press(cell) => editor.press(cell),
            Intent::Scrub(cell, fraction) => {
                editor.scrub(cell, fraction);
            }
            Intent::EndScrub(cell, cancelled) => editor.end_scrub(cell, cancelled),
            Intent::Step(cell, steps) => {
                editor.step(cell, steps);
            }
            Intent::StepToEnd(cell, maximum) => {
                editor.step_to_end(cell, maximum);
            }
            Intent::BeginInput(cell) => {
                state.answer = editor.begin_input(cell);
                if state.answer {
                    *state.opened_input = Some(cell);
                }
            }
            Intent::InputEdited(cell, text) => editor.set_input(cell, &text),
            Intent::CommitInput(cell) => state.answer = editor.commit_input(cell),
            Intent::CancelInput(cell) => editor.cancel_input(cell),
            Intent::FinishInput(cell) => editor.finish_input(cell),
            Intent::PickTier(row, tier) => editor.pick_tier(row, tier),
            Intent::PickSeal(row, seal) => editor.pick_seal(row, seal),
            Intent::ToggleFlag(index, on) => editor.toggle_flag(index, on),
            Intent::PickRarity(exalted) => editor.pick_rarity(exalted),
            Intent::PickBase(option) => editor.pick_base(option),
            Intent::PickIdentity(choice) => editor.pick_identity(choice),
            Intent::IdentitySearch(text) => editor.set_identity_search(&text),
            Intent::BaseAllow(allow) => editor.set_base_allow_incompatible(allow),
            Intent::PickerSearch(text) => editor.picker_search(&text),
            Intent::PickerFilter(filter) => editor.picker_filter(filter),
            Intent::PickerAllow(allow) => editor.picker_allow(allow),
            Intent::PickerCategory(choice) => editor.picker_category(choice),
            Intent::PickerClass(choice) => editor.picker_class(choice),
            Intent::PickerSlot(choice) => editor.picker_slot(choice),
            Intent::PickerLevel(choice) => editor.picker_level(choice),
            Intent::PickerSource(choice) => editor.picker_source(choice),
            Intent::PickerTag(choice, on) => editor.picker_tag(choice, on),
            Intent::PickerRemoveChip(chip) => editor.picker_remove_chip(chip),
            Intent::PickerClearFilters => editor.picker_clear_filters(),
            Intent::PickerShowUniques => editor.picker_show_uniques(),
            Intent::PickerSelect(index) => editor.picker_select(index),
            Intent::UsePicker => editor.use_picker(),
            Intent::CancelPicker => editor.cancel_picker(),
            Intent::OpenReplace(row) => state.answer = editor.open_replace(row),
            Intent::OpenAdd => state.answer = editor.open_add(),
            Intent::DiscardRow(row) => editor.discard_row(row),
            Intent::RemoveAffix(row) => editor.remove_affix(row),
            Intent::DiscardEntry(entry) => editor.discard_entry(entry),
            Intent::DiscardAll => editor.discard_all(),
            Intent::MaxTiers => editor.max_tiers(),
            Intent::MaxRolls => editor.max_rolls(),
            // Templates are read from their folder at the start, when the flyout opens and after each change.
            Intent::ReadTemplates => read_templates(editor, effects),
            Intent::TemplatesRead(found) => {
                // After a panic, which is in the fault log, what was read before stays.
                if let Ok(found) = found {
                    *editor.templates = found;
                }
            }
            Intent::TemplatesOpened => {
                editor.template_message.clear();
                editor.template_name.clear();
                read_templates(editor, effects);
            }
            Intent::TemplateName(name) => {
                editor.template_name = name;
            }
            Intent::TemplateSave(name, whole) => match editor.template_to_save(&name, whole) {
                Ok((template, replaced)) => effects.push(Effect::SaveTemplate { template, replaced }),
                Err(problem) => editor.template_message = problem,
            },
            Intent::TemplateSaved { name, replaced, result } => {
                editor.template_saved(&name, replaced, failure_of(result));
                read_templates(editor, effects);
            }
            Intent::TemplateApply(index) => editor.apply_template(index),
            Intent::TemplateDelete(index) => {
                if let Some(template) = editor.templates.get(index) {
                    effects.push(Effect::DeleteTemplate(template.name.clone()));
                }
            }
            Intent::TemplateDeleted { name, result } => {
                editor.template_deleted(&name, failure_of(result));
                read_templates(editor, effects);
            }
            Intent::Apply => editor.apply(),
            Intent::Undo => editor.undo(),
            Intent::UndoDraft => editor.undo_draft_only(),
            // Up and Down from a cell move the selection; the same cell of the next row takes focus.
            Intent::MoveRow { row, delta, part } => {
                let count = editor.affixes.len() as i32;
                let next = (row as i32 + delta).clamp(0, (count - 1).max(0)) as usize;
                editor.select_affix(Some(next));
                state.focus.fire(Focus {
                    target: FocusTarget::Cell(Cell::Affix(next)),
                    part,
                });
            }
            // The nearest line in that direction with rolls to scrub takes the selection and the focus; past the ends
            // nothing moves.
            Intent::MoveUnique { row, delta, roll } => {
                let target = next_with_rolls(
                    row,
                    delta,
                    editor.uniques.len(),
                    |line| editor.uniques[line].is_editable().then(|| editor.uniques[line].rolls.len()),
                    roll,
                );
                if let Some((line, roll)) = target {
                    editor.select_unique(Some(line));
                    state.focus.fire(Focus {
                        target: FocusTarget::Cell(Cell::Unique(line, roll)),
                        part: 0,
                    });
                }
            }
            Intent::MoveImplicit { row, delta, roll } => {
                if let Some((line, roll)) = next_with_rolls(row, delta, editor.implicits.len(), |line| Some(editor.implicits[line].rolls.len()), roll) {
                    state.focus.fire(Focus {
                        target: FocusTarget::Cell(Cell::Implicit(line, roll)),
                        part: 0,
                    });
                }
            }
            // A ledger entry selects its affix or unique row, or brings the cell that edits it into focus: a value's scrubber,
            // a flag's toggle, or the base or the identity in the header.
            Intent::ShowDraft(entry) => {
                let focus = editor.show_draft(entry).and_then(|row| match row {
                    RowRef::Implicit(uid) => editor
                        .implicits
                        .iter()
                        .position(|line| line.uid == uid)
                        .map(|line| (FocusTarget::Cell(Cell::Implicit(line, 0)), 0)),
                    RowRef::Field(uid) if editor.base.as_ref().is_some_and(|base| base.uid == uid) => Some((FocusTarget::Header, 4)),
                    RowRef::Field(uid) if editor.identity.as_ref().is_some_and(|identity| identity.uid == uid) => Some((FocusTarget::Header, 5)),
                    RowRef::Field(uid) => editor
                        .fields
                        .iter()
                        .position(|field| field.uid == uid)
                        .map(|index| (FocusTarget::Cell(Cell::Field(index)), i32::from(editor.fields[index].kind != FieldKind::Number))),
                    _ => None,
                });
                if let Some((target, part)) = focus {
                    state.focus.fire(Focus { target, part });
                }
            }
            // A report no folder took is made again with the next read of the item, as if it had never been made.
            Intent::UnshownReported { generation, written } => {
                if !written && state.reported.as_ref().is_some_and(|(reported, _)| *reported == generation) {
                    state.reported = None;
                }
            }
        }
        for request in state.editor.game().take_requests() {
            effects.push(Effect::Worker(request));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::snapshot;
    use spiderland::step;

    fn state(on_page: bool) -> State {
        State::new(snapshot(0, Phase::Ready), true, on_page, false)
    }

    fn observed(revision: u64, phase: Phase, running: bool) -> Intent {
        Intent::Observed {
            snapshot: Arc::new(snapshot(revision, phase)),
            running,
        }
    }

    #[test]
    fn a_read_while_another_page_shows_is_rebuilt_once_when_the_page_shows() {
        let mut state = state(false);
        step::<Forge>(&mut state, observed(1, Phase::Ready, true));
        let taken = state.editor().game_generation();
        step::<Forge>(&mut state, Intent::PageChanged { forge: true });
        assert_ne!(state.editor().game_generation(), taken, "the page's return builds from the latest read");
        let built = state.editor().game_generation();
        step::<Forge>(&mut state, Intent::PageChanged { forge: false });
        step::<Forge>(&mut state, Intent::PageChanged { forge: true });
        assert_eq!(state.editor().game_generation(), built, "with no new read there is nothing to build");
    }

    #[test]
    fn a_read_while_the_page_shows_is_built_at_once() {
        let mut state = state(true);
        step::<Forge>(&mut state, observed(1, Phase::Ready, true));
        let built = state.editor().game_generation();
        step::<Forge>(&mut state, Intent::PageChanged { forge: true });
        assert_eq!(state.editor().game_generation(), built);
    }

    #[test]
    fn a_worker_lost_without_stopping_leaves_no_live_data() {
        let mut state = state(false);
        let before = state.editor().game_generation();
        step::<Forge>(&mut state, observed(1, Phase::Ready, false));
        assert!(state.editor().game_generation() > before + 1, "the catalog goes, wherever the user is");
        assert!(!state.editor().has_item());
        assert!(!state.editor().can_mutate());
    }

    #[test]
    fn a_refused_write_with_nothing_waiting_asks_nothing_more() {
        let mut state = state(true);
        assert!(step::<Forge>(&mut state, Intent::WriteRefused { batch: true }).effects.is_empty());
        assert!(step::<Forge>(&mut state, Intent::WriteAnswered(Ok(()))).effects.is_empty());
    }

    #[test]
    fn up_and_down_take_the_nearest_line_with_rolls_and_stop_at_the_ends() {
        let rolls = |line: usize| [Some(2), None, Some(0), Some(4)][line];
        assert_eq!(next_with_rolls(0, 1, 4, rolls, 3), Some((3, 3)), "past one with none and one that's fixed");
        assert_eq!(next_with_rolls(3, -1, 4, rolls, 3), Some((0, 1)), "the same roll, or the last there is");
        assert_eq!(next_with_rolls(3, 1, 4, rolls, 0), None);
        assert_eq!(next_with_rolls(0, -1, 4, rolls, 0), None);
        assert_eq!(next_with_rolls(1, 0, 4, rolls, 0), None);
    }

    fn template(name: &str) -> ItemTemplate {
        ItemTemplate {
            name: name.into(),
            affixes: Vec::new(),
            item: None,
        }
    }

    fn keeping() -> State {
        let mut state = state(true);
        state.editor_mut().keeps_templates = true;
        state
    }

    #[test]
    fn the_forge_starts_by_looking_at_the_worker_and_reading_its_templates() {
        assert!(matches!(spiderland::start::<Forge>(&mut state(true))[..], [Effect::Observe]));
        assert!(matches!(spiderland::start::<Forge>(&mut keeping())[..], [Effect::Observe, Effect::ReadTemplates]));
    }

    #[test]
    fn a_run_without_a_template_folder_never_asks_for_one() {
        let mut state = state(true);
        assert!(step::<Forge>(&mut state, Intent::ReadTemplates).effects.is_empty());
        assert!(step::<Forge>(&mut state, Intent::TemplatesOpened).effects.is_empty());
        assert!(step::<Forge>(&mut state, Intent::TemplateSave("Boots".into(), false)).effects.is_empty());
        assert_eq!(state.editor().template_message, "Templates are not kept in this run.");
    }

    #[test]
    fn templates_are_read_when_the_flyout_opens_and_after_each_change() {
        let mut state = keeping();
        assert!(matches!(step::<Forge>(&mut state, Intent::TemplatesOpened).effects.as_slice(), [Effect::ReadTemplates]));
        assert!(step::<Forge>(&mut state, Intent::TemplatesRead(Ok(vec![template("Boots")]))).effects.is_empty());
        assert_eq!(state.editor().templates.len(), 1);
        // Typing reads nothing: what a name would replace comes from what was read.
        assert!(step::<Forge>(&mut state, Intent::TemplateName("BOOTS".into())).effects.is_empty());
        assert_eq!(state.editor().template_replaces().as_deref(), Some("Boots"));
        let saved = Intent::TemplateSaved {
            name: "BOOTS".into(),
            replaced: Some("Boots".into()),
            result: Ok(Ok(())),
        };
        assert!(matches!(step::<Forge>(&mut state, saved).effects.as_slice(), [Effect::ReadTemplates]));
        assert_eq!(state.editor().template_message, "Saved \"BOOTS\" in place of \"Boots\".");
        assert_eq!(state.editor().template_name, "", "a saved name is done with");
        let effects = step::<Forge>(&mut state, Intent::TemplateDelete(0)).effects;
        assert!(matches!(effects.as_slice(), [Effect::DeleteTemplate(name)] if name == "Boots"));
        assert!(step::<Forge>(&mut state, Intent::TemplateDelete(5)).effects.is_empty(), "no such row");
    }

    #[test]
    fn a_failed_save_keeps_what_was_typed_and_says_why() {
        let mut state = keeping();
        step::<Forge>(&mut state, Intent::TemplateName("Boots".into()));
        let failed = |result| Intent::TemplateSaved {
            name: "Boots".into(),
            replaced: None,
            result,
        };
        step::<Forge>(&mut state, failed(Ok(Err("disk full".into()))));
        assert_eq!(state.editor().template_message, "The template could not be saved: disk full");
        assert_eq!(state.editor().template_name, "Boots");
        step::<Forge>(&mut state, failed(Err(Error::Panicked)));
        assert!(state.editor().template_message.contains("fault log"));
        assert_eq!(state.editor().template_name, "Boots");
    }

    #[test]
    fn a_read_that_panics_leaves_what_was_read_before() {
        let mut state = keeping();
        step::<Forge>(&mut state, Intent::TemplatesRead(Ok(vec![template("Boots")])));
        step::<Forge>(&mut state, Intent::TemplatesRead(Err(Error::Panicked)));
        assert_eq!(state.editor().templates.len(), 1);
    }

    #[test]
    fn keys_on_a_cell_move_the_selection_and_ask_for_focus() {
        let mut state = state(true);
        let asked = state.focus().serial();
        step::<Forge>(&mut state, Intent::MoveRow { row: 0, delta: 1, part: 2 });
        let focus = Focus {
            target: FocusTarget::Cell(Cell::Affix(0)),
            part: 2,
        };
        assert_eq!(*state.focus().value(), focus);
        assert_ne!(state.focus().serial(), asked);
        let asked = state.focus().serial();
        step::<Forge>(&mut state, Intent::MoveRow { row: 0, delta: 1, part: 2 });
        assert_ne!(state.focus().serial(), asked, "the same cell again still moves focus");
    }
}
