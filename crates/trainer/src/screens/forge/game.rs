//! The game's data as the forge shows it (`Catalog` and its definitions), the shapes of the game's answers (a lookup that may
//! still be on its way, `Lookup`, and a typed write outcome, `EditOutcome`), and the game as the editor knows it
//! (`ForgeGame`).

use super::item::{self, ItemEdit, ModifierSnapshot, RollRequest, ValueTable};
use super::project::{project_item, value_table};
use crate::backend::{BackendError, BackendSnapshot, Command, ForgeState as BackendForgeState, Phase};
use crate::item::edit::{ExpectedObservation, RollEdit, RollTarget};
use crate::item::values::{MappingAddress, MappingRequest, MappingTarget};
use crate::write::ForgePlan;
use phrases::phrases;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

phrases! {
    BatchNotAccepted: "The trainer cannot accept another command."
    pub UniqueLoading: "Reading unique descriptions…"
    pub PreviewLoading: "Updating preview…"
}

/// Optional game data has an explicit pending state; display copy never controls readiness.
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup<T> {
    Ready(T),
    Loading,
    Unavailable(String),
}
impl<T> From<Result<T, String>> for Lookup<T> {
    fn from(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Self::Ready(value),
            Err(reason) => Self::Unavailable(reason),
        }
    }
}

use super::item::{AffixDefinition, ForgeItem, TierProperty, format_tier_range};
pub use crate::write::WriteOutcome;

/// An independently observed item can be undisplayable by this editor. That
/// presentation limitation never downgrades the completed write certainty.
pub type EditOutcome = WriteOutcome<Option<ForgeItem>>;

pub fn refused(message: impl Into<String>) -> EditOutcome {
    EditOutcome::refused(message)
}

/// A base of an item type, from the captured item list.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseDefinition {
    pub sub_type: u16,
    pub name: String,
    pub level: u8,
    pub corrupted: bool,
    /// The classes an item on this base is for (the game's `ItemClassRequirements` bits, 0 for any class). A base change
    /// judges the affixes with it.
    pub class_requirement: i32,
    /// The implicits it grants, in the game's words with their ranges ("+10–16 Armor").
    pub implicits: Vec<String>,
    /// What it does to an affix's values (`Catalog::value_scale`).
    pub affix_effect_modifier: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemTypeDefinition {
    pub item_type: u16,
    pub name: String,
    pub bases: Vec<BaseDefinition>,
}

/// The game's own ranges for item-level values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemLimits {
    pub forging_potential: (u8, u8),
    pub legendary_potential: (u8, u8),
    pub weavers_will: (u8, u8),
    pub weavers_touch: (u8, u8),
    pub level_requirement: (u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendaryType {
    LegendaryPotential,
    WeaversWill,
}

/// A unique or set item from the captured data.
#[derive(Clone, Debug, PartialEq)]
pub struct UniqueDefinition {
    pub id: u16,
    pub name: String,
    pub item_type: u16,
    /// The bases it comes on.
    pub sub_types: Vec<u16>,
    pub legendary_type: LegendaryType,
    pub is_set: bool,
    pub can_drop_as_legendary: bool,
}

/// The captured game data the forge reads.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub uniques: Vec<UniqueDefinition>,
    pub definitions: Vec<AffixDefinition>,
    /// Each tier's value ranges by (affix id, tier), one per stat line as the game data writes them; a line it cannot
    /// format is none, and its tier is left blank, never guessed.
    pub tier_columns: std::collections::HashMap<(u16, u8), Vec<Option<TierProperty>>>,
    /// The item modifier each affix's values are written for (`standardAffixEffectModifier`).
    pub standard_effects: std::collections::HashMap<u16, f32>,
    /// The game's rarity names by rarity byte; empty entries fall back to the app's.
    pub rarity_names: Vec<String>,
    pub limits: Option<ItemLimits>,
    pub item_types: Vec<ItemTypeDefinition>,
}

impl Catalog {
    pub fn definition(&self, id: u16) -> AffixDefinition {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
            .cloned()
            .unwrap_or_else(|| AffixDefinition::unknown(id))
    }

    /// A source unique's name, as its modifiers carry it.
    pub fn source_name(&self, unique: u16) -> String {
        self.definitions
            .iter()
            .filter_map(|definition| definition.source.as_ref())
            .find(|source| source.unique == unique)
            .map_or_else(String::new, |source| source.name.clone())
    }

    /// What an item on `base` (type and subtype) does to this affix's values, as `AffixList.GetSingleAffixValue` works it
    /// out: `(1 + the base's modifier) / (1 + the affix's)`. Without a base, its values as written.
    pub fn value_scale(&self, id: u16, base: Option<(u16, u16)>) -> f64 {
        let standard = self.standard_effects.get(&id).copied().unwrap_or(0.0);
        match base.and_then(|(item_type, sub_type)| self.base(item_type, sub_type)) {
            Some(base) if base.affix_effect_modifier != standard => f64::from(1.0 + base.affix_effect_modifier) / f64::from(1.0 + standard),
            _ => 1.0,
        }
    }

    /// A tier's value ranges on an item on `base`.
    pub fn tier_range(&self, id: u16, tier: u8, base: Option<(u16, u16)>) -> String {
        let scale = self.value_scale(id, base);
        let columns = self
            .tier_columns
            .get(&(id, tier))
            .map(|columns| columns.iter().map(|column| column.map(|property| property.scaled(scale))).collect::<Option<Vec<_>>>());
        columns.flatten().as_deref().and_then(format_tier_range).unwrap_or_default()
    }

    /// Each stat line as a tooltip writes it, on an item on `base`: its value where every tier has the same, then its words;
    /// where tiers differ, or the game hides the value, its words alone.
    pub fn effect_lines(&self, definition: &AffixDefinition, base: Option<(u16, u16)>) -> Vec<String> {
        phrases! { WithValue: "{value} {stat}" }
        let scale = self.value_scale(definition.id, base);
        let column_of = |column: usize, tier: u8| self.tier_columns.get(&(definition.id, tier)).and_then(|columns| columns.get(column).copied().flatten());
        let mut lines: Vec<String> = Vec::with_capacity(definition.effects.len());
        for (column, effect) in definition.effects.iter().enumerate() {
            let value = column_of(column, 1)
                .filter(|first| !effect.hides_value && !effect.words.is_empty() && (2..=definition.max_tier).all(|tier| column_of(column, tier) == Some(*first)))
                .and_then(|first| format_tier_range(&[first.scaled(scale).unsigned()]))
                .map(|value| if effect.no_plus { value.trim_start_matches('+').to_owned() } else { value });
            let line = match value {
                Some(value) => WithValue.with(&[("value", &value), ("stat", &effect.words)]),
                None => effect.heading.clone(),
            };
            if !line.is_empty() && !lines.contains(&line) {
                lines.push(line);
            }
        }
        lines
    }

    pub fn rarity_name(&self, rarity: u8) -> Option<String> {
        self.rarity_names.get(rarity as usize).filter(|name| !name.is_empty()).cloned()
    }

    pub fn unique(&self, id: u16) -> Option<&UniqueDefinition> {
        self.uniques.iter().find(|unique| unique.id == id)
    }

    pub fn item_type(&self, item_type: u16) -> Option<&ItemTypeDefinition> {
        self.item_types.iter().find(|definition| definition.item_type == item_type)
    }

    pub fn base(&self, item_type: u16, sub_type: u16) -> Option<&BaseDefinition> {
        self.item_type(item_type)?.bases.iter().find(|base| base.sub_type == sub_type)
    }
}

phrases! {
    /// The fallback shown when the game gave no text for an implicit.
    pub ImplicitTextUnavailable: "Implicit (the game's text is unavailable)"
}

/// One base implicit as a line: the game's text at both ends of its range, joined as a range, or the fallback when the
/// game gave neither.
pub fn implicit_line(minimum: Option<&str>, maximum: Option<&str>) -> String {
    let text = modifier_range(minimum.unwrap_or_default(), maximum.unwrap_or_default());
    if text.is_empty() { ImplicitTextUnavailable.into() } else { text }
}

/// The game's text with the rolled number shown as its range, for example "+10–16 Armor" from "+10 Armor" and
/// "+16 Armor". Texts that differ in more than a number read "minimum – maximum".
pub fn modifier_range(minimum: &str, maximum: &str) -> String {
    if minimum == maximum || minimum.is_empty() || maximum.is_empty() {
        return if minimum.is_empty() { maximum } else { minimum }.to_owned();
    }
    let low: Vec<char> = minimum.chars().collect();
    let high: Vec<char> = maximum.chars().collect();
    let number_part = |c: char| c.is_numeric() || c == '.' || c == ',';
    let mut prefix = 0;
    while prefix < low.len() && prefix < high.len() && low[prefix] == high[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < low.len() - prefix && suffix < high.len() - prefix && low[low.len() - suffix - 1] == high[high.len() - suffix - 1] {
        suffix += 1;
    }
    while prefix > 0 && number_part(low[prefix - 1]) {
        prefix -= 1;
    }
    while suffix > 0 && number_part(low[low.len() - suffix]) {
        suffix -= 1;
    }
    let is_number = |text: &[char]| text.first().is_some_and(|c| c.is_numeric()) && text.last().is_some_and(|c| c.is_numeric()) && text.iter().all(|c| number_part(*c));
    let (from, to) = (&low[prefix..low.len() - suffix], &high[prefix..high.len() - suffix]);
    if !is_number(from) || !is_number(to) {
        return format!("{minimum} – {maximum}");
    }
    let text = |chars: &[char]| chars.iter().collect::<String>();
    format!("{}{}–{}{}", text(&low[..prefix]), text(from), text(to), text(&low[low.len() - suffix..]))
}

/// A failed request, in the session's words.
pub type GameError = String;

phrases! {
    ReadOnly: "This connection offers no supported item edits."
    UndoGone: "The confirmed Undo is no longer available. Check the item and override setting."
    PreviewItemChanged: "The item changed before its preview could be read."
    PreviewUnsupported: "This preview draft is unsupported."
    PreviewUniqueOnly: "Only unique rolls have a tooltip preview."
    PreviewBusy: "The trainer cannot accept another preview request."
    ValuesItemChanged: "The item changed before its values could be read."
    RollUnsupported: "This roll is unsupported."
    ValuesBusy: "The trainer cannot accept another value request."
    EditPending: "An item edit is already pending."
    EditUnavailable: "This edit is unavailable in this build."
    ItemChanged: "The item changed. Discard the stale draft and edit it again."
    EditNotAccepted: "The trainer cannot accept the edit. Nothing was submitted."
    NoTerminalOutcome: "The worker ended without a terminal write outcome."
    NoWriteRecord: "The worker stopped without a completed write record."
}

/// How long a draft's preview waits for the next draft.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(150);

/// What the forge asks of the worker. Whoever performs it hears nothing here: its answer comes back as an intent.
pub enum Request {
    /// Send an edit, or an Undo. Its receipt must be waited for to its end, whatever else happens.
    Write(Box<Command>),
    /// The edits that follow are one Undo. If the worker can't take this, the write after it must not go.
    BeginBatch,
    /// Read a value table. Cancelling it, by its id, drops the read.
    Mapping {
        id: u64,
        expected: Box<ExpectedObservation>,
        request: Box<MappingRequest>,
    },
    CancelMappings(Vec<u64>),
    /// Run `Intent::PreviewDue` after [`PREVIEW_DELAY`], in place of any earlier one.
    PreviewAfterDelay,
    Preview {
        expected: Box<ExpectedObservation>,
        draft: Box<RollEdit>,
    },
    Describe {
        expected: Box<ExpectedObservation>,
    },
}

struct PendingPreview {
    expected: ExpectedObservation,
    draft: RollEdit,
    /// The delay has passed, so it can be sent once no edit is on its way.
    due: bool,
    /// It was sent, and its answer hasn't come.
    asked: bool,
    error: Option<String>,
}

struct PendingMapping {
    id: u64,
    expected: ExpectedObservation,
    request: MappingRequest,
    error: Option<String>,
}

struct PendingEdit {
    before: Arc<BackendSnapshot>,
    plan: ForgePlan,
    answer: Option<Result<(), BackendError>>,
}

#[derive(Default)]
struct Work {
    pending: Option<PendingEdit>,
    /// A write that never left, with what it came to.
    refused: Option<EditOutcome>,
    mappings: Vec<PendingMapping>,
    mapping_read_sequence: Option<u64>,
    next_mapping: u64,
    preview: Option<PendingPreview>,
    descriptions: Option<(ExpectedObservation, u64)>,
    outbox: Vec<Request>,
}

impl Work {
    /// Drops the value reads `keep` turns down, and cancels those still out.
    fn retain_mappings(&mut self, mut keep: impl FnMut(&PendingMapping) -> bool) {
        let mut cancelled = Vec::new();
        self.mappings.retain(|pending| {
            let kept = keep(pending);
            if !kept && pending.error.is_none() {
                cancelled.push(pending.id);
            }
            kept
        });
        if !cancelled.is_empty() {
            self.outbox.push(Request::CancelMappings(cancelled));
        }
    }
}

/// The forge's game: plain data from the worker's latest snapshot, and an outbox of what the editor asks of the worker.
/// Holding no handle on the worker, it can never read a state newer than the one the editor is working from, and nothing
/// here can block.
pub struct ForgeGame {
    catalog: Arc<Catalog>,
    snapshot: Arc<BackendSnapshot>,
    running: bool,
    /// Moves with each snapshot and catalog taken, for a view to tell whether what it read from them can have changed.
    generation: u64,
    work: RefCell<Work>,
}

impl ForgeGame {
    pub fn new(snapshot: BackendSnapshot, running: bool) -> ForgeGame {
        ForgeGame {
            catalog: Arc::new(Catalog::default()),
            snapshot: Arc::new(snapshot),
            running,
            generation: 0,
            work: RefCell::default(),
        }
    }

    /// Takes what the worker has published, without touching what is on its way.
    pub(super) fn observe(&mut self, snapshot: Arc<BackendSnapshot>, running: bool) {
        self.snapshot = snapshot;
        self.running = running;
        self.generation += 1;
    }

    pub(super) fn set_catalog(&mut self, catalog: Arc<Catalog>) {
        self.catalog = catalog;
        self.generation += 1;
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn snapshot(&self) -> &BackendSnapshot {
        &self.snapshot
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// What the editor has asked of the worker since this was last called.
    pub(super) fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut self.work.borrow_mut().outbox)
    }

    /// Production and authored captures use the same class, category and idol rules.
    pub(crate) fn compatibility(&self, item: &ForgeItem, definition: &AffixDefinition, slot: Option<&item::AffixState>, unrestricted: bool) -> item::Compatibility {
        item::compatibility(item, definition, slot, unrestricted)
    }
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    pub(crate) fn shared_catalog(&self) -> Arc<Catalog> {
        self.catalog.clone()
    }
    pub(crate) fn read(&self) -> Option<ForgeItem> {
        if !self.running {
            return None;
        }
        project_item(&self.snapshot).ok().flatten()
    }
    pub(crate) fn can_write(&self) -> bool {
        self.running && self.work.borrow().pending.is_none() && self.snapshot.phase == Phase::Ready && self.snapshot.activity.is_none()
    }
    pub(crate) fn unrestricted(&self) -> bool {
        self.snapshot.override_enabled
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.running && self.snapshot.phase == Phase::Ready && self.snapshot.activity.is_some()
    }
    pub(crate) fn supports(&self, edit: &ItemEdit) -> bool {
        self.running && self.snapshot.capabilities.allows(edit)
    }
    pub(crate) fn read_only_reason(&self) -> Option<&str> {
        let capabilities = self.snapshot.capabilities;
        (!capabilities.status_fields.iter().any(|s| *s)
            && !capabilities.base_change
            && !capabilities.identity_change
            && !capabilities.affix_rolls
            && !capabilities.modifier_rolls
            && !capabilities.affix_change
            && !capabilities.affix_list)
            .then_some(ReadOnly.get())
    }
    pub(crate) fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        self.submit(expected, edit, false)
    }
    /// Asks for the batch; if the worker can't take it, the write that follows comes back refused.
    pub(crate) fn begin_batch(&mut self) -> Result<(), GameError> {
        self.work.borrow_mut().outbox.push(Request::BeginBatch);
        Ok(())
    }
    pub(crate) fn undo_depth(&self) -> usize {
        self.snapshot.undo_forge_depth
    }
    pub(crate) fn undo_needs_override(&self) -> bool {
        self.snapshot.undo_forge_needs_override
    }
    pub(crate) fn undo_intent(&self) -> Option<ItemEdit> {
        let state = &self.snapshot;
        state.forge_undo_plan(state.undo_forge.as_ref()?).ok().map(|plan| plan.edit_intent().clone())
    }
    pub(crate) fn undo(&mut self, expected: &ForgeItem) -> EditOutcome {
        let Some(edit) = self.undo_intent() else {
            return refused(UndoGone);
        };
        self.submit(expected, &edit, true)
    }
    pub(crate) fn clear_unique_preview(&self) {
        self.work.borrow_mut().preview = None;
    }

    /// Plans the edit on the latest read and asks for it at once. The helper compares the complete item with the one the
    /// plan was made on before it stores anything, so an item changed since that read is refused, never overwritten.
    fn submit(&mut self, expected: &ForgeItem, edit: &ItemEdit, undoing: bool) -> EditOutcome {
        let mut work = self.work.borrow_mut();
        if work.pending.is_some() {
            return refused(EditPending);
        }
        work.retain_mappings(|_| false);
        let state = self.snapshot.clone();
        if !state.capabilities.allows(edit) || !self.running {
            return refused(EditUnavailable);
        }
        if project_item(&state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(expected)) {
            return refused(ItemChanged);
        }
        let undo = if undoing { state.undo_forge.clone() } else { None };
        let planned = if undoing {
            undo.as_ref().ok_or(crate::item::edit::EditError::StaleItem).and_then(|undo| state.forge_undo_plan(undo))
        } else {
            state.forge_plan(edit.clone())
        };
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return refused(error.to_string()),
        };
        let command = match undo {
            Some(undo) => Command::UndoForge(Box::new(undo)),
            None => Command::Write(plan.clone().into()),
        };
        work.outbox.push(Request::Write(Box::new(command)));
        work.pending = Some(PendingEdit {
            before: state,
            plan,
            answer: None,
        });
        WriteOutcome::Pending
    }

    /// The write's receipt came back.
    pub(super) fn write_answered(&self, result: Result<(), BackendError>) {
        if let Some(pending) = self.work.borrow_mut().pending.as_mut() {
            pending.answer = Some(result);
        }
    }

    /// The worker took no write: nothing was submitted.
    pub(super) fn write_refused(&self, batch: bool) {
        let mut work = self.work.borrow_mut();
        work.pending = None;
        work.refused = Some(if batch { refused(BatchNotAccepted) } else { refused(EditNotAccepted) });
    }

    /// What a write came to, once its receipt has come back. Only the worker's record of this plan says it was kept.
    pub(crate) fn poll_write(&mut self) -> Option<EditOutcome> {
        let mut work = self.work.borrow_mut();
        if let Some(outcome) = work.refused.take() {
            return Some(outcome);
        }
        let result = work.pending.as_mut()?.answer.take()?;
        let pending = work.pending.take()?;
        let state = &self.snapshot;
        let record = state.last_write.as_ref().filter(|r| r.plan == pending.plan);
        // A failure before admission must not pick up an earlier identical
        // plan's result. Once a new record exists, it outranks receipt errors.
        if let Some(record) = record.filter(|r| result.is_ok() || pending.before.last_write.as_ref() != Some(*r)) {
            if matches!(record.status, WriteOutcome::Pending) {
                return Some(WriteOutcome::uncertain(NoTerminalOutcome));
            }
            return Some(record.status.clone().map(|actual| {
                let mut observed = (*pending.before).clone();
                let resolved_name = observed.catalog.data().and_then(|c| c.resolve_item_name(&actual));
                observed.forge = BackendForgeState::Item {
                    generation: observed.generation,
                    snapshot: Arc::new(actual),
                    resolved_name,
                    implicits: None,
                    unique: None,
                };
                // Keep projection failure explicit; never substitute the
                // requested presentation for an undisplayable readback.
                project_item(&observed).ok().flatten()
            }));
        }
        Some(match result {
            Err(error) if error.delivery != Some(crate::backend::Delivery::MayHaveRun) => refused(error.message),
            _ => WriteOutcome::uncertain(NoWriteRecord),
        })
    }

    pub(crate) fn preview_unique(&self, item: &ForgeItem, edit: &ItemEdit) -> Lookup<Vec<ModifierSnapshot>> {
        let result: Result<Lookup<Vec<ModifierSnapshot>>, GameError> = (|| {
            let draft = match edit {
                ItemEdit::Roll(draft) => Some(draft),
                _ => None,
            };
            let state = &self.snapshot;
            if !self.running || project_item(state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                return Err(PreviewItemChanged.into());
            }
            let expected = state.expected_observation().map_err(|e| e.to_string())?;
            let cached = state
                .unique_preview
                .as_ref()
                .filter(|p| p.expected == expected && Some(&p.draft) == draft)
                .map(|preview| preview.result.as_ref().map(|p| p.preview().modifiers.clone()).map_err(Clone::clone));
            let Some(draft) = draft.cloned() else {
                return Err(PreviewUnsupported.into());
            };
            if draft.target != RollTarget::Unique {
                return Err(PreviewUniqueOnly.into());
            }
            if let Some(cached) = cached {
                return cached.map(Lookup::Ready);
            }
            let mut work = self.work.borrow_mut();
            if work.preview.as_ref().is_none_or(|p| p.expected != expected || p.draft != draft) {
                work.preview = Some(PendingPreview {
                    expected,
                    draft,
                    due: false,
                    asked: false,
                    error: None,
                });
                work.outbox.push(Request::PreviewAfterDelay);
            }
            if let Some(error) = work.preview.as_ref().and_then(|p| p.error.clone()) {
                return Err(error);
            }
            Ok(Lookup::Loading)
        })();
        result.unwrap_or_else(Lookup::Unavailable)
    }

    /// The delay has passed: the preview waits for no further drafts.
    pub(super) fn preview_due(&self) {
        if let Some(preview) = self.work.borrow_mut().preview.as_mut() {
            preview.due = true;
        }
        self.send_preview();
    }

    /// Lets go of a preview for an item no longer shown, and sends one whose turn has come.
    pub(super) fn send_preview(&self) {
        let expected = self.snapshot.expected_observation();
        let mut work = self.work.borrow_mut();
        if work.preview.as_ref().is_some_and(|p| expected.as_ref() != Ok(&p.expected)) {
            work.preview = None;
            return;
        }
        let edit_waiting = work.pending.is_some();
        let Some(preview) = work.preview.as_mut() else {
            return;
        };
        if preview.due && !preview.asked && !edit_waiting {
            preview.asked = true;
            let request = Request::Preview {
                expected: Box::new(preview.expected.clone()),
                draft: Box::new(preview.draft.clone()),
            };
            work.outbox.push(request);
        }
    }

    pub(super) fn preview_answered(&self, error: Option<String>) {
        if let Some(preview) = self.work.borrow_mut().preview.as_mut() {
            preview.asked = false;
            preview.due = false;
            preview.error = error;
        }
    }

    pub(super) fn preview_refused(&self) {
        self.preview_answered(Some(PreviewBusy.into()));
    }

    pub(crate) fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> Lookup<ValueTable> {
        let result: Result<Lookup<ValueTable>, GameError> = (|| {
            let mapping = u8::try_from(request.index).ok().map(|index| MappingRequest {
                address: MappingAddress {
                    target: mapping_target(request.target),
                    index,
                    line: request.line.and_then(|line| u32::try_from(line).ok()),
                },
                draft: request.draft.clone(),
            });
            let state = &self.snapshot;
            if !self.running || project_item(state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                return Err(ValuesItemChanged.into());
            }
            let expected = state.expected_observation().map_err(|error| error.to_string());
            let ready = expected
                .as_ref()
                .ok()
                .zip(mapping.as_ref())
                .and_then(|(expected, mapping)| state.mapping(expected, mapping))
                .map(|result| result.as_ref().map(|table| value_table(table)).map_err(Clone::clone));
            let read_sequence = state.forge_read_sequence;
            let mut work = self.work.borrow_mut();
            if work.mapping_read_sequence != Some(read_sequence) {
                // A fresh owned read permits another optional read attempt. Merely
                // republishing a mapping result must never create a retry loop.
                work.mapping_read_sequence = Some(read_sequence);
                work.retain_mappings(|pending| pending.error.is_none());
            }
            let expected = expected?;
            let request = mapping.ok_or(RollUnsupported.get())?;
            work.retain_mappings(|pending| pending.expected == expected && (pending.request.address != request.address || pending.request == request));
            if let Some(result) = ready {
                return result.map(Lookup::Ready);
            }
            if let Some(error) = work.mappings.iter().find(|pending| pending.request == request).and_then(|pending| pending.error.clone()) {
                return Err(error);
            }
            if !work.mappings.iter().any(|pending| pending.request == request) && work.pending.is_none() {
                work.next_mapping += 1;
                let id = work.next_mapping;
                work.outbox.push(Request::Mapping {
                    id,
                    expected: Box::new(expected.clone()),
                    request: Box::new(request.clone()),
                });
                work.mappings.push(PendingMapping {
                    id,
                    expected,
                    request,
                    error: None,
                });
            }
            Ok(Lookup::Loading)
        })();
        result.unwrap_or_else(Lookup::Unavailable)
    }

    pub(crate) fn retain_mappings(&self, requests: &[RollRequest]) {
        let expected = self.snapshot.expected_observation().ok();
        self.work.borrow_mut().retain_mappings(|pending| {
            expected.as_ref() == Some(&pending.expected)
                && requests.iter().any(|request| {
                    mapping_target(request.target) == pending.request.address.target
                        && request.index == usize::from(pending.request.address.index)
                        && request.draft == pending.request.draft
                        && request.line == pending.request.address.line.and_then(|line| usize::try_from(line).ok())
                })
        });
    }

    /// A value read came back. True when it was still wanted.
    pub(super) fn mapping_answered(&self, id: u64, error: Option<String>) -> bool {
        let mut work = self.work.borrow_mut();
        let Some(at) = work.mappings.iter().position(|pending| pending.id == id) else {
            return false;
        };
        match error {
            None => {
                work.mappings.remove(at);
            }
            Some(error) => work.mappings[at].error = Some(error),
        }
        true
    }

    /// The worker took no value read, for now.
    pub(super) fn mapping_refused(&self, id: u64) -> bool {
        self.mapping_answered(id, Some(ValuesBusy.into()))
    }

    /// Asks for a unique's descriptions once per read of an item that has none yet.
    pub(super) fn ask_descriptions(&self) {
        let state = &self.snapshot;
        let undescribed = matches!(&state.forge, BackendForgeState::Item { unique: None, .. });
        let read_sequence = state.forge_read_sequence;
        let mut work = self.work.borrow_mut();
        let Ok(expected) = state.expected_observation() else {
            work.descriptions = None;
            return;
        };
        if !matches!(expected.snapshot().rarity, 7..=9) {
            work.descriptions = None;
            return;
        }
        if !undescribed {
            return;
        }
        if work
            .descriptions
            .as_ref()
            .is_none_or(|(before, sequence)| before != &expected || *sequence != read_sequence)
        {
            work.outbox.push(Request::Describe {
                expected: Box::new(expected.clone()),
            });
            work.descriptions = Some((expected, read_sequence));
        }
    }

    /// The worker took no request for descriptions: the next look asks again.
    pub(super) fn descriptions_refused(&self) {
        self.work.borrow_mut().descriptions = None;
    }
}

/// Which of the item's rolls a value table maps.
fn mapping_target(target: RollTarget) -> MappingTarget {
    match target {
        RollTarget::Affix => MappingTarget::Affix,
        RollTarget::Unique => MappingTarget::Unique,
        RollTarget::Implicit => MappingTarget::Implicit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_lines_join_the_games_texts_as_a_range() {
        assert_eq!(modifier_range("+10 Armor", "+16 Armor"), "+10–16 Armor");
        assert_eq!(modifier_range("+12% Fire Resistance", "+18% Fire Resistance"), "+12–18% Fire Resistance");
        assert_eq!(modifier_range("+1.5 Mana Regen", "+12.5 Mana Regen"), "+1.5–12.5 Mana Regen");
        assert_eq!(modifier_range("+5 Ward", "+5 Ward"), "+5 Ward");
        assert_eq!(modifier_range("Armour +5", "Ward +9"), "Armour +5 – Ward +9");
        assert_eq!(implicit_line(Some(""), Some("+16 Armor")), "+16 Armor");
        assert_eq!(implicit_line(Some("+10 Armor"), None), "+10 Armor");
        assert_eq!(implicit_line(None, None), ImplicitTextUnavailable.get());
    }
}
