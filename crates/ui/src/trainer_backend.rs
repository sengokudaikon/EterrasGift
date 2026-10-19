//! Read-only UI adapter. The worker owns native access; this module observes owned
//! snapshots and queues commands. No synchronous game/pipe/process work runs here.
use crate::{
    Connection, MainWindow, TrainerState,
    forge::{
        self,
        game::{Catalog, ForgeGame, GameError},
        item::{self, AffixDefinition, AffixKind, AffixState, Category, ForgeItem, ItemEdit, ModifierState, RollRequest, Seal, ValueTable},
    },
};
use eg_trainer::{
    backend::{Activity, BackendHandle, BackendSnapshot, CatalogState, Command, ForgeState, Phase},
    catalog::{Availability, GameCatalog},
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

const READ_ONLY: &str = "Read-only. Editing and formatted values are not available yet.";

struct ObservedGame {
    catalog: Arc<Catalog>,
    item: Option<ForgeItem>,
}
impl ForgeGame for ObservedGame {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    fn read(&self) -> Option<ForgeItem> {
        self.item.clone()
    }
    fn can_write(&self) -> bool {
        false
    }
    fn read_only_reason(&self) -> Option<&str> {
        Some(READ_ONLY)
    }
    fn resolve(&self, _: &ForgeItem, _: &RollRequest) -> Result<ValueTable, GameError> {
        Err("Actual values are unavailable until game formatting is connected.".into())
    }
    fn write(&mut self, _: &ForgeItem, _: &ItemEdit) -> Result<ForgeItem, GameError> {
        Err(READ_ONLY.into())
    }
    fn preview_unique(&self, _: &ForgeItem, _: &ItemEdit) -> Result<Vec<ModifierState>, GameError> {
        Err("Localized descriptions are unavailable.".into())
    }
}

struct Binding {
    backend: BackendHandle,
    window: slint::Weak<MainWindow>,
    forge: forge::ForgeHandle,
    revision: Cell<Option<u64>>,
    catalog: RefCell<(Option<Arc<GameCatalog>>, Arc<Catalog>)>,
    timer: Timer,
}

/// Callback ownership keeps the binding alive; the timer holds only a weak Rc.
/// Rebinding or dropping the window releases the UI observer and its timer.
pub fn bind(window: &MainWindow, backend: BackendHandle) {
    let empty = Arc::new(Catalog::default());
    let forge = forge::attach(
        window,
        Box::new(ObservedGame {
            catalog: empty.clone(),
            item: None,
        }),
    );
    let binding = Rc::new(Binding {
        backend,
        window: window.as_weak(),
        forge,
        revision: Cell::new(None),
        catalog: RefCell::new((None, empty)),
        timer: Timer::default(),
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
            self.forge.observe(Box::new(ObservedGame {
                catalog: Arc::new(Catalog::default()),
                item: None,
            }));
            let _ = slint::quit_event_loop();
            return;
        }
        if self.revision.get() == Some(snapshot.revision) {
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
        connection.set_text(if snapshot.phase == Phase::Ready { "Connected · Read-only" } else { title }.into());
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
            item: item.ok().flatten(),
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
                item_types: vec![],
                max_tier,
                verified: false,
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
