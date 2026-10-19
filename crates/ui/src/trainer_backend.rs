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
    fn compatibility(&self, item: &ForgeItem, definition: &AffixDefinition, slot: Option<&AffixState>, unrestricted: bool) -> item::Compatibility {
        use eg_trainer::edits::{CompatibilityReason, compatibility};
        use item::{Compatibility, Rejection};
        let state = self.writer.borrow().backend.snapshot();
        let Some(data) = state.catalog.data() else {
            return Compatibility::Unknown;
        };
        let Some(definition) = data.affixes.get(&definition.id) else {
            return Compatibility::Unknown;
        };
        let ForgeState::Item { snapshot, .. } = &state.forge else {
            return Compatibility::Unknown;
        };
        if project_item(&state).ok().flatten().is_none_or(|actual| actual.token != item.token) {
            return Compatibility::Unknown;
        }
        // Earlier draft steps may already move/replace affixes in this local
        // projection. Keep the captured class context and use the domain rules.
        let mut projected = (**snapshot).clone();
        let Ok(item_type) = u8::try_from(item.item_type) else {
            return Compatibility::Unknown;
        };
        projected.item_type = item_type;
        projected.sub_type = item.sub_type;
        projected.rarity = item.rarity;
        projected.affixes.clear();
        for affix in &item.affixes {
            let Ok(slot) = u8::try_from(affix.slot) else {
                return Compatibility::Unknown;
            };
            projected.affixes.push(eg_trainer::backend::AffixSnapshot {
                slot,
                id: affix.id,
                tier: u16::from(affix.tier),
                roll: affix.roll,
                kind: match affix.kind {
                    AffixKind::Prefix => 0,
                    AffixKind::Suffix => 1,
                },
                special: match affix.category {
                    Category::Standard => 0,
                    Category::Experimental => 1,
                    Category::Personal => 2,
                    Category::Set => 3,
                    Category::Corrupted => 6,
                },
                seal: affix.seal.index() as i32,
                name: String::new(),
            });
        }
        let slot = match slot {
            Some(slot) => match projected.affixes.iter().find(|a| usize::from(a.slot) == slot.slot) {
                Some(a) => Some(a),
                None => return Compatibility::Unknown,
            },
            None => None,
        };
        match compatibility(data, &projected, definition, slot, None, unrestricted).reason {
            CompatibilityReason::None => Compatibility::Allowed,
            CompatibilityReason::Override => Compatibility::Override,
            CompatibilityReason::ClassRequirement => Compatibility::Rejected(Rejection::ClassRequirement),
            CompatibilityReason::SlotType => Compatibility::Rejected(Rejection::SlotType),
            CompatibilityReason::EquipmentType => Compatibility::Rejected(Rejection::EquipmentType),
            CompatibilityReason::SpecialCategory => Compatibility::Rejected(Rejection::SpecialCategory),
            CompatibilityReason::AlreadyPresent => Compatibility::Rejected(Rejection::AlreadyPresent),
            CompatibilityReason::MissingMetadata | CompatibilityReason::UnknownGameValue => Compatibility::Unknown,
        }
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
            && !state.capabilities.affix_change)
            .then_some(READ_ONLY)
    }
    fn resolve(&self, _: &ForgeItem, _: &RollRequest) -> Result<ValueTable, GameError> {
        Err("Actual values are unavailable until game formatting is connected.".into())
    }
    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        self.writer.borrow_mut().submit(expected, edit)
    }
    fn poll_write(&mut self) -> Option<EditOutcome> {
        self.writer.borrow_mut().poll()
    }
    fn preview_unique(&self, _: &ForgeItem, _: &ItemEdit) -> Result<Vec<ModifierState>, GameError> {
        Err("Localized descriptions are unavailable.".into())
    }
}

// A receipt survives replacement of the owned observation provider. The UI never
// blocks on it, and never resubmits an accepted command after losing its reply.
struct Writer {
    backend: BackendHandle,
    pending: Option<PendingEdit>,
}
struct PendingEdit {
    receipt: Pending,
    expected: ForgeItem,
    edit: ItemEdit,
    before: BackendSnapshot,
    plan: Option<eg_trainer::writes::ForgePlan>,
}
impl Writer {
    fn submit(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        if self.pending.is_some() {
            return refused("An item edit is already pending.");
        }
        let state = self.backend.snapshot();
        if !supports(&state, edit) {
            return refused("This edit is unavailable in this build.");
        }
        if project_item(&state).ok().flatten().as_ref() != Some(expected) {
            return refused("The item changed. Discard the stale draft and edit it again.");
        }
        match self.backend.try_submit(Command::ReadForge) {
            Ok(receipt) => {
                self.pending = Some(PendingEdit {
                    receipt,
                    expected: expected.clone(),
                    edit: edit.clone(),
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
            || state.forge != pending.before.forge
            || project_item(&state).ok().flatten().as_ref() != Some(&pending.expected)
        {
            return Some(refused("The item changed before submission. Discard the stale draft and edit it again."));
        }
        let intent = match canonical_edit(&pending.edit, state.override_enabled) {
            Some(edit) => edit,
            None => return Some(refused("This edit is unavailable in this build.")),
        };
        let plan = match state.forge_plan(intent) {
            Ok(plan) => plan,
            Err(error) => return Some(refused(error.to_string())),
        };
        match self.backend.try_submit(Command::Write(plan.clone().into())) {
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
        ItemEdit::Affix { .. } => state.capabilities.affix_change,
        ItemEdit::Status { field, .. } => state.capabilities.status_fields[*field as usize],
        ItemEdit::Base { .. } => state.capabilities.base_change,
        ItemEdit::Identity { .. } => state.capabilities.identity_change,
        ItemEdit::Rolls {
            target: item::RollTarget::Affix, ..
        } => state.capabilities.affix_rolls,
        _ => false,
    }
}
fn canonical_edit(edit: &ItemEdit, override_enabled: bool) -> Option<eg_trainer::edits::ItemEdit> {
    use eg_trainer::edits as native;
    Some(match edit {
        ItemEdit::Affix {
            slot,
            id,
            tier,
            roll,
            seal,
            allow_incompatible,
        } => native::ItemEdit::Affix(native::AffixEdit {
            slot: i32::try_from(*slot).ok()?,
            id: i32::from(*id),
            tier: i32::from(*tier),
            roll: i32::from(*roll),
            seal: match seal {
                item::Seal::Normal => native::SealState::Normal,
                item::Seal::Sealed => native::SealState::Sealed,
                item::Seal::Primordial => native::SealState::Primordial,
                item::Seal::Corrupted => native::SealState::Corrupted,
            },
            allow_incompatible: *allow_incompatible,
        }),
        ItemEdit::Status { field, value, unrestricted } => native::ItemEdit::Status(native::StatusEdit {
            field: *field,
            value: i32::from(*value),
            unrestricted: *unrestricted,
        }),
        ItemEdit::Base { sub_type, allow_incompatible } => native::ItemEdit::BaseType(native::BaseTypeEdit {
            sub_type: i32::from(*sub_type),
            allow_incompatible: *allow_incompatible,
            item_type: None,
        }),
        ItemEdit::Identity {
            unique_id,
            sub_type,
            rarity,
            unrestricted,
        } => native::ItemEdit::UniqueIdentity(native::UniqueIdentityEdit {
            unique_id: i32::from(*unique_id),
            sub_type: i32::from(*sub_type),
            rarity: i32::from(*rarity),
            unrestricted: *unrestricted,
        }),
        ItemEdit::Rolls {
            target: item::RollTarget::Affix,
            changes,
        } => native::ItemEdit::Roll(native::RollEdit {
            target: native::RollTarget::Affix,
            changes: changes
                .iter()
                .map(|(index, value)| {
                    Some(native::RollChange {
                        index: i32::try_from(*index).ok()?,
                        value: i32::from(*value),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            allow_incompatible: override_enabled,
        }),
        _ => return None,
    })
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
        if let Some(binding) = weak.upgrade() {
            binding.refresh();
        }
    });
    binding.refresh();
}

impl Binding {
    fn refresh(&self) {
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
            self.forge.edit(|editor| editor.poll_writes());
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
fn project_item(state: &BackendSnapshot) -> Result<Option<ForgeItem>, String> {
    let ForgeState::Item {
        generation,
        snapshot,
        resolved_name,
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
        modifiers: vec![],
        formatting_error: "Localized descriptions are unavailable in this build.".into(),
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
        implicits: None,
        implicit_rolls: snapshot.implicit_rolls.clone(),
        status,
    }))
}
