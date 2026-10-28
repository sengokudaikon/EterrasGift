//! The Item Forge on the trainer's worker: the editor's game (`ForgeGame`) reads the worker's snapshots in place, and
//! its writes, value tables, previews and unique descriptions are commands whose receipts the writer polls. No game,
//! pipe or process work runs here.
use super::{
    ForgeHandle,
    game::{Catalog, EditOutcome, GameError, WriteOutcome, refused},
    item::{self, AffixDefinition, AffixKind, Category, ForgeItem, ItemEdit, ModifierSnapshot, RollRequest, ValueTable},
};
use crate::{
    MainWindow, TrainerState,
    shell::{Page, ShellHandle},
};
use eg_core::fault::{FaultDetails, FaultLog};
use eg_trainer::{
    backend::{BackendHandle, BackendSnapshot, CatalogState, Command, ForgeSnapshot, ForgeState, MappingRead, Pending, Phase},
    definitions::catalog::{Availability, GameCatalog},
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

const READ_ONLY: &str = "This connection offers no supported item edits.";
/// How often local receipts and the preview's debounce are looked at while the writer has work; snapshots wake the
/// binding on their own.
const POLL: Duration = Duration::from_millis(50);

/// The forge's game: the worker's snapshots read in place, and its commands, whose receipts the writer polls.
pub struct ForgeGame {
    catalog: Arc<Catalog>,
    writer: Rc<RefCell<Writer>>,
}
impl ForgeGame {
    /// Production and authored captures use the same class, category and idol rules.
    pub(crate) fn compatibility(&self, item: &ForgeItem, definition: &AffixDefinition, slot: Option<&item::AffixState>, unrestricted: bool) -> item::Compatibility {
        item::compatibility(item, definition, slot, unrestricted)
    }
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    pub(crate) fn read(&self) -> Option<ForgeItem> {
        let writer = self.writer.borrow();
        if !writer.backend.is_running() {
            return None;
        }
        writer.backend.peek(project_item).ok().flatten()
    }
    // The editor asks these per row, option and cell: they read the snapshot in place rather than copy it each time.
    pub(crate) fn can_write(&self) -> bool {
        let writer = self.writer.borrow();
        writer.backend.is_running() && writer.pending.is_none() && writer.backend.peek(|state| state.phase == Phase::Ready && state.activity.is_none())
    }
    pub(crate) fn unrestricted(&self) -> bool {
        self.writer.borrow().backend.peek(|state| state.override_enabled)
    }
    pub(crate) fn is_busy(&self) -> bool {
        let writer = self.writer.borrow();
        writer.backend.is_running() && writer.backend.peek(|state| state.phase == Phase::Ready && state.activity.is_some())
    }
    pub(crate) fn supports(&self, edit: &ItemEdit) -> bool {
        let writer = self.writer.borrow();
        writer.backend.is_running() && writer.backend.peek(|state| supports(state, edit))
    }
    pub(crate) fn read_only_reason(&self) -> Option<&str> {
        let capabilities = self.writer.borrow().backend.peek(|state| state.capabilities);
        (!capabilities.status_fields.iter().any(|s| *s)
            && !capabilities.base_change
            && !capabilities.identity_change
            && !capabilities.affix_rolls
            && !capabilities.modifier_rolls
            && !capabilities.affix_change
            && !capabilities.affix_list)
            .then_some(READ_ONLY)
    }
    pub(crate) fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> super::game::Lookup<ValueTable> {
        self.writer.borrow_mut().resolve(item, request)
    }
    pub(crate) fn retain_mappings(&self, requests: &[RollRequest]) {
        self.writer.borrow_mut().retain_mappings(requests);
    }
    pub(crate) fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        self.writer.borrow_mut().submit(expected, edit, false)
    }
    pub(crate) fn begin_batch(&mut self) -> Result<(), GameError> {
        self.writer
            .borrow()
            .backend
            .try_submit(Command::BeginForgeBatch)
            .map(|_| ())
            .map_err(|_| "The trainer cannot accept another command.".into())
    }
    pub(crate) fn undo_depth(&self) -> usize {
        self.writer.borrow().backend.peek(|state| state.undo_forge_depth)
    }
    pub(crate) fn undo_needs_override(&self) -> bool {
        self.writer.borrow().backend.peek(|state| state.undo_forge_needs_override)
    }
    pub(crate) fn undo_intent(&self) -> Option<ItemEdit> {
        self.writer
            .borrow()
            .backend
            .peek(|state| state.forge_undo_plan(state.undo_forge.as_ref()?).ok().map(|plan| plan.edit_intent().clone()))
    }
    pub(crate) fn undo(&mut self, expected: &ForgeItem) -> EditOutcome {
        let Some(edit) = self.undo_intent() else {
            return refused("The confirmed Undo is no longer available. Check the item and override setting.");
        };
        self.writer.borrow_mut().submit(expected, &edit, true)
    }
    pub(crate) fn poll_write(&mut self) -> Option<EditOutcome> {
        self.writer.borrow_mut().poll()
    }
    pub(crate) fn clear_unique_preview(&self) {
        self.writer.borrow_mut().preview = None;
    }
    pub(crate) fn preview_unique(&self, item: &ForgeItem, edit: &ItemEdit) -> super::game::Lookup<Vec<ModifierSnapshot>> {
        self.writer.borrow_mut().preview(item, edit)
    }
}

// A receipt survives replacement of the owned observation provider. The UI never
// blocks on it, and never resubmits an accepted command after losing its reply.
struct Writer {
    backend: BackendHandle,
    pending: Option<PendingEdit>,
    mappings: Vec<PendingMapping>,
    mapping_read_sequence: Option<u64>,
    preview: Option<PendingPreview>,
    descriptions: Option<(eg_trainer::item::edit::ExpectedObservation, u64, Pending)>,
    /// Runs only while there is work (`keep_polling`); its tick refreshes the binding.
    polling: Timer,
    /// The binding's refresh, set once the binding exists; holds it weakly.
    tick: Option<Rc<dyn Fn()>>,
}
struct PendingPreview {
    expected: eg_trainer::item::edit::ExpectedObservation,
    draft: eg_trainer::item::edit::RollEdit,
    starts: Option<std::time::Instant>,
    receipt: Option<Pending>,
    error: Option<String>,
}
struct PendingMapping {
    expected: eg_trainer::item::edit::ExpectedObservation,
    request: eg_trainer::item::values::MappingRequest,
    receipt: MappingRead,
    error: Option<String>,
}
struct PendingEdit {
    receipt: Pending,
    expected: ForgeItem,
    edit: ItemEdit,
    undoing: bool,
    before: BackendSnapshot,
    plan: Option<eg_trainer::write::ForgePlan>,
}
impl Writer {
    fn has_pending_work(&self) -> bool {
        self.pending.is_some()
            || self.preview.as_ref().is_some_and(|preview| preview.starts.is_some() || preview.receipt.is_some())
            || self.mappings.iter().any(|mapping| mapping.error.is_none())
    }
    /// Starts polling when work is pending and stops once it is drained: an idle trainer wakes nothing.
    fn keep_polling(&self) {
        if !self.has_pending_work() {
            self.polling.stop();
        } else if !self.polling.running()
            && let Some(tick) = self.tick.clone()
        {
            self.polling.start(TimerMode::Repeated, POLL, move || tick());
        }
    }
    fn poll_descriptions(&mut self) {
        let (expected, undescribed, read_sequence) = self.backend.peek(|state| {
            (
                state.expected_observation(),
                matches!(&state.forge, ForgeState::Item { unique: None, .. }),
                state.forge_read_sequence,
            )
        });
        let Ok(expected) = expected else {
            self.descriptions = None;
            return;
        };
        if !matches!(expected.snapshot().rarity, 7..=9) {
            self.descriptions = None;
            return;
        }
        if !undescribed {
            return;
        }
        if self
            .descriptions
            .as_ref()
            .is_none_or(|(before, sequence, _)| before != &expected || *sequence != read_sequence)
            && let Ok(receipt) = self.backend.try_submit(Command::DescribeUnique {
                expected: Box::new(expected.clone()),
            })
        {
            self.descriptions = Some((expected, read_sequence, receipt));
        }
    }

    fn preview(&mut self, item: &ForgeItem, edit: &ItemEdit) -> super::game::Lookup<Vec<ModifierSnapshot>> {
        let result: Result<super::game::Lookup<Vec<ModifierSnapshot>>, GameError> = (|| {
            let draft = match edit {
                eg_trainer::item::edit::ItemEdit::Roll(draft) => Some(draft),
                _ => None,
            };
            let closed = !self.backend.is_running();
            let (expected, cached) = self.backend.peek(|state| -> Result<_, GameError> {
                if closed || project_item(state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                    return Err("The item changed before its preview could be read.".into());
                }
                let expected = state.expected_observation().map_err(|e| e.to_string())?;
                let cached = state
                    .unique_preview
                    .as_ref()
                    .filter(|p| p.expected == expected && Some(&p.draft) == draft)
                    .map(|preview| preview.result.as_ref().map(|p| p.preview().modifiers.clone()).map_err(Clone::clone));
                Ok((expected, cached))
            })?;
            let Some(draft) = draft.cloned() else {
                return Err("This preview draft is unsupported.".into());
            };
            if draft.target != eg_trainer::item::edit::RollTarget::Unique {
                return Err("Only unique rolls have a tooltip preview.".into());
            }
            if let Some(cached) = cached {
                return cached.map(super::game::Lookup::Ready);
            }
            if self.preview.as_ref().is_none_or(|p| p.expected != expected || p.draft != draft) {
                self.preview = Some(PendingPreview {
                    expected,
                    draft,
                    starts: Some(std::time::Instant::now() + std::time::Duration::from_millis(150)),
                    receipt: None,
                    error: None,
                });
            }
            if let Some(error) = self.preview.as_ref().and_then(|p| p.error.clone()) {
                return Err(error);
            }
            Ok(super::game::Lookup::Loading)
        })();
        self.keep_polling();
        result.unwrap_or_else(super::game::Lookup::Unavailable)
    }
    fn poll_preview(&mut self) -> bool {
        if self
            .preview
            .as_ref()
            .is_some_and(|p| self.backend.peek(|state| state.expected_observation().as_ref() != Ok(&p.expected)))
        {
            self.preview = None;
            return false;
        }
        let Some(preview) = self.preview.as_mut() else {
            return false;
        };
        if preview.starts.is_some_and(|at| std::time::Instant::now() >= at) && self.pending.is_none() {
            preview.starts = None;
            match self.backend.try_submit(Command::PreviewUnique {
                expected: Box::new(preview.expected.clone()),
                draft: preview.draft.clone(),
            }) {
                Ok(receipt) => preview.receipt = Some(receipt),
                Err(_) => {
                    preview.error = Some("The trainer cannot accept another preview request.".into());
                    return true;
                }
            }
        }
        if let Some(result) = preview.receipt.as_mut().and_then(Pending::try_result) {
            preview.receipt = None;
            if let Err(error) = result {
                preview.error = Some(error.message);
            }
            return true;
        }
        false
    }

    fn resolve(&mut self, item: &ForgeItem, request: &RollRequest) -> super::game::Lookup<ValueTable> {
        let result: Result<super::game::Lookup<ValueTable>, GameError> = (|| {
            use eg_trainer::item::values::{MappingAddress, MappingRequest, MappingTarget};
            if request.target != item::RollTarget::Affix {
                return Err("the game's values are read for affixes only".into());
            }
            let mapping = u8::try_from(request.index).ok().map(|index| MappingRequest {
                address: MappingAddress {
                    target: MappingTarget::Affix,
                    index,
                    line: None,
                },
                draft: request.draft.clone(),
            });
            let closed = !self.backend.is_running();
            let (read_sequence, expected, ready) = self.backend.peek(|state| -> Result<_, GameError> {
                if closed || project_item(state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                    return Err("The item changed before its values could be read.".into());
                }
                let expected = state.expected_observation().map_err(|error| error.to_string());
                let ready = expected
                    .as_ref()
                    .ok()
                    .zip(mapping.as_ref())
                    .and_then(|(expected, mapping)| state.mapping(expected, mapping))
                    .map(|result| result.as_ref().map(|table| value_table(table)).map_err(Clone::clone));
                Ok((state.forge_read_sequence, expected, ready))
            })?;
            self.poll_mappings();
            if self.mapping_read_sequence != Some(read_sequence) {
                // A fresh owned read permits another optional read attempt. Merely
                // republishing a mapping result must never create a retry loop.
                self.mapping_read_sequence = Some(read_sequence);
                self.mappings.retain(|pending| pending.error.is_none());
            }
            let expected = expected?;
            let request = mapping.ok_or("This affix slot is unsupported.")?;
            self.mappings
                .retain(|pending| pending.expected == expected && (pending.request.address != request.address || pending.request == request));
            if let Some(result) = ready {
                return result.map(super::game::Lookup::Ready);
            }
            self.mappings.retain(|pending| pending.expected == expected);
            if let Some(error) = self.mappings.iter().find(|pending| pending.request == request).and_then(|pending| pending.error.clone()) {
                return Err(error);
            }
            if !self.mappings.iter().any(|pending| pending.expected == expected && pending.request == request) && self.pending.is_none() {
                let receipt = self
                    .backend
                    .try_resolve_mapping(expected.clone(), request.clone())
                    .map_err(|_| "The trainer cannot accept another value request.")?;
                self.mappings.push(PendingMapping {
                    expected,
                    request,
                    receipt,
                    error: None,
                });
            }
            Ok(super::game::Lookup::Loading)
        })();
        self.keep_polling();
        result.unwrap_or_else(super::game::Lookup::Unavailable)
    }
    fn retain_mappings(&mut self, requests: &[RollRequest]) {
        let expected = self.backend.peek(|state| state.expected_observation().ok());
        self.mappings.retain(|pending| {
            expected.as_ref() == Some(&pending.expected)
                && requests.iter().any(|request| {
                    request.target == item::RollTarget::Affix
                        && request.index == usize::from(pending.request.address.index)
                        && request.draft == pending.request.draft
                        && request.line == pending.request.address.line.and_then(|line| usize::try_from(line).ok())
                })
        });
    }
    fn poll_mappings(&mut self) -> bool {
        let mut completed = false;
        self.mappings.retain_mut(|pending| {
            if pending.error.is_some() {
                return true;
            }
            let Some(result) = pending.receipt.try_result() else {
                return true;
            };
            completed = true;
            match result {
                Ok(()) => false,
                Err(error) => {
                    pending.error = Some(error.message);
                    true
                }
            }
        });
        completed
    }
    fn submit(&mut self, expected: &ForgeItem, edit: &ItemEdit, undoing: bool) -> EditOutcome {
        if self.pending.is_some() {
            return refused("An item edit is already pending.");
        }
        self.mappings.clear();
        let state = self.backend.snapshot();
        if !supports(&state, edit) {
            return refused("This edit is unavailable in this build.");
        }
        if project_item(&state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(expected)) {
            return refused("The item changed. Discard the stale draft and edit it again.");
        }
        match self.backend.try_submit(Command::ReadForge) {
            Ok(receipt) => {
                self.pending = Some(PendingEdit {
                    receipt,
                    expected: expected.clone(),
                    edit: edit.clone(),
                    undoing,
                    before: state,
                    plan: None,
                });
                self.keep_polling();
                WriteOutcome::Pending
            }
            Err(_) => refused("The trainer cannot accept another command. Nothing was submitted."),
        }
    }
    fn poll(&mut self) -> Option<EditOutcome> {
        let result = self.pending.as_mut()?.receipt.try_result()?;
        let mut pending = self.pending.take().unwrap();
        let state = self.backend.snapshot();
        if let Some(plan) = &pending.plan {
            let record = state.last_write.as_ref().filter(|r| &r.plan == plan);
            // A failure before admission must not pick up an earlier identical
            // plan's result. Once a new record exists, it outranks receipt errors.
            if let Some(record) = record.filter(|r| result.is_ok() || pending.before.last_write.as_ref() != Some(*r)) {
                if matches!(record.status, WriteOutcome::Pending) {
                    return Some(WriteOutcome::uncertain("The worker ended without a terminal write outcome."));
                }
                return Some(record.status.clone().map(|actual| {
                    let mut observed = pending.before.clone();
                    let resolved_name = observed.catalog.data().and_then(|c| c.resolve_item_name(&actual));
                    observed.forge = ForgeState::Item {
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
            return Some(match result {
                Err(error) if error.delivery != Some(eg_trainer::backend::Delivery::MayHaveRun) => refused(error.message),
                _ => WriteOutcome::uncertain("The worker stopped without a completed write record."),
            });
        }
        if let Err(error) = result {
            return Some(refused(error.message));
        }
        // Full owned snapshots, not the presentation projection alone, protect
        // fields omitted from the editor. Each draft step obtains a new read.
        if state.generation != pending.before.generation
            || state.attached != pending.before.attached
            || !state
                .expected_observation()
                .ok()
                .zip(pending.before.expected_observation().ok())
                .is_some_and(|(current, before)| current == before)
            || project_item(&state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(&pending.expected))
        {
            return Some(refused("The item changed before submission. Discard the stale draft and edit it again."));
        }
        let undo = if pending.undoing { state.undo_forge.clone() } else { None };
        let planned = if pending.undoing {
            undo.as_ref()
                .ok_or(eg_trainer::item::edit::EditError::StaleItem)
                .and_then(|undo| state.forge_undo_plan(undo))
        } else {
            state.forge_plan(pending.edit.clone())
        };
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return Some(refused(error.to_string())),
        };
        let command = match undo {
            Some(undo) => Command::UndoForge(Box::new(undo)),
            None => Command::Write(plan.clone().into()),
        };
        match self.backend.try_submit(command) {
            Ok(receipt) => {
                pending.receipt = receipt;
                pending.plan = Some(plan);
                pending.before = state;
                self.pending = Some(pending);
                None
            }
            Err(_) => Some(refused("The trainer cannot accept the edit. Nothing was submitted.")),
        }
    }
}

fn supports(state: &BackendSnapshot, edit: &ItemEdit) -> bool {
    state.capabilities.allows(edit)
}

/// The forge on the worker: its item and catalog follow the snapshots, and its receipts settle as they come.
struct Binding {
    backend: BackendHandle,
    window: slint::Weak<MainWindow>,
    forge: ForgeHandle,
    revision: Cell<Option<u64>>,
    catalog: RefCell<(Option<Arc<GameCatalog>>, Arc<Catalog>)>,
    writer: Rc<RefCell<Writer>>,
    /// The forge is rebuilt only while its page shows.
    shell: ShellHandle,
    /// A read arrived while the Forge page was not showing: the forge is rebuilt from the latest one when it shows again.
    forge_stale: Cell<bool>,
    faults: Option<FaultLog>,
    /// The last item the forge could not show and that went to the fault log, so each is recorded once.
    reported: RefCell<Option<(u64, Arc<ForgeSnapshot>)>>,
}

/// Puts the forge on the worker. Its snapshot follower keeps the binding alive; the page's hook and the writer's polling
/// timer hold only weak references.
pub fn bind(window: &MainWindow, backend: BackendHandle, shell: &ShellHandle, faults: Option<FaultLog>) -> ForgeHandle {
    let empty = Arc::new(Catalog::default());
    let writer = Rc::new(RefCell::new(Writer {
        backend: backend.clone(),
        pending: None,
        mappings: Vec::new(),
        mapping_read_sequence: None,
        preview: None,
        descriptions: None,
        polling: Timer::default(),
        tick: None,
    }));
    let forge = super::attach(
        window,
        ForgeGame {
            catalog: empty.clone(),
            writer: writer.clone(),
        },
    );
    let binding = Rc::new(Binding {
        backend,
        window: window.as_weak(),
        forge,
        revision: Cell::new(None),
        catalog: RefCell::new((None, empty)),
        writer,
        shell: shell.clone(),
        forge_stale: Cell::new(false),
        faults,
        reported: RefCell::new(None),
    });
    shell.on_page_entered({
        let binding = Rc::downgrade(&binding);
        move |page| {
            if page == Page::Forge
                && let Some(binding) = binding.upgrade()
                && binding.forge_stale.replace(false)
            {
                binding.observe_forge();
            }
        }
    });
    let weak = Rc::downgrade(&binding);
    binding.writer.borrow_mut().tick = Some(Rc::new(move || {
        if let Some(binding) = weak.upgrade() {
            binding.refresh();
        }
    }));
    binding.refresh();
    // The snapshot follower owns the binding: it ends with the worker, or at the first snapshot after the window is gone.
    let owner = binding.clone();
    crate::pages::trainer::follow(&binding.backend, move || {
        if owner.window.upgrade().is_none() {
            return false;
        }
        owner.refresh();
        true
    });
    binding.forge.clone()
}

impl Binding {
    /// Takes what came, then polls on only while the writer still has work.
    fn refresh(&self) {
        self.observe();
        self.writer.borrow().keep_polling();
    }

    fn observe(&self) {
        self.writer.borrow_mut().poll_descriptions();
        let mappings_completed = self.writer.borrow_mut().poll_mappings();
        let preview_completed = self.writer.borrow_mut().poll_preview();
        // A tick without a new snapshot only settles the editor's receipts; the snapshot is copied only when it changed.
        let (phase, revision) = self.backend.peek(|state| (state.phase, state.revision));
        if !self.backend.is_running() && phase != Phase::Stopped {
            // An unexpected worker failure must not leave apparently live data.
            self.forge_stale.set(false);
            self.forge.observe(ForgeGame {
                catalog: Arc::new(Catalog::default()),
                writer: self.writer.clone(),
            });
            return;
        }
        if self.revision.get() == Some(revision) {
            if !mappings_completed && !preview_completed {
                // Only the editor's receipts can have moved on; while none did there is nothing new to show.
                self.forge.edit_if(|editor| editor.poll_writes());
            } else if self.shell.current() == Page::Forge {
                self.forge.edit(|editor| editor.refresh());
            } else {
                self.forge_stale.set(true);
                self.forge.edit_if(|editor| editor.poll_writes());
            }
            return;
        }
        let snapshot = self.backend.snapshot();
        self.revision.set(Some(snapshot.revision));
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let mut catalog = self.catalog.borrow_mut();
        if let Some(data) = snapshot.catalog.data()
            && catalog.0.as_ref().is_none_or(|old| !Arc::ptr_eq(old, data))
        {
            catalog.1 = Arc::new(project_catalog(data));
            catalog.0 = Some(data.clone());
        }
        let projection_error = project_item(&snapshot).err().map(|error| self.report_unshown(&snapshot, error));
        drop(catalog);
        if self.shell.current() == Page::Forge {
            self.forge_stale.set(false);
            self.observe_forge();
        } else {
            // Rebuilding the forge for every read while another page shows is wasted work: it is rebuilt once when its page
            // shows. A write in flight still settles.
            self.forge_stale.set(true);
            self.forge.edit_if(|editor| editor.poll_writes());
        }
        window.global::<TrainerState>().set_item_unavailable(projection_error.unwrap_or_default().into());
    }

    /// Puts the latest read and catalog behind the forge and shows it.
    fn observe_forge(&self) {
        let catalog = self.catalog.borrow().1.clone();
        self.forge.observe(ForgeGame {
            catalog,
            writer: self.writer.clone(),
        });
    }

    /// Records an item the forge cannot show, once, with the item as the game reported it, so the refused value can be
    /// traced. Returns what the forge says in its place.
    fn report_unshown(&self, state: &BackendSnapshot, error: String) -> String {
        let (Some(faults), ForgeState::Item { generation, snapshot, .. }) = (&self.faults, &state.forge) else {
            return error;
        };
        let mut reported = self.reported.borrow_mut();
        if reported.as_ref().is_none_or(|(g, s)| g != generation || s != snapshot) {
            *reported = Some((*generation, snapshot.clone()));
            let details = FaultDetails {
                code: Some("ItemNotShown".into()),
                readback: Some(serde_json::json!({ "generation": generation, "item": eg_trainer::backend::forge_record(snapshot) })),
                ..Default::default()
            };
            faults.write(
                "Item Forge",
                Some(&error),
                Some(&details),
                Some("The forge shows the reason in place of the item. Nothing was sent to the game."),
            );
        }
        format!("{error} Details are in the fault log.")
    }
}

fn available_text(text: &Availability<String>) -> Option<&str> {
    match text {
        Availability::Available(text) => Some(text),
        Availability::Unavailable { .. } => None,
    }
}
fn project_catalog(data: &GameCatalog) -> Catalog {
    // Presentation-only data. Native write planning always uses the canonical
    // GameCatalog in the worker, never this projection or guessed ranges/text.
    let mut catalog = Catalog::default();
    let pair = |range: eg_trainer::definitions::catalog::ValueRange| (range.minimum, range.maximum);
    catalog.limits = Some(super::game::ItemLimits {
        forging_potential: pair(data.limits.forging_potential),
        legendary_potential: pair(data.limits.legendary_potential),
        weavers_will: pair(data.limits.weavers_will),
        weavers_touch: pair(data.limits.weavers_touch),
        level_requirement: pair(data.limits.level_requirement),
    });
    catalog.item_types = data
        .bases
        .values()
        .map(|definition| super::game::ItemTypeDefinition {
            item_type: u16::from(definition.item_type),
            name: definition.name.text.clone(),
            bases: definition
                .sub_types
                .iter()
                .map(|base| super::game::BaseDefinition {
                    sub_type: base.sub_type,
                    name: base.name.text.clone(),
                    level: base.level_requirement.clamp(0, 255) as u8,
                    corrupted: base.is_corrupted,
                    class_requirement: base.class_requirement,
                    // C# `ForgeChoices.BaseOptions` lists each implicit with `ImplicitLine`; an undescribed list is none.
                    implicits: match &base.implicits {
                        Availability::Available(implicits) => implicits
                            .iter()
                            .map(|implicit| super::game::implicit_line(available_text(&implicit.minimum_text), available_text(&implicit.maximum_text)))
                            .collect(),
                        Availability::Unavailable { .. } => Vec::new(),
                    },
                })
                .collect(),
        })
        .collect();
    catalog.uniques = data
        .uniques
        .values()
        .filter_map(|definition| {
            Some(super::game::UniqueDefinition {
                id: definition.id,
                name: definition.name.text.clone(),
                item_type: u16::from(definition.item_type),
                sub_types: definition.sub_types.iter().map(|id| u16::from(*id)).collect(),
                legendary_type: match definition.legendary_type {
                    0 => super::game::LegendaryType::LegendaryPotential,
                    1 => super::game::LegendaryType::WeaversWill,
                    _ => return None,
                },
                is_set: definition.is_set_item,
                can_drop_as_legendary: definition.can_drop_as_legendary,
            })
        })
        .collect();
    for definition in data.affixes.values() {
        {
            // Every affix the game defines is kept, for its name at least. One of the game's other types (its SPECIAL)
            // is shown but cannot be written: an affix's type is part of what is written.
            let kind = match definition.kind {
                0 => Some(AffixKind::Prefix),
                1 => Some(AffixKind::Suffix),
                _ => None,
            };
            let name = match &definition.game_display_name {
                Availability::Available(text) if !text.is_empty() => text.clone(),
                _ if !definition.raw_display_name.is_empty() => definition.raw_display_name.clone(),
                _ => definition.raw_name.clone(),
            };
            catalog.definitions.push(AffixDefinition {
                id: definition.id,
                name,
                kind: kind.unwrap_or(AffixKind::Prefix),
                category: Category::from_raw(definition.special),
                item_types: definition
                    .item_types
                    .as_ref()
                    .map(|types| types.iter().filter_map(|t| u16::try_from(*t).ok()).collect())
                    .unwrap_or_default(),
                max_tier: definition.tier_count.min(255) as u8,
                writable: kind.is_some(),
            });
        }
    }
    catalog.rarity_names = vec![String::new(); 256];
    for (id, rarity) in &data.rarities {
        if let Availability::Available(name) = &rarity.name {
            catalog.rarity_names[usize::from(*id)] = name.text.clone();
        }
    }
    catalog
}
fn value_table(table: &eg_trainer::item::values::ValueTable) -> ValueTable {
    ValueTable {
        label: "Value".into(),
        samples: table
            .samples()
            .iter()
            .map(|sample| item::ValueSample {
                value: sample.value,
                display: sample.display.clone(),
                effects: sample.effects.clone(),
            })
            .collect(),
    }
}
fn project_item(state: &BackendSnapshot) -> Result<Option<ForgeItem>, String> {
    let ForgeState::Item {
        generation,
        snapshot,
        resolved_name,
        implicits,
        unique,
    } = &state.forge
    else {
        return Ok(None);
    };
    if state.phase != Phase::Ready || *generation != state.generation || !matches!(state.catalog, CatalogState::Current { generation, .. } if generation == state.generation) {
        return Ok(None);
    }
    let Some(_) = &state.attached else {
        return Ok(None);
    };
    let context = Arc::new(item::ForgeContext {
        expected: state.expected_observation().map_err(|e| e.to_string())?,
        catalog: state.catalog.data().cloned().ok_or("Catalog unavailable.")?,
    });
    let name = resolved_name.as_ref().map_or_else(
        || if snapshot.name.is_empty() { "Unnamed item".into() } else { snapshot.name.clone() },
        |name| name.text.clone(),
    );
    let mut item = ForgeItem::observed(context, name);
    item.implicits = implicits.as_ref().map(|lines| lines.to_vec());
    if let Some(identity) = item.unique.as_mut().or(item.set_item.as_mut()) {
        identity.modifiers = unique.as_ref().and_then(|result| result.as_ref().ok()).map(|lines| lines.to_vec()).unwrap_or_default();
        identity.formatting = match unique {
            Some(Ok(_)) => super::game::Lookup::Ready(()),
            Some(Err(reason)) => super::game::Lookup::Unavailable(reason.clone()),
            None => super::game::Lookup::Loading,
        };
    }
    Ok(Some(item))
}
