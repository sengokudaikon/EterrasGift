//! Command-based UI adapter. The worker owns native access; this module observes owned
//! snapshots and queues commands. No synchronous game/pipe/process work runs here.
use crate::trainer::backend_session::sentence;
use crate::{
    Connection, MainWindow, TrainerState,
    decorations::{Ash, Decorations},
    forge::{
        self,
        game::{Catalog, EditOutcome, ForgeGame, GameError, WriteOutcome, refused},
        item::{self, AffixDefinition, AffixKind, Category, ForgeItem, ItemEdit, ModifierState, RollRequest, ValueTable},
    },
    shell::ShellHandle,
};
use eg_core::{bloom::BloomPhase, game_log::GameReadiness};
use eg_trainer::{
    backend::{Activity, BackendHandle, BackendSnapshot, CatalogState, Command, ErrorKind, ForgeState, MappingRead, Pending, Phase},
    catalog::{Availability, GameCatalog},
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

struct ObservedGame {
    catalog: Arc<Catalog>,
    writer: Rc<RefCell<Writer>>,
}
impl ForgeGame for ObservedGame {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    fn read(&self) -> Option<ForgeItem> {
        let writer = self.writer.borrow();
        if writer.backend.subscribe().has_changed().is_err() {
            return None;
        }
        writer.backend.peek(project_item).ok().flatten()
    }
    // The editor asks these per row, option and cell: they read the snapshot in place rather than copy it each time.
    fn can_write(&self) -> bool {
        let writer = self.writer.borrow();
        writer.backend.subscribe().has_changed().is_ok() && writer.pending.is_none() && writer.backend.peek(|state| state.phase == Phase::Ready && state.activity.is_none())
    }
    fn unrestricted(&self) -> bool {
        self.writer.borrow().backend.peek(|state| state.override_enabled)
    }
    fn is_busy(&self) -> bool {
        let writer = self.writer.borrow();
        writer.backend.subscribe().has_changed().is_ok() && writer.backend.peek(|state| state.phase == Phase::Ready && state.activity.is_some())
    }
    fn supports(&self, edit: &ItemEdit) -> bool {
        let writer = self.writer.borrow();
        writer.backend.subscribe().has_changed().is_ok() && writer.backend.peek(|state| supports(state, edit))
    }
    fn read_only_reason(&self) -> Option<&str> {
        let capabilities = self.writer.borrow().backend.peek(|state| state.capabilities);
        (!capabilities.status_fields.iter().any(|s| *s)
            && !capabilities.base_change
            && !capabilities.identity_change
            && !capabilities.affix_rolls
            && !capabilities.affix_change
            && !capabilities.affix_list)
            .then_some(READ_ONLY)
    }
    fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> crate::forge::game::Lookup<ValueTable> {
        self.writer.borrow_mut().resolve(item, request)
    }
    fn retain_mappings(&self, requests: &[RollRequest]) {
        self.writer.borrow_mut().retain_mappings(requests);
    }
    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        self.writer.borrow_mut().submit(expected, edit, false)
    }
    fn begin_batch(&mut self) -> Result<(), GameError> {
        self.writer
            .borrow()
            .backend
            .try_submit(Command::BeginForgeBatch)
            .map(|_| ())
            .map_err(|_| "The trainer cannot accept another command.".into())
    }
    fn undo_depth(&self) -> usize {
        self.writer.borrow().backend.peek(|state| state.undo_forge_depth)
    }
    fn undo_needs_override(&self) -> bool {
        self.writer.borrow().backend.peek(|state| state.undo_forge_needs_override)
    }
    fn undo_intent(&self) -> Option<ItemEdit> {
        self.writer
            .borrow()
            .backend
            .peek(|state| state.forge_undo_plan(state.undo_forge.as_ref()?).ok().map(|plan| plan.edit_intent().clone()))
    }
    fn undo(&mut self, expected: &ForgeItem) -> EditOutcome {
        let Some(edit) = self.undo_intent() else {
            return refused("The confirmed Undo is no longer available. Check the item and override setting.");
        };
        self.writer.borrow_mut().submit(expected, &edit, true)
    }
    fn poll_write(&mut self) -> Option<EditOutcome> {
        self.writer.borrow_mut().poll()
    }
    fn clear_unique_preview(&self) {
        self.writer.borrow_mut().preview = None;
    }
    fn preview_unique(&self, item: &ForgeItem, edit: &ItemEdit) -> crate::forge::game::Lookup<Vec<ModifierState>> {
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
    descriptions: Option<(eg_trainer::edits::ExpectedObservation, u64, Pending)>,
    /// Runs only while there is work (`keep_polling`); its tick refreshes the binding.
    polling: Timer,
    /// The binding's refresh, set once the binding exists; holds it weakly.
    tick: Option<Rc<dyn Fn()>>,
}
struct PendingPreview {
    expected: eg_trainer::edits::ExpectedObservation,
    draft: eg_trainer::edits::RollEdit,
    starts: Option<std::time::Instant>,
    receipt: Option<Pending>,
    error: Option<String>,
}
struct PendingMapping {
    expected: eg_trainer::edits::ExpectedObservation,
    request: eg_trainer::mapping::MappingRequest,
    receipt: MappingRead,
    error: Option<String>,
}
struct PendingEdit {
    receipt: Pending,
    expected: ForgeItem,
    edit: ItemEdit,
    undoing: bool,
    before: BackendSnapshot,
    plan: Option<eg_trainer::writes::ForgePlan>,
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
        if !matches!(expected.snapshot().rarity, 7 | 9) {
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

    fn preview(&mut self, item: &ForgeItem, edit: &ItemEdit) -> crate::forge::game::Lookup<Vec<ModifierState>> {
        let result: Result<crate::forge::game::Lookup<Vec<ModifierState>>, GameError> = (|| {
            let draft = match edit {
                eg_trainer::edits::ItemEdit::Roll(draft) => Some(draft),
                _ => None,
            };
            let closed = self.backend.subscribe().has_changed().is_err();
            let (expected, cached) = self.backend.peek(|state| -> Result<_, GameError> {
                if closed || project_item(state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                    return Err("The item changed before its preview could be read.".into());
                }
                let expected = state.expected_observation().map_err(|e| e.to_string())?;
                let cached = state.unique_preview.as_ref().filter(|p| p.expected == expected && Some(&p.draft) == draft).map(|preview| {
                    preview
                        .result
                        .as_ref()
                        .map(|p| p.preview().modifiers.iter().map(project_modifier).collect())
                        .map_err(Clone::clone)
                });
                Ok((expected, cached))
            })?;
            let Some(draft) = draft.cloned() else {
                return Err("This preview draft is unsupported.".into());
            };
            if draft.target != eg_trainer::edits::RollTarget::Unique {
                return Err("Only unique rolls have a tooltip preview.".into());
            }
            if let Some(cached) = cached {
                return cached.map(crate::forge::game::Lookup::Ready);
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
            Ok(crate::forge::game::Lookup::Loading)
        })();
        self.keep_polling();
        result.unwrap_or_else(crate::forge::game::Lookup::Unavailable)
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

    fn resolve(&mut self, item: &ForgeItem, request: &RollRequest) -> crate::forge::game::Lookup<ValueTable> {
        let result: Result<crate::forge::game::Lookup<ValueTable>, GameError> = (|| {
            use eg_trainer::mapping::{MappingAddress, MappingRequest, MappingTarget};
            if request.target != item::RollTarget::Affix {
                return Err("Actual values for this modifier are unavailable in this build.".into());
            }
            let mapping = u8::try_from(request.index).ok().map(|index| MappingRequest {
                address: MappingAddress {
                    target: MappingTarget::Affix,
                    index,
                    line: None,
                },
                draft: request.draft.clone(),
            });
            let closed = self.backend.subscribe().has_changed().is_err();
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
                return result.map(crate::forge::game::Lookup::Ready);
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
            Ok(crate::forge::game::Lookup::Loading)
        })();
        self.keep_polling();
        result.unwrap_or_else(crate::forge::game::Lookup::Unavailable)
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
            undo.as_ref().ok_or(eg_trainer::edits::EditError::StaleItem).and_then(|undo| state.forge_undo_plan(undo))
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
    match edit {
        ItemEdit::AddAffix(eg_trainer::edits::AddAffixEdit { .. }) | ItemEdit::RemoveAffix(eg_trainer::edits::RemoveAffixEdit { .. }) => state.capabilities.affix_list,
        ItemEdit::Affix(eg_trainer::edits::AffixEdit { .. }) => state.capabilities.affix_change,
        ItemEdit::Status(eg_trainer::edits::StatusEdit { field, .. }) => state.capabilities.status_fields[*field as usize],
        ItemEdit::BaseType(eg_trainer::edits::BaseTypeEdit { .. }) => state.capabilities.base_change,
        ItemEdit::UniqueIdentity(eg_trainer::edits::UniqueIdentityEdit { .. }) => state.capabilities.identity_change,
        ItemEdit::Roll(eg_trainer::edits::RollEdit {
            target: item::RollTarget::Affix, ..
        }) => state.capabilities.affix_rolls,
        _ => false,
    }
}

struct Binding {
    backend: BackendHandle,
    window: slint::Weak<MainWindow>,
    forge: forge::ForgeHandle,
    revision: Cell<Option<u64>>,
    catalog: RefCell<(Option<Arc<GameCatalog>>, Arc<Catalog>)>,
    writer: Rc<RefCell<Writer>>,
    /// The ash under the connection's wait follows the window's decorations, which only the page showing may drive.
    shell: ShellHandle,
    decorations: Rc<Decorations<MainWindow>>,
    /// How far the wait for the game has come while the connection waits on it; None when it does not.
    waiting: Cell<Option<BloomPhase>>,
}

/// The connection as a run without the trainer's worker shows it: not connected, in the navigation's foot (only when the
/// trainer is on) and in place of every trainer page's body. A worker's binding takes both over.
pub fn unbound(window: &MainWindow, trainer: bool) {
    let connection = window.global::<Connection>();
    connection.set_present(trainer);
    connection.set_connected(false);
    connection.set_text("Not connected".into());
    connection.set_show_action(true);
    connection.set_action_text("Connect".into());
    let state = window.global::<TrainerState>();
    state.set_connection_title("Not connected".into());
    state.set_connection_detail("Connect to an offline character to use the trainer.".into());
}

/// Callback ownership keeps the binding alive; the writer's polling timer holds only a weak Rc.
/// Rebinding or dropping the window releases the UI observer and its timer.
pub fn bind(window: &MainWindow, backend: BackendHandle, shell: &ShellHandle, decorations: Rc<Decorations<MainWindow>>) {
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
    let forge = forge::attach(
        window,
        Box::new(ObservedGame {
            catalog: empty.clone(),
            writer: writer.clone(),
        }),
    );
    let binding = Rc::new(Binding {
        backend,
        window: window.as_weak(),
        forge,
        revision: Cell::new(None),
        catalog: RefCell::new((None, empty)),
        writer,
        shell: shell.clone(),
        decorations,
        waiting: Cell::new(None),
    });
    shell.on_page_entered({
        let binding = Rc::downgrade(&binding);
        move |page| {
            if let Some(binding) = binding.upgrade()
                && page.is_trainer()
            {
                binding.drift(true);
            }
        }
    });
    let owner = binding.clone();
    window.global::<Connection>().on_connect(move || {
        let phase = owner.backend.peek(|state| state.phase);
        if matches!(phase, Phase::Connecting | Phase::Closing | Phase::Ready | Phase::Stopped | Phase::Disabled) {
            return;
        }
        let command = if phase == Phase::Blocked { Command::Retry } else { Command::Connect };
        if owner.backend.try_submit(command).is_ok() {
            // --no-connect suppresses automatic startup; an explicit Connect
            // starts observation too, so connected items continue to follow reads.
            let _ = owner.backend.try_submit(Command::StartMonitoring);
            if let Some(window) = owner.window.upgrade() {
                window.global::<Connection>().set_show_action(false);
            }
        }
    });
    let weak = Rc::downgrade(&binding);
    binding.writer.borrow_mut().tick = Some(Rc::new(move || {
        if let Some(binding) = weak.upgrade() {
            binding.refresh();
        }
    }));
    let mut updates = binding.backend.subscribe();
    let weak = Rc::downgrade(&binding);
    slint::spawn_local(async move {
        loop {
            let ended = updates.changed().await.is_err();
            let Some(binding) = weak.upgrade() else { break };
            binding.refresh();
            if ended {
                break;
            }
        }
    })
    .expect("trainer binding requires a Slint event-loop proxy");
    binding.refresh();
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
        if self.backend.subscribe().has_changed().is_err() && phase != Phase::Stopped {
            // An unexpected worker failure must not leave apparently live data.
            if let Some(window) = self.window.upgrade() {
                let connection = window.global::<Connection>();
                connection.set_connected(false);
                connection.set_blocked(true);
                connection.set_show_action(false);
                connection.set_text("Trainer stopped".into());
                let state = window.global::<TrainerState>();
                state.set_connection_title("Trainer stopped".into());
                state.set_connection_detail("Restart Eterra's Gift to connect again. Home and Mods remain available.".into());
                state.set_connection_stopped(true);
                state.set_connection_waiting(false);
            }
            self.waiting.set(None);
            self.forge.observe(Box::new(ObservedGame {
                catalog: Arc::new(Catalog::default()),
                writer: self.writer.clone(),
            }));
            return;
        }
        if self.revision.get() == Some(revision) {
            self.forge.edit(|editor| {
                if mappings_completed || preview_completed {
                    editor.refresh();
                } else {
                    editor.poll_writes();
                }
            });
            return;
        }
        let snapshot = self.backend.snapshot();
        self.revision.set(Some(snapshot.revision));
        let Some(window) = self.window.upgrade() else {
            return;
        };
        window.global::<crate::SettingsState>().invoke_backend_updated();
        let connection = window.global::<Connection>();
        connection.set_connected(snapshot.phase == Phase::Ready);
        connection.set_blocked(snapshot.phase == Phase::Blocked);
        let Status {
            title,
            detail,
            nav,
            stopped,
            waiting,
        } = status(&snapshot);
        connection.set_text(nav.into());
        connection.set_show_action(
            matches!(snapshot.phase, Phase::Disconnected | Phase::Blocked)
                || ((!snapshot.auto_connect || !snapshot.monitoring) && matches!(snapshot.phase, Phase::WaitingForGame | Phase::WaitingForCharacter)),
        );
        connection.set_action_text(if snapshot.phase == Phase::Blocked { "Retry" } else { "Connect" }.into());
        let mut catalog = self.catalog.borrow_mut();
        if let Some(data) = snapshot.catalog.data()
            && catalog.0.as_ref().is_none_or(|old| !Arc::ptr_eq(old, data))
        {
            catalog.1 = Arc::new(project_catalog(data));
            catalog.0 = Some(data.clone());
        }
        let projection_error = project_item(&snapshot).err();
        self.forge.observe(Box::new(ObservedGame {
            catalog: catalog.1.clone(),
            writer: self.writer.clone(),
        }));
        let state = window.global::<TrainerState>();
        // Not connected, every trainer page shows this in place of its body; connected, the forge says only why its item
        // cannot be shown. Ash drifts during a real wait and lies still under the empty forge (ItemEditorViewModel.ShowsAsh).
        state.set_connection_title(title.into());
        state.set_connection_detail(detail.into());
        state.set_connection_stopped(stopped);
        state.set_connection_waiting(waiting.is_some());
        state.set_item_unavailable(projection_error.unwrap_or_default().into());
        self.waiting.set(waiting);
        self.drift(false);
    }

    /// Drives the window's ash while a trainer page shows and the connection waits; Home takes the decorations back when
    /// it shows.
    fn drift(&self, entering: bool) {
        if !self.shell.current().is_trainer() {
            return;
        }
        if let Some(phase) = self.waiting.get() {
            if entering {
                self.decorations.enter(phase, Ash::Drifting);
            } else {
                self.decorations.follow(phase, Ash::Drifting);
            }
        }
    }
}

/// What the app says about the connection: what every trainer page shows in place of its body while not connected, the
/// navigation foot's status line, whether the connection stopped for a reason the user has to fix, and how far a wait has
/// come (ItemEditorViewModel, TrainerConnectionViewModel).
struct Status {
    title: &'static str,
    detail: String,
    nav: String,
    stopped: bool,
    waiting: Option<BloomPhase>,
}

/// Several games run: the advice says the app connects by itself only when it will, as the C# `SessionErrorMessages`
/// `waits` (automatic connection, with the game being watched) decides.
fn multiple_games(snapshot: &BackendSnapshot) -> &'static str {
    if snapshot.monitoring && snapshot.auto_connect {
        "Close extra game instances; the app connects once one is left."
    } else {
        "Close extra game instances, then connect again."
    }
}

/// The game's log identifies online mode or a character: the connection refuses access.
fn online_refused(snapshot: &BackendSnapshot) -> bool {
    matches!(snapshot.phase, Phase::WaitingForCharacter | Phase::Blocked) && snapshot.gate.as_ref().is_some_and(|gate| gate.readiness == GameReadiness::Online)
}

/// Advice follows proven residency; an unloaded helper needs no restart.
fn online_advice(snapshot: &BackendSnapshot) -> &'static str {
    use eg_trainer::backend::{HELPER_RESTART_REQUIRED, HelperResidency};
    match snapshot.retained_helper.as_ref().map(|helper| helper.residency) {
        Some(HelperResidency::Stuck | HelperResidency::MayBeLoaded | HelperResidency::Loaded) => HELPER_RESTART_REQUIRED,
        Some(HelperResidency::Leaving) => "The helper is leaving Last Epoch.",
        Some(HelperResidency::Gone) | None => "Exit online mode and load an offline character to use the trainer.",
    }
}

/// The preparation's progress while connecting: the game files, when a changed install is read again.
fn progress(snapshot: &BackendSnapshot) -> Option<String> {
    (snapshot.activity == Some(Activity::ReadingGameFiles)).then(|| "Reading game files…".to_owned())
}

fn status(snapshot: &BackendSnapshot) -> Status {
    let line = |title: &'static str, detail: &str, nav: &str| Status {
        title,
        detail: detail.to_owned(),
        nav: nav.to_owned(),
        stopped: false,
        waiting: None,
    };
    if online_refused(snapshot) {
        let advice = online_advice(snapshot);
        return Status {
            stopped: true,
            ..line("Online mode", advice, &format!("Online mode. {advice}"))
        };
    }
    let writing = matches!(snapshot.activity, Some(Activity::WritingForge | Activity::WritingAttackLeech));
    match snapshot.phase {
        // Connected, the pages show their own bodies; only the navigation's line is said.
        Phase::Ready => line("Connected", "", if writing { "Connected · applying…" } else { "Connected" }),
        Phase::WaitingForGame if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::MultipleProcesses) => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line("Waiting for Last Epoch", multiple_games(snapshot), multiple_games(snapshot))
        },
        Phase::WaitingForGame => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line("Waiting for Last Epoch", "Start Last Epoch and load an offline character.", "Waiting for game")
        },
        Phase::WaitingForCharacter if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::CharacterLoading) => Status {
            waiting: Some(BloomPhase::Opening),
            ..line("Changing area", "Waiting for the offline character to finish changing area.", "Changing area…")
        },
        Phase::WaitingForCharacter => Status {
            waiting: Some(BloomPhase::Opening),
            ..line(
                "Waiting for an offline character",
                "Load an offline character to continue.",
                "Waiting for offline character",
            )
        },
        Phase::Connecting => {
            let progress = progress(snapshot);
            Status {
                waiting: Some(BloomPhase::Budding),
                ..line(
                    "Connecting",
                    progress.as_deref().unwrap_or("Connecting to the game…"),
                    progress.as_deref().unwrap_or("Preparing…"),
                )
            }
        }
        // A write that may have reached the game stops the connection: its worker reason is in the fault log, and the page
        // says what it means and what to do (Retry is the page's one button).
        Phase::Blocked if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::WriteUncertain) => Status {
            stopped: true,
            ..line(
                "Edit not confirmed",
                "The edit may have reached the game, but it could not be confirmed. Check the item in the game, then Retry to read it again. \
                 The edit will not be sent again; drafts and Undo were cleared, and details are in the fault log.",
                "Edit not confirmed. Check the item in the game, then Retry.",
            )
        },
        Phase::Blocked => {
            let message = snapshot.error.as_ref().map_or_else(
                || "The trainer could not connect. Retry when the game is ready.".to_owned(),
                |error| sentence(&error.message),
            );
            Status {
                stopped: true,
                ..line("Connection stopped", &message, &message)
            }
        }
        Phase::Closing => line("Disconnecting…", "Finishing requests and turning off trainer effects.", "Disconnecting…"),
        Phase::Disabled => line("Trainer is off", "Enable the trainer in Settings and restart Eterra's Gift.", "Trainer is off"),
        Phase::Stopped => Status {
            stopped: true,
            ..line("Trainer stopped", "Restart Eterra's Gift to connect again.", "Trainer stopped")
        },
        Phase::Disconnected => line("Not connected", "Connect to an offline character to use the trainer.", "Not connected"),
    }
}

fn category(value: i32) -> Result<Category, String> {
    match value {
        0 => Ok(Category::Standard),
        1 => Ok(Category::Experimental),
        2 => Ok(Category::Personal),
        3 => Ok(Category::Set),
        6 => Ok(Category::Corrupted),
        _ => Err("This item's affix category is not supported by the editor yet.".into()),
    }
}
fn kind(value: i32) -> Result<AffixKind, String> {
    match value {
        0 => Ok(AffixKind::Prefix),
        1 => Ok(AffixKind::Suffix),
        _ => Err("This item's affix type is not supported by the editor yet.".into()),
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
    let pair = |range: eg_trainer::catalog::ValueRange| (range.minimum, range.maximum);
    catalog.limits = Some(forge::game::ItemLimits {
        forging_potential: pair(data.limits.forging_potential),
        legendary_potential: pair(data.limits.legendary_potential),
        weavers_will: pair(data.limits.weavers_will),
        weavers_touch: pair(data.limits.weavers_touch),
        level_requirement: pair(data.limits.level_requirement),
    });
    catalog.item_types = data
        .bases
        .values()
        .map(|definition| forge::game::ItemTypeDefinition {
            item_type: u16::from(definition.item_type),
            name: definition.name.text.clone(),
            bases: definition
                .sub_types
                .iter()
                .filter_map(|base| {
                    Some(forge::game::BaseDefinition {
                        sub_type: base.sub_type,
                        name: base.name.text.clone(),
                        level: u8::try_from(base.level_requirement).ok()?,
                        corrupted: base.is_corrupted,
                        class_requirement: base.class_requirement,
                        // C# `ForgeChoices.BaseOptions` lists each implicit with `ImplicitLine`; an undescribed list is none.
                        implicits: match &base.implicits {
                            Availability::Available(implicits) => implicits
                                .iter()
                                .map(|implicit| forge::game::implicit_line(available_text(&implicit.minimum_text), available_text(&implicit.maximum_text)))
                                .collect(),
                            Availability::Unavailable { .. } => Vec::new(),
                        },
                    })
                })
                .collect(),
        })
        .collect();
    catalog.uniques = data
        .uniques
        .values()
        .filter_map(|definition| {
            Some(forge::game::UniqueDefinition {
                id: definition.id,
                name: definition.name.text.clone(),
                item_type: u16::from(definition.item_type),
                sub_types: definition.sub_types.iter().map(|id| u16::from(*id)).collect(),
                legendary_type: match definition.legendary_type {
                    0 => forge::game::LegendaryType::LegendaryPotential,
                    1 => forge::game::LegendaryType::WeaversWill,
                    _ => return None,
                },
                is_set: definition.is_set_item,
                can_drop_as_legendary: definition.can_drop_as_legendary,
            })
        })
        .collect();
    for definition in data.affixes.values() {
        if let (Ok(kind), Ok(category), Ok(max_tier)) = (kind(definition.kind), category(definition.special), u8::try_from(definition.tier_count)) {
            let name = match &definition.game_display_name {
                Availability::Available(text) if !text.is_empty() => text.clone(),
                _ if !definition.raw_display_name.is_empty() => definition.raw_display_name.clone(),
                _ => definition.raw_name.clone(),
            };
            catalog.definitions.push(AffixDefinition {
                id: definition.id,
                name,
                kind,
                category,
                item_types: definition
                    .item_types
                    .as_ref()
                    .map(|types| types.iter().filter_map(|t| u16::try_from(*t).ok()).collect())
                    .unwrap_or_default(),
                max_tier,
                verified: definition.id <= 4095 && (1..=8).contains(&max_tier) && definition.item_types.as_ref().is_some_and(|types| types.iter().all(|t| (0..=41).contains(t))),
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
fn value_table(table: &eg_trainer::mapping::ValueTable) -> ValueTable {
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
fn project_modifier(line: &eg_trainer::backend::ModifierSnapshot) -> item::ModifierState {
    item::ModifierState {
        index: line.index as usize,
        text: line.text.clone(),
        range: line.range.clone(),
        roll_ids: line.roll_ids.iter().map(|id| usize::from(*id)).collect(),
        editable: line.editable,
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
    // Said in the item's own terms: the generic status refusal doesn't tell the user which value is out of reach.
    if u8::try_from(snapshot.status.level_requirement).is_err() {
        return Err("This item's level requirement is outside the editor's supported range.".into());
    }
    let mut item = ForgeItem::observed(context, name).map_err(|error| error.to_string())?;
    item.implicits = implicits.as_ref().map(|lines| lines.iter().map(project_modifier).collect());
    if let Some(identity) = item.unique.as_mut().or(item.set_item.as_mut()) {
        identity.modifiers = unique
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|lines| lines.iter().map(project_modifier).collect())
            .unwrap_or_default();
        identity.formatting = match unique {
            Some(Ok(_)) => crate::forge::game::Lookup::Ready(()),
            Some(Err(reason)) => crate::forge::game::Lookup::Unavailable(reason.clone()),
            None => crate::forge::game::Lookup::Loading,
        };
    }
    Ok(Some(item))
}
