//! Command-based UI adapter. The worker owns native access; this module observes owned
//! snapshots and queues commands. No synchronous game/pipe/process work runs here.
use crate::{
    Connection, MainWindow, TrainerState,
    forge::{
        self,
        game::{Catalog, EditOutcome, ForgeGame, GameError, WriteOutcome, refused},
        item::{self, AffixDefinition, AffixKind, AffixState, Category, ForgeItem, ItemEdit, ModifierState, RollRequest, Seal, ValueTable},
    },
};
use eg_trainer::{
    backend::{Activity, BackendHandle, BackendSnapshot, CatalogState, Command, ForgeState, Pending, Phase},
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
        project_item(&writer.backend.snapshot()).ok().flatten()
    }
    fn can_write(&self) -> bool {
        let writer = self.writer.borrow();
        let state = writer.backend.snapshot();
        writer.backend.subscribe().has_changed().is_ok() && state.phase == Phase::Ready && state.activity.is_none() && writer.pending.is_none()
    }
    fn unrestricted(&self) -> bool {
        self.writer.borrow().backend.snapshot().override_enabled
    }
    fn is_busy(&self) -> bool {
        let state = self.writer.borrow().backend.snapshot();
        self.writer.borrow().backend.subscribe().has_changed().is_ok() && state.phase == Phase::Ready && state.activity.is_some()
    }
    fn supports(&self, edit: &ItemEdit) -> bool {
        let writer = self.writer.borrow();
        writer.backend.subscribe().has_changed().is_ok() && supports(&writer.backend.snapshot(), edit)
    }
    fn read_only_reason(&self) -> Option<&str> {
        let state = self.writer.borrow().backend.snapshot();
        (!state.capabilities.status_fields.iter().any(|s| *s)
            && !state.capabilities.base_change
            && !state.capabilities.identity_change
            && !state.capabilities.affix_rolls
            && !state.capabilities.affix_change
            && !state.capabilities.affix_list)
            .then_some(READ_ONLY)
    }
    fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> crate::forge::game::Lookup<ValueTable> {
        self.writer.borrow_mut().resolve(item, request)
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
        self.writer.borrow().backend.snapshot().undo_forge_depth
    }
    fn undo_needs_override(&self) -> bool {
        self.writer.borrow().backend.snapshot().undo_forge_needs_override
    }
    fn undo_intent(&self) -> Option<ItemEdit> {
        let state = self.writer.borrow().backend.snapshot();
        state.forge_undo_plan(state.undo_forge.as_ref()?).ok().map(|plan| plan.edit_intent().clone())
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
    receipt: Pending,
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
    fn poll_descriptions(&mut self) {
        let state = self.backend.snapshot();
        let Ok(expected) = state.expected_observation() else {
            self.descriptions = None;
            return;
        };
        if !matches!(expected.snapshot().rarity, 7 | 9) {
            self.descriptions = None;
            return;
        }
        if !matches!(&state.forge, ForgeState::Item { unique: None, .. }) {
            return;
        }
        if self
            .descriptions
            .as_ref()
            .is_none_or(|(before, sequence, _)| before != &expected || *sequence != state.forge_read_sequence)
            && let Ok(receipt) = self.backend.try_submit(Command::DescribeUnique {
                expected: Box::new(expected.clone()),
            })
        {
            self.descriptions = Some((expected, state.forge_read_sequence, receipt));
        }
    }

    fn preview(&mut self, item: &ForgeItem, edit: &ItemEdit) -> crate::forge::game::Lookup<Vec<ModifierState>> {
        let result: Result<crate::forge::game::Lookup<Vec<ModifierState>>, GameError> = (|| {
            let state = self.backend.snapshot();
            if self.backend.subscribe().has_changed().is_err() || project_item(&state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                return Err("The item changed before its preview could be read.".into());
            }
            let expected = state.expected_observation().map_err(|e| e.to_string())?;
            let eg_trainer::edits::ItemEdit::Roll(draft) = edit.clone() else {
                return Err("This preview draft is unsupported.".into());
            };
            if draft.target != eg_trainer::edits::RollTarget::Unique {
                return Err("Only unique rolls have a tooltip preview.".into());
            }
            if let Some(preview) = state.unique_preview.as_ref().filter(|p| p.expected == expected && p.draft == draft) {
                return preview
                    .result
                    .as_ref()
                    .map(|p| p.preview().modifiers.iter().map(project_modifier).collect())
                    .map_err(Clone::clone)
                    .map(crate::forge::game::Lookup::Ready);
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
        result.unwrap_or_else(crate::forge::game::Lookup::Unavailable)
    }
    fn poll_preview(&mut self) -> bool {
        if self
            .preview
            .as_ref()
            .is_some_and(|p| self.backend.snapshot().expected_observation().as_ref() != Ok(&p.expected))
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
            let state = self.backend.snapshot();
            if self.backend.subscribe().has_changed().is_err() || project_item(&state).ok().flatten().as_ref().is_none_or(|actual| !actual.same_state(item)) {
                return Err("The item changed before its values could be read.".into());
            }
            self.poll_mappings();
            if self.mapping_read_sequence != Some(state.forge_read_sequence) {
                // A fresh owned read permits another optional read attempt. Merely
                // republishing a mapping result must never create a retry loop.
                self.mapping_read_sequence = Some(state.forge_read_sequence);
                self.mappings.retain(|pending| pending.error.is_none());
            }
            let expected = state.expected_observation().map_err(|error| error.to_string())?;
            let request = MappingRequest {
                address: MappingAddress {
                    target: MappingTarget::Affix,
                    index: u8::try_from(request.index).map_err(|_| "This affix slot is unsupported.")?,
                    line: None,
                },
                draft: request.draft.clone(),
            };
            if let Some(result) = state.mapping(&expected, &request) {
                return result
                    .as_ref()
                    .map(|table| ValueTable {
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
                    })
                    .map_err(Clone::clone)
                    .map(crate::forge::game::Lookup::Ready);
            }
            self.mappings.retain(|pending| pending.expected == expected);
            if let Some(error) = self.mappings.iter().find(|pending| pending.request == request).and_then(|pending| pending.error.clone()) {
                return Err(error);
            }
            if !self.mappings.iter().any(|pending| pending.expected == expected && pending.request == request) && self.pending.is_none() {
                let receipt = self
                    .backend
                    .try_submit(Command::ResolveMapping {
                        expected: Box::new(expected.clone()),
                        request: request.clone(),
                    })
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
        result.unwrap_or_else(crate::forge::game::Lookup::Unavailable)
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
    timer: Timer,
    writer: Rc<RefCell<Writer>>,
}

/// Callback ownership keeps the binding alive; the timer holds only a weak Rc.
/// Rebinding or dropping the window releases the UI observer and its timer.
pub fn bind(window: &MainWindow, backend: BackendHandle) {
    let empty = Arc::new(Catalog::default());
    let writer = Rc::new(RefCell::new(Writer {
        backend: backend.clone(),
        pending: None,
        mappings: Vec::new(),
        mapping_read_sequence: None,
        preview: None,
        descriptions: None,
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
        timer: Timer::default(),
        writer,
    });
    let owner = binding.clone();
    window.global::<Connection>().on_connect(move || {
        let snapshot = owner.backend.snapshot();
        if matches!(snapshot.phase, Phase::Connecting | Phase::Closing | Phase::Ready | Phase::Stopped | Phase::Disabled) {
            return;
        }
        let command = if snapshot.phase == Phase::Blocked { Command::Retry } else { Command::Connect };
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
    binding.timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        if let Some(binding) = weak.upgrade()
            && binding.writer.borrow().has_pending_work()
        {
            binding.refresh();
        }
    });
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
    fn refresh(&self) {
        self.writer.borrow_mut().poll_descriptions();
        let mappings_completed = self.writer.borrow_mut().poll_mappings();
        let preview_completed = self.writer.borrow_mut().poll_preview();
        let snapshot = self.backend.snapshot();
        if self.backend.subscribe().has_changed().is_err() && snapshot.phase != Phase::Stopped {
            // An unexpected worker failure must not leave apparently live data.
            if let Some(window) = self.window.upgrade() {
                let connection = window.global::<Connection>();
                connection.set_connected(false);
                connection.set_blocked(true);
                connection.set_show_action(false);
                connection.set_text("Trainer stopped".into());
                let state = window.global::<TrainerState>();
                state.set_empty_title("Trainer stopped".into());
                state.set_empty_detail("Restart Eterra's Gift to connect again. Home and Mods remain available.".into());
            }
            self.forge.observe(Box::new(ObservedGame {
                catalog: Arc::new(Catalog::default()),
                writer: self.writer.clone(),
            }));
            return;
        }
        if self.revision.get() == Some(snapshot.revision) {
            self.forge.edit(|editor| {
                if mappings_completed || preview_completed {
                    editor.refresh();
                } else {
                    editor.poll_writes();
                }
            });
            return;
        }
        self.revision.set(Some(snapshot.revision));
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let connection = window.global::<Connection>();
        connection.set_connected(snapshot.phase == Phase::Ready);
        connection.set_blocked(snapshot.phase == Phase::Blocked);
        let (title, detail) = status(&snapshot);
        connection.set_text(if snapshot.phase == Phase::Ready { "Connected" } else { title }.into());
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
        let item = project_item(&snapshot);
        let projection_error = item.as_ref().err().cloned();
        self.forge.observe(Box::new(ObservedGame {
            catalog: catalog.1.clone(),
            writer: self.writer.clone(),
        }));
        let state = window.global::<TrainerState>();
        state.set_empty_title(if projection_error.is_some() { "Item details unavailable" } else { title }.into());
        state.set_empty_detail(projection_error.unwrap_or(detail).into());
    }
}

fn status(snapshot: &BackendSnapshot) -> (&'static str, String) {
    match snapshot.phase {
        Phase::Ready => ("Forge is empty", "Place an item in the in-game forge to inspect it.".into()),
        Phase::WaitingForGame => ("Waiting for Last Epoch", "Start Last Epoch and load an offline character.".into()),
        Phase::WaitingForCharacter => (
            "Waiting for an offline character",
            "Load an offline character to use the trainer. Online characters are refused.".into(),
        ),
        Phase::Connecting => {
            if snapshot.activity == Some(Activity::CapturingCatalog) {
                ("Reading game definitions…", "Reading the game's definitions before showing the item.".into())
            } else {
                ("Connecting…", "Preparing the trainer for this offline game.".into())
            }
        }
        Phase::Blocked => (
            "Cannot connect",
            snapshot
                .error
                .as_ref()
                .map_or("The trainer could not connect. Retry when the game is ready.".into(), |e| e.message.clone()),
        ),
        Phase::Closing => ("Disconnecting…", "Finishing requests and turning off trainer effects.".into()),
        Phase::Disabled => ("Trainer is off", "Enable the trainer in Settings and restart Eterra's Gift.".into()),
        Phase::Stopped => ("Trainer stopped", "Restart Eterra's Gift to connect again.".into()),
        Phase::Disconnected => ("Not connected", "Connect, then place an item in the forge.".into()),
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
                        implicits: vec![],
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
    let Some(target) = &state.attached else {
        return Ok(None);
    };
    let affixes = snapshot
        .affixes
        .iter()
        .map(|a| {
            Ok(AffixState {
                slot: usize::from(a.slot),
                id: a.id,
                tier: u8::try_from(a.tier).map_err(|_| "This item's tier is outside the editor's supported range.")?,
                roll: a.roll,
                kind: kind(a.kind)?,
                category: category(a.special)?,
                seal: match a.seal {
                    0 => Seal::Normal,
                    1 => Seal::Sealed,
                    2 => Seal::Primordial,
                    3 => Seal::Corrupted,
                    _ => return Err("This item's seal type is not supported by the editor yet.".into()),
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let identity = snapshot.identity.as_ref().map(|i| item::UniqueState {
        id: i.id,
        rolls: i.rolls.clone(),
        modifiers: unique
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|lines| lines.iter().map(project_modifier).collect())
            .unwrap_or_default(),
        formatting: match unique {
            Some(Ok(_)) => crate::forge::game::Lookup::Ready(()),
            Some(Err(reason)) => crate::forge::game::Lookup::Unavailable(reason.clone()),
            None => crate::forge::game::Lookup::Loading,
        },
    });
    let level_requirement = u8::try_from(snapshot.status.level_requirement).map_err(|_| "This item's level requirement is outside the editor's supported range.")?;
    let status = Some(item::ItemStatus {
        forging_potential: snapshot.status.forging_potential,
        legendary_potential: snapshot.status.legendary_potential,
        weavers_will: snapshot.status.weavers_will,
        weavers_touch: snapshot.status.weavers_touch,
        level_requirement,
        corrupted: snapshot.status.corrupted,
        ruined: snapshot.status.ruined,
    });
    Ok(Some(ForgeItem {
        context: Some(Arc::new(item::ForgeContext {
            expected: state.expected_observation().map_err(|e| e.to_string())?,
            catalog: state.catalog.data().cloned().ok_or("Catalog unavailable.")?,
        })),
        token: format!("{}:{}:{}:{}:{}", target.pid, target.created, state.generation, snapshot.item, snapshot.individual_id),
        name: resolved_name
            .as_ref()
            .map_or_else(|| if snapshot.name.is_empty() { "Unnamed item".into() } else { snapshot.name.clone() }, |n| n.text.clone()),
        item_type: u16::from(snapshot.item_type),
        sub_type: snapshot.sub_type,
        idol: (25..=33).contains(&snapshot.item_type),
        rarity: snapshot.rarity,
        affixes,
        unique: if snapshot.rarity == 8 { None } else { identity.clone() },
        set_item: if snapshot.rarity == 8 { identity } else { None },
        implicits: implicits.as_ref().map(|lines| {
            lines
                .iter()
                .map(|line| item::ModifierState {
                    index: line.index as usize,
                    text: line.text.clone(),
                    range: line.range.clone(),
                    roll_ids: line.roll_ids.iter().map(|id| usize::from(*id)).collect(),
                    editable: line.editable,
                })
                .collect()
        }),
        implicit_rolls: snapshot.implicit_rolls.clone(),
        status,
    }))
}
