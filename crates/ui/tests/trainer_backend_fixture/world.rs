//! The fixture world behind the Item Forge's `item-*` captures, on the real worker: the boots, unique boots and rings of
//! the C# app's `FakeGameSession` and `ShowcaseSession` (`spec/forge-fixture.json`). Synthetic, not game data. The fake
//! connection reads a write back as its plan's projection, formats the unique's lines from its rolls and answers value
//! tables from the world's ranges; drafts, Apply and Undo are the app's own.
use super::*;
use eg_trainer::backend::{AffixSnapshot, ModifierSnapshot, UniqueIdentity};
use eg_trainer::definitions::catalog::RarityDefinition;
use eg_trainer::item::edit::{ExpectedObservation, ItemEdit, RollEdit};
use eg_trainer::item::values::{
    BoundPreview, BoundTooltip, DisplayScalarParser, MappingBuilder, MappingRequest, MappingStart, MappingTarget, RawMappingPage, RawValueSample, UniqueRollPreview, ValueTable,
};
use eg_trainer::write::{WriteControl, WriteObservation, WriteOutcome, WritePlan};
use eg_ui::pages::forge::{
    ForgeHandle,
    editor::{Cell, ForgeEditor},
    item::Seal,
    rows::LOADING,
};
use eg_ui::{ForgeState, Nav, Tip};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{LogicalPosition, Model};
use std::collections::BTreeMap;

const BOOTS: u16 = 3;
const RING: u16 = 20;
const AMULET: u16 = 21;
const PREFIX: i32 = 0;
const SUFFIX: i32 = 1;
const STANDARD: i32 = 0;
const SET: i32 = 3;
const CORRUPTED: i32 = 6;

/// A game the forge can hold: its definitions, its item and how it answers.
#[derive(Clone)]
struct World {
    catalog: GameCatalog,
    item: ForgeSnapshot,
    /// Every table is value = roll, as the C# `FakeGameSession`'s.
    identity: bool,
    /// Affixes whose value tables the game cannot read.
    unmapped: Vec<u16>,
    /// The unique's lines as written, before the game formats them from its rolls.
    unique_lines: Vec<ModifierSnapshot>,
    /// The implicits as the game describes them; None when it does not.
    implicit_lines: Option<Vec<ModifierSnapshot>>,
}

fn catalog(base_implicits: bool, class_affixes: bool) -> GameCatalog {
    let affix = |id: u16, text: &str, kind: i32, special: i32, item_types: &[u16], class_requirement: i32| {
        (
            id,
            AffixDefinition {
                id,
                raw_name: text.into(),
                raw_display_name: text.into(),
                game_display_name: Availability::Available(text.into()),
                kind,
                special,
                class_requirement,
                item_types: Some(item_types.iter().map(|t| i32::from(*t)).collect()),
                tier_count: 8,
                standard_effect: 1.0,
                // Flat ranges that grow with the tier, so the tier menu and the picker show values.
                numeric: Availability::Available(
                    (1..=8)
                        .map(|tier: i32| eg_trainer::definitions::catalog::AffixTier {
                            properties: vec![eg_trainer::definitions::catalog::NumericProperty {
                                property: 0,
                                tags: 0,
                                special_tag: 0,
                                extra_tag: 0,
                                modifier_type: 0,
                                set_property: false,
                                minimum: (tier * 10 - 5) as f32,
                                maximum: (tier * 10 + 4) as f32,
                                roll_id: -1,
                                rounding: 1,
                                display_added_as_percentage: false,
                                display_added_as_tenth: false,
                                less_is_better: false,
                            }],
                        })
                        .collect(),
                ),
            },
        )
    };
    let mut affixes: BTreeMap<u16, AffixDefinition> = [
        affix(25, "Health", SUFFIX, STANDARD, &[BOOTS, RING], 0),
        affix(1148, "Caretaker's Regrowth Reforged", PREFIX, SET, &[BOOTS], 0),
        affix(4000, "Movement Speed", PREFIX, STANDARD, &[BOOTS], 0),
        affix(4001, "Fire Resistance", SUFFIX, STANDARD, &[BOOTS], 0),
        affix(4002, "Cold Resistance", SUFFIX, STANDARD, &[BOOTS], 0),
        affix(4003, "Increased Armour", PREFIX, STANDARD, &[BOOTS], 0),
        affix(4004, "Sentinel's Resolve Reforged", PREFIX, SET, &[BOOTS], 0),
        affix(4005, "Void Corruption", SUFFIX, CORRUPTED, &[BOOTS], 0),
    ]
    .into_iter()
    .collect();
    // A Sentinel's ring's class affixes: the Sapphire Ring (any class) breaks both, the Obsidian Ring (Acolyte) the
    // Sentinel-only one.
    const SENTINEL: i32 = 4;
    const ACOLYTE: i32 = 8;
    if class_affixes {
        affixes.extend([
            affix(4010, "Sentinel's Bulwark", PREFIX, STANDARD, &[RING], 8),
            affix(4011, "Oathbound Vigil", SUFFIX, STANDARD, &[RING], 8 | 16),
        ]);
    }
    let unique = |id: u16, text: &str, item_type: u16, sub_types: &[u8], weavers_will: bool, is_set: bool, legendary: bool| {
        (
            id,
            UniqueDefinition {
                id,
                name: name(text),
                item_type: item_type as u8,
                sub_types: sub_types.to_vec(),
                is_set_item: is_set,
                set_id: 0,
                legendary_type: i32::from(weavers_will),
                can_drop_as_legendary: legendary,
                is_pre_corrupted: false,
                numeric: Availability::Unavailable {
                    reason: "The fixture world has no unique numeric catalog.".into(),
                },
            },
        )
    };
    let base = |sub_type: u16, text: &str, level: i32, corrupted: bool, class_requirement: i32, implicits: Vec<ImplicitDefinition>| SubTypeDefinition {
        sub_type,
        name: name(text),
        level_requirement: level,
        class_requirement,
        cannot_drop: false,
        is_corrupted: corrupted,
        implicits: Availability::Available(implicits),
    };
    let line = |minimum: &str, maximum: &str| implicit(Availability::Available(minimum.into()), Availability::Available(maximum.into()));
    let implicits = |lines: Vec<ImplicitDefinition>| if base_implicits { lines } else { Vec::new() };
    let class = |requirement| if class_affixes { requirement } else { 0 };
    let range = |minimum, maximum| ValueRange { minimum, maximum };
    GameCatalog {
        identity: CatalogIdentity {
            binary_hash: "a".repeat(64),
            metadata_hash: "b".repeat(64),
            extraction_version: EXTRACTION_VERSION,
            locale: GameLocale {
                language: "en".into(),
                game_version: "owned-ui".into(),
            },
        },
        limits: ItemLimits {
            forging_potential: range(0, 255),
            legendary_potential: range(0, 4),
            weavers_will: range(5, 28),
            weavers_touch: range(5, 14),
            level_requirement: range(0, 100),
        },
        bases: [(
            RING as u8,
            ItemTypeDefinition {
                item_type: RING as u8,
                name: name("Ring"),
                sub_types: vec![
                    base(
                        2,
                        "Gold Ring",
                        20,
                        false,
                        class(SENTINEL),
                        implicits(vec![line("+10 Armor", "+16 Armor"), line("+5% Cold Resistance", "+5% Cold Resistance")]),
                    ),
                    base(5, "Sapphire Ring", 28, false, 0, implicits(vec![line("+12% Fire Resistance", "+18% Fire Resistance")])),
                    base(9, "Obsidian Ring", 40, true, class(ACOLYTE), Vec::new()),
                ],
            },
        )]
        .into_iter()
        .collect(),
        affixes,
        // The ring uniques of the C# `UniqueIdentityTests`: one on another base and never legendary, one with Weaver's
        // Will, a set item; the amulet's must not be offered.
        uniques: [
            unique(77, "Wayfarer's Boots", BOOTS, &[0], false, false, true),
            unique(70, "Fixture Potential Unique", RING, &[2], false, false, false),
            unique(71, "Fixture Woven Unique", RING, &[2, 5], true, false, true),
            unique(72, "Fixture Set Ring", RING, &[2], false, true, false),
            unique(73, "Fixture Sapphire Unique", RING, &[5], false, false, false),
            unique(74, "Fixture Ember Unique", RING, &[2], false, false, true),
            unique(80, "Fixture Amulet Unique", AMULET, &[0], false, false, false),
        ]
        .into_iter()
        .collect(),
        rarities: ["Normal", "Magic", "Magic", "Rare", "Rare", "", "", "Unique", "Set", "Legendary"]
            .into_iter()
            .enumerate()
            .filter(|(_, text)| !text.is_empty())
            .map(|(id, text)| {
                (
                    id as u8,
                    RarityDefinition {
                        id: id as u8,
                        name: Availability::Available(name(text)),
                    },
                )
            })
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn affix(slot: u8, id: u16, tier: u16, roll: u8, kind: i32, seal: Seal, special: i32, text: &str) -> AffixSnapshot {
    AffixSnapshot {
        slot,
        id,
        tier,
        roll,
        kind,
        seal: seal.index() as i32,
        special,
        name: text.into(),
    }
}

fn status(forging_potential: u8, level_requirement: i32) -> ItemStatus {
    ItemStatus {
        forging_potential,
        forging_potential_type: 0,
        legendary_potential: 0,
        weavers_will: 0,
        weavers_touch: 0,
        level_requirement,
        corrupted: false,
        ruined: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn snapshot(
    text: &str,
    item_type: u16,
    sub_type: u16,
    rarity: u8,
    class_requirement: i32,
    affixes: Vec<AffixSnapshot>,
    identity: Option<UniqueIdentity>,
    implicit_rolls: Vec<u8>,
    status: ItemStatus,
) -> ForgeSnapshot {
    let mut item = ForgeSnapshot {
        item: 1,
        individual_id: 1,
        fingerprint: 0,
        name: text.into(),
        item_type: item_type as u8,
        sub_type,
        rarity,
        class_requirement,
        affixes,
        implicit_rolls,
        identity,
        status,
    };
    item.fingerprint = item.legacy_fingerprint();
    item
}

fn line(index: u32, text: &str, range: &str, roll_ids: &[u8], editable: bool) -> ModifierSnapshot {
    ModifierSnapshot {
        index,
        text: text.into(),
        range: range.into(),
        roll_ids: roll_ids.to_vec(),
        editable,
    }
}

/// The unique's lines as the world's game formats them from the rolls.
fn format_unique(rolls: &[u8], lines: &[ModifierSnapshot]) -> Vec<ModifierSnapshot> {
    let roll = |index: usize| f64::from(rolls.get(index).copied().unwrap_or(0));
    let speed = (10.0 + 15.0 * roll(0) / 255.0).round_ties_even();
    let void = (30.0 + 30.0 * roll(1) / 255.0).round_ties_even();
    lines
        .iter()
        .map(|line| ModifierSnapshot {
            text: match line.index {
                0 => format!("+{speed}% Movement Speed"),
                1 => format!("{void}% increased Void Damage over Time"),
                2 => format!("Your minions gain {speed}% Movement Speed"),
                _ => line.text.clone(),
            },
            ..line.clone()
        })
        .collect()
}

impl World {
    /// The showcase session on the boots: four affixes, value tables scaled by tier.
    fn boots() -> World {
        World {
            catalog: catalog(false, false),
            item: snapshot(
                "Wayfarer's Boots",
                BOOTS,
                0,
                0,
                0,
                vec![
                    affix(0, 25, 5, 170, SUFFIX, Seal::Normal, STANDARD, "Health"),
                    affix(1, 1148, 3, 200, PREFIX, Seal::Normal, SET, "Caretaker's Regrowth Reforged"),
                    affix(2, 4000, 1, 100, PREFIX, Seal::Sealed, STANDARD, "Movement Speed"),
                    affix(3, 4001, 4, 230, SUFFIX, Seal::Corrupted, STANDARD, "Fire Resistance"),
                ],
                None,
                Vec::new(),
                status(0, 0),
            ),
            identity: false,
            unmapped: Vec::new(),
            unique_lines: Vec::new(),
            implicit_lines: None,
        }
    }

    /// The legendary showcase item with its unique modifiers and a shared roll.
    fn unique() -> World {
        World {
            item: snapshot(
                "Wayfarer's Boots",
                BOOTS,
                0,
                9,
                0,
                vec![affix(0, 25, 7, 170, SUFFIX, Seal::Normal, STANDARD, "Health")],
                Some(UniqueIdentity {
                    id: 77,
                    rolls: vec![140, 200, 50],
                }),
                vec![80, 90, 100],
                status(0, 0),
            ),
            unique_lines: vec![
                line(0, "", "10% to 25%", &[0], true),
                line(1, "", "30% to 60%", &[1], true),
                line(2, "", "", &[0], true),
                line(3, "Cannot be slowed below base movement speed", "", &[], true),
                line(4, "Unresolved modifier", "", &[2], false),
            ],
            ..World::boots()
        }
    }

    /// The ring of the C# header tests (`ItemHeaderTests`), or with implicits and bases that grant them (`ImplicitTests`);
    /// identity tables as `FakeGameSession`.
    fn ring(implicits: bool) -> World {
        World {
            catalog: catalog(implicits, false),
            item: snapshot(
                "Gold Ring",
                RING,
                2,
                3,
                0,
                vec![affix(0, 25, 5, 170, SUFFIX, Seal::Normal, STANDARD, "Health")],
                None,
                if implicits { vec![128, 0] } else { vec![128] },
                status(30, 24),
            ),
            identity: true,
            unmapped: Vec::new(),
            unique_lines: Vec::new(),
            implicit_lines: implicits.then(|| vec![line(0, "+14 Armor", "(10 to 16)", &[0], true), line(1, "+5% Cold Resistance", "", &[], true)]),
        }
    }

    /// A unique ring (`UniqueIdentityTests`): its name opens the identity picker.
    fn unique_ring() -> World {
        let mut world = World::ring(false);
        world.item.name = "Fixture Potential Unique".into();
        world.item.rarity = 7;
        world.item.affixes.clear();
        world.item.identity = Some(UniqueIdentity { id: 70, rolls: vec![100, 7] });
        world.item.status = status(0, 0);
        world.change(|_| {})
    }

    /// A Sentinel's ring with two class affixes, for the base picker's incompatible counts and the refusal of a base that
    /// breaks affixes (the C# `ForgeChoices.BaseOptions`, `ApplyPlanner`).
    fn class_ring() -> World {
        let mut world = World::ring(false);
        world.catalog = catalog(false, true);
        world.item.class_requirement = 4;
        world.item.affixes.push(affix(1, 4010, 4, 120, PREFIX, Seal::Normal, STANDARD, "Sentinel's Bulwark"));
        world.item.affixes.push(affix(2, 4011, 3, 90, SUFFIX, Seal::Normal, STANDARD, "Oathbound Vigil"));
        world.change(|_| {})
    }

    fn change(mut self, change: impl FnOnce(&mut ForgeSnapshot)) -> World {
        change(&mut self.item);
        self.item.fingerprint = self.item.legacy_fingerprint();
        self
    }
}

struct WorldData {
    target: ProcessIdentity,
    world: World,
    item: Mutex<ForgeSnapshot>,
    writes: AtomicUsize,
}

struct WorldLocator(Arc<WorldData>);
impl ProcessLocator for WorldLocator {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        Ok(vec![self.0.target.clone()])
    }
}

struct WorldLogs;
impl LogSource for WorldLogs {
    fn read(&mut self, _: jiff::Timestamp) -> GameLogState {
        GameLogState {
            status: GameLogStatus::Current,
            readiness: GameReadiness::Offline,
            shell_state: Some("None".into()),
            offline_mode: Some(true),
            character: Some(eg_core::game_log::LoggedCharacter {
                name: "Owned fixture".into(),
                level: Some(12),
                online: false,
                cycle: "Legacy".into(),
                class: "Primalist".into(),
            }),
            problem: None,
        }
    }
}

struct WorldConnect(Arc<WorldData>);
impl Connector for WorldConnect {
    type Connection = WorldConnection;
    async fn connect(&mut self, _: ProcessIdentity, _: jiff::Timestamp) -> Result<WorldConnection, BackendError> {
        Ok(WorldConnection(self.0.clone()))
    }
}

/// Answers from the world's ranges: value = round(min + (max - min) × roll / 255), half to even as .NET rounds, shown as
/// `format` with the unit it ends in.
struct Range {
    minimum: f64,
    maximum: f64,
    decimals: i32,
    format: &'static str,
    effect: &'static str,
}

struct WorldConnection(Arc<WorldData>);
impl WorldConnection {
    fn range(&self, expected: &ExpectedObservation, request: &MappingRequest) -> Result<Option<Range>, BackendError> {
        if self.0.world.identity {
            return Ok(None);
        }
        let range = |minimum, maximum, decimals, format, effect| Range {
            minimum,
            maximum,
            decimals,
            format,
            effect,
        };
        Ok(Some(match request.address.target {
            MappingTarget::Unique => match request.address.index {
                0 => range(10.0, 25.0, 0, "{v}%", "{v}% increased Movement Speed"),
                1 => range(30.0, 60.0, 0, "{v}%", "{v}% increased Void Damage over Time"),
                _ => range(1.0, 5.0, 0, "{v}", ""),
            },
            MappingTarget::Implicit => range(1.0, 10.0, 0, "{v}", ""),
            MappingTarget::Affix => {
                let (id, tier) = match &request.draft {
                    Some(ItemEdit::Affix(edit)) => (edit.id as u16, edit.tier),
                    _ => {
                        let affix = &expected.snapshot().affixes[usize::from(request.address.index)];
                        (affix.id, i32::from(affix.tier))
                    }
                };
                if self.0.world.unmapped.contains(&id) {
                    return Err(BackendError::blocked(
                        backend::ErrorKind::MappingUnavailable,
                        "Actual values are unavailable for this affix.",
                    ));
                }
                let (minimum, maximum, decimals, format, effect) = match id {
                    25 => (20.0, 36.0, 0, "+{v}", "+{v} Health"),
                    1148 => (5.0, 9.0, 1, "{v}%", "{v}% increased Health Regen"),
                    4000 => (4.0, 8.0, 0, "{v}%", "{v}% increased Movement Speed"),
                    4001 => (15.0, 25.0, 0, "+{v}%", "+{v}% Fire Resistance"),
                    _ => (1.0, 10.0, 0, "{v}", ""),
                };
                // Synthetic tier scaling so a tier draft changes the value, as tiers do in the game.
                let scale = 0.6 + 0.1 * f64::from(tier);
                range(minimum * scale, maximum * scale, decimals, format, effect)
            }
        }))
    }
}

impl BackendConnection for WorldConnection {
    fn target(&self) -> &ProcessIdentity {
        &self.0.target
    }
    fn capabilities(&self) -> backend::Capabilities {
        backend::Capabilities {
            read_forge: true,
            level_requirement: true,
            status_fields: [true; 7],
            base_change: true,
            identity_change: true,
            affix_rolls: true,
            modifier_rolls: true,
            affix_change: true,
            affix_list: true,
            ..Default::default()
        }
    }
    async fn write(&mut self, plan: &WritePlan, _: &mut dyn WriteControl) -> Result<WriteOutcome<WriteObservation>, BackendError> {
        let WritePlan::Forge(plan) = plan;
        self.0.writes.fetch_add(1, Ordering::SeqCst);
        let mut after = plan.projected().clone();
        // The game names the item after its new identity.
        if let ItemEdit::UniqueIdentity(edit) = plan.edit_intent()
            && let Some(unique) = self.0.world.catalog.uniques.get(&(edit.unique_id as u16))
        {
            after.name = unique.name.text.clone();
            after.fingerprint = after.legacy_fingerprint();
        }
        *self.0.item.lock().unwrap() = after.clone();
        Ok(WriteOutcome::Confirmed(WriteObservation::forge(after)))
    }
    async fn refresh(&mut self) -> Result<RuntimeStatus, BackendError> {
        Ok(RuntimeStatus {
            online: false,
            manager: 12,
            features_on: 0,
        })
    }
    async fn read_locale(&mut self) -> Result<GameLocale, BackendError> {
        Ok(GameLocale {
            language: "en".into(),
            game_version: "owned-ui".into(),
        })
    }
    fn build_hashes(&self) -> (String, String) {
        ("a".repeat(64), "b".repeat(64))
    }
    async fn read_forge(&mut self) -> Result<Option<ForgeSnapshot>, BackendError> {
        Ok(Some(self.0.item.lock().unwrap().clone()))
    }
    async fn describe_implicits(&mut self, _: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(self.0.world.implicit_lines.clone())
    }
    async fn describe_unique(&mut self, expected: &ForgeSnapshot) -> Result<Option<Vec<ModifierSnapshot>>, BackendError> {
        Ok(expected.identity.as_ref().map(|identity| format_unique(&identity.rolls, &self.0.world.unique_lines)))
    }
    async fn preview_unique(&mut self, expected: &ExpectedObservation, draft: &RollEdit, evidence: &BoundTooltip) -> Result<BoundPreview, BackendError> {
        let mut rolls = expected.snapshot().identity.as_ref().map(|identity| identity.rolls.clone()).unwrap_or_default();
        for change in &draft.changes {
            if let Some(roll) = rolls.get_mut(change.index as usize) {
                *roll = change.value as u8;
            }
        }
        let modifiers = format_unique(&rolls, &self.0.world.unique_lines);
        BoundPreview::new(expected, draft.clone(), UniqueRollPreview { modifiers }, evidence)
            .map_err(|error| BackendError::blocked(backend::ErrorKind::MappingUnavailable, error.to_string()))
    }
    async fn resolve_mapping(
        &mut self,
        expected: &ExpectedObservation,
        request: &MappingRequest,
        tooltip: Option<&eg_trainer::item::values::BoundTooltip>,
        _: &mut dyn backend::ReadObserver,
    ) -> Result<ValueTable, BackendError> {
        let range = self.range(expected, request)?;
        let sample = |roll: u8| -> RawValueSample {
            let Some(range) = &range else {
                return RawValueSample {
                    raw_quantity: f64::from(roll),
                    display: roll.to_string(),
                    unit: String::new(),
                    effects: vec![format!("Fixture effect {roll}")],
                };
            };
            let scale = 10f64.powi(range.decimals);
            let value = ((range.minimum + (range.maximum - range.minimum) * f64::from(roll) / 255.0) * scale).round_ties_even() / scale;
            let shown = format!("{value:.*}", range.decimals as usize);
            RawValueSample {
                raw_quantity: value,
                display: range.format.replace("{v}", &shown),
                unit: if range.format.ends_with('%') { "%".into() } else { String::new() },
                effects: if range.effect.is_empty() {
                    Vec::new()
                } else {
                    vec![range.effect.replace("{v}", &shown)]
                },
            }
        };
        let page = |start, samples| RawMappingPage {
            language: "en".into(),
            context: b"fixture-world".to_vec(),
            address: request.address,
            start,
            samples,
        };
        struct Parser;
        impl DisplayScalarParser for Parser {
            fn parse(&mut self, _: &str, display: &str, unit: &str) -> Result<f64, String> {
                display
                    .strip_suffix(unit)
                    .unwrap_or(display)
                    .parse()
                    .map_err(|error: std::num::ParseFloatError| error.to_string())
            }
        }
        let invalid = |error: eg_trainer::item::values::MappingError| BackendError::blocked(backend::ErrorKind::MappingUnavailable, error.to_string());
        let mut builder = MappingBuilder::new(expected.clone(), request.clone(), page(MappingStart::ContextOnly, vec![]), tooltip).map_err(invalid)?;
        while let Some(start) = builder.next_start() {
            let samples = (start..=start + 15).map(sample).collect();
            builder.push(expected, request, page(MappingStart::Samples(start), samples), &mut Parser).map_err(invalid)?;
        }
        builder.finish().map_err(invalid)
    }
    async fn disconnect(&mut self) -> Cleanup {
        Cleanup::ConfirmedOff
    }
}

/// The forge in a window, on a worker connected to a world.
struct Forge {
    ui: MainWindow,
    handle: ForgeHandle,
    backend: BackendHandle,
    data: Arc<WorldData>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Forge {
    fn drop(&mut self) {
        command(&self.backend, Command::Shutdown, || self.backend.snapshot().phase == Phase::Stopped);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

/// No value table or unique description is still on its way.
fn settled(editor: &ForgeEditor) -> bool {
    editor.affixes.iter().all(|row| row.value.status() != LOADING)
        && editor
            .uniques
            .iter()
            .chain(&editor.implicits)
            .flat_map(|row| &row.rolls)
            .all(|roll| roll.editor.status() != LOADING)
}

impl Forge {
    fn open(session: &Session, world: World, width: u32, height: u32) -> Forge {
        let folder = session.folder();
        let data = Arc::new(WorldData {
            target: ProcessIdentity {
                pid: 1,
                created: 134_369_000_000_000_000,
                executable: folder.join("owned-never-opened.exe"),
            },
            item: Mutex::new(world.item.clone()),
            writes: AtomicUsize::new(0),
            world,
        });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (backend, worker) = backend::service(
            WorldLocator(data.clone()),
            WorldConnect(data.clone()),
            WorldLogs,
            SystemClock,
            BackendOptions {
                auto_connect: false,
                poll_interval: Duration::from_secs(60),
                game_data: Some(GameDataHandle::fixed(GameData::Ready(Arc::new(FileCatalog::restore(
                    data.world.catalog.clone(),
                    "owned".into(),
                    vec![],
                ))))),
                ..Default::default()
            },
        );
        let worker = std::thread::spawn(move || runtime.block_on(worker.run()));
        let ui = MainWindow::new().unwrap();
        let store = SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]);
        let installed = eg_ui::install(
            &ui,
            App {
                launcher: None,
                mods: None,
                trainer: true,
                trainer_backend: Some(backend.clone()),
                faults: None,
                templates: None,
                settings: SettingsPage::new(SettingsService::inline(store), vec![eg_ui::pages::settings::trainer::descriptor(true)], None, None),
                start: Page::Forge,
            },
        );
        let handle = installed.forge.expect("the forge on the worker");
        // Applied entries carry the local time; captures hold one.
        handle.edit(|editor| editor.clock = || "23:54".into());
        ui.global::<Tokens>().set_motion(false);
        ui.global::<Tokens>().set_high_contrast(false);
        session.window.set_size(PhysicalSize::new(width, height));
        ui.show().unwrap();
        command(&backend, Command::Enable, || backend.snapshot().phase == Phase::Disconnected);
        ui.global::<eg_ui::Connection>().invoke_connect();
        wait(|| ui.global::<eg_ui::Connection>().get_connected());
        command(&backend, Command::ReadForge, || matches!(backend.snapshot().forge, backend::ForgeState::Item { .. }));
        let forge = Forge {
            ui,
            handle,
            backend,
            data,
            worker: Some(worker),
        };
        forge.settle();
        // The forge comes into being with the first frame, and its pane follows the window class from there.
        forge.ui.window().take_snapshot().expect("the forge's first frame");
        settle();
        forge
    }

    /// Waits until the item shows, its unique's lines have come, and no table or description is on its way.
    fn settle(&self) {
        let lines = self.data.world.unique_lines.len();
        let until = std::time::Instant::now() + Duration::from_secs(8);
        while !(self.ui.global::<ForgeState>().get_has_item()
            && self.handle.edit(|editor| settled(editor) && editor.uniques.len() >= lines)
            && !self.ui.global::<ForgeState>().get_formatting_loading())
        {
            assert!(
                std::time::Instant::now() < until,
                "the forge did not settle: has item {}, unavailable {:?}, message {:?}",
                self.ui.global::<ForgeState>().get_has_item(),
                self.ui.global::<eg_ui::TrainerState>().get_item_unavailable(),
                self.ui.global::<ForgeState>().get_message()
            );
            std::thread::sleep(Duration::from_millis(5));
            settle();
        }
        settle();
    }

    /// The game changes the item behind the editor (another player action, a reload), and the forge reads it again.
    fn change(&self, change: impl FnOnce(&mut ForgeSnapshot)) {
        let item = {
            let mut item = self.data.item.lock().unwrap();
            change(&mut item);
            item.fingerprint = item.legacy_fingerprint();
            item.clone()
        };
        command(
            &self.backend,
            Command::ReadForge,
            || matches!(&self.backend.snapshot().forge, backend::ForgeState::Item { snapshot, .. } if **snapshot == item),
        );
        wait(|| {
            self.handle
                .edit(|editor| editor.item().is_some_and(|shown| shown.context.as_ref().is_some_and(|c| c.expected.snapshot() == &item)))
        });
        self.settle();
    }

    fn writes(&self) -> usize {
        self.data.writes.load(Ordering::SeqCst)
    }

    /// Applies the drafts and waits for the writes to settle.
    fn apply(&self) {
        self.handle.edit(ForgeEditor::apply);
        self.wait_writes();
    }

    /// Waits until no write is in flight.
    fn wait_writes(&self) {
        wait(|| !self.handle.edit(|editor| editor.is_writing()));
        self.settle();
    }
}

// Types a value into a cell's popover, as Enter commits it.
fn type_value(editor: &mut ForgeEditor, cell: Cell, text: &str) {
    assert!(editor.begin_input(cell));
    editor.set_input(cell, text);
    assert!(editor.commit_input(cell));
}

/// The Item Forge on the fixture world, as the C# app's fixture captures show it: drafts, Apply and Undo, the cells' menus,
/// the pickers, implicits and a unique item.
pub(in super::super) fn item_captures(session: &Session) {
    // Not connected, Connect in the page (`forge-not-connected`; Home stays in the rail), and the same wait at three sizes.
    for (name, width, height) in [
        ("forge-not-connected", 1100, 700),
        ("item-empty", 1100, 700),
        ("item-empty-compact", 940, 580),
        ("item-empty-narrow", 680, 580),
    ] {
        let ui = session.open(true, width, height);
        ui.global::<Nav>().invoke_navigate("forge".into());
        settle();
        assert!(!ui.global::<eg_ui::Connection>().get_connected());
        session.capture(&ui, name);
    }

    // Apply, a draft stepped back with Ctrl+Z, Undo apply, then a draft discarded: the ledger keeps both readbacks and the
    // message says what was discarded (`ui-preview`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    forge.handle.edit(|editor| {
        editor.select_affix(Some(0));
        type_value(editor, Cell::Affix(0), "37");
    });
    forge.apply();
    forge.handle.edit(|editor| {
        type_value(editor, Cell::Affix(0), "38");
        editor.undo_draft_only();
        editor.undo();
    });
    forge.wait_writes();
    forge.handle.edit(|editor| {
        type_value(editor, Cell::Affix(0), "36");
        editor.discard_entry(0);
    });
    settle();
    assert_eq!(forge.ui.global::<ForgeState>().get_message(), "Draft for Health discarded.");
    assert_eq!(forge.writes(), 2, "the apply and its undo");
    session.capture(&forge.ui, "item-ui-preview");
    drop(forge);

    // Two drafts pending at every window class: Movement Speed T1 → T4, Health +34 → +37.
    for (class, width, height) in SIZES {
        let forge = Forge::open(session, World::boots(), width, height);
        forge.handle.edit(|editor| {
            editor.pick_tier(2, 4);
            editor.select_affix(Some(0));
        });
        forge.settle();
        forge.handle.edit(|editor| type_value(editor, Cell::Affix(0), "37"));
        settle();
        let state = forge.ui.global::<ForgeState>();
        assert_eq!(state.get_pane_open(), class != "narrow", "the pane starts closed only in a narrow window");
        assert_eq!(state.get_apply_text(), "Apply 2 drafts");
        state.set_pane_open(true);
        settle();
        session.capture(&forge.ui, &format!("item-{class}"));
        state.set_pane_open(false);
        settle();
        session.capture(&forge.ui, &format!("item-{class}-hidden"));
    }

    // A narrow window with the pane open over the table: Tab skips the cells under it and goes on into the pane
    // (`item-narrow-pane-keyboard`); Escape there closes the pane and gives focus back to Changes (`item-narrow-pane-escape`).
    let forge = Forge::open(session, World::boots(), 680, 580);
    forge.handle.edit(|editor| editor.pick_tier(2, 4));
    forge.settle();
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(0), "37"));
    let state = forge.ui.global::<ForgeState>();
    state.set_pane_open(true);
    settle();
    // The rail's four stops, the rarity, the status strip's four, then the four values (the names, tiers and seals lie
    // under the pane), Add affix, Max tiers and Max rolls, which drafts wait beside.
    for _ in 0..17 {
        session.key(Key::Tab);
    }
    session.capture(&forge.ui, "item-narrow-pane-keyboard");
    // The first Escape closes the entry's tip, the next the pane.
    session.key(Key::Escape);
    assert!(state.get_pane_open());
    session.key(Key::Escape);
    assert!(!state.get_pane_open(), "Escape in the overlaid pane closes it");
    session.capture(&forge.ui, "item-narrow-pane-escape");
    drop(forge);

    // A seal-only draft beside a value draft, then Apply: both confirmed by readback, Fire Resistance moved where the game
    // loads a normal affix (`seal-draft`, `confirmed`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    forge.handle.edit(|editor| {
        type_value(editor, Cell::Affix(0), "37");
        editor.select_affix(Some(3));
        editor.pick_seal(3, Seal::Normal);
    });
    settle();
    session.capture(&forge.ui, "item-seal-draft");
    forge.apply();
    assert!(forge.ui.global::<ForgeState>().get_message().starts_with("2 drafts applied."));
    assert_eq!(forge.writes(), 2);
    session.capture(&forge.ui, "item-confirmed");
    drop(forge);

    // A name too long for the header wraps to two lines and ends in an ellipsis, with a seal drafted and another row
    // selected (`item-long-name-{940,1100}`, the C# `shell-{940,1100}-long-name`).
    const LONG_NAME: &str = "Wayfarer's Boots of Unwavering Determination and Extraordinary Elemental Resistance";
    for (width, height) in [(940, 580), (1100, 700)] {
        let forge = Forge::open(session, World::boots().change(|item| item.name = LONG_NAME.into()), width, height);
        forge.handle.edit(|editor| {
            editor.select_affix(Some(0));
            editor.pick_seal(0, Seal::Sealed);
            editor.select_affix(Some(2));
        });
        settle();
        assert_eq!(forge.ui.global::<ForgeState>().get_item_name(), LONG_NAME);
        session.capture(&forge.ui, &format!("item-long-name-{width}"));
    }

    // High contrast raises the hairlines, frames and notes in place, a value drafted to its maximum (`item-high-contrast`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    forge.ui.global::<Tokens>().set_high_contrast(true);
    forge.handle.edit(|editor| editor.step_to_end(Cell::Affix(0), true));
    settle();
    session.capture(&forge.ui, "item-high-contrast");
    forge.ui.global::<Tokens>().set_high_contrast(false);
    drop(forge);

    // A draft the game changed after it was made: Apply refuses the whole ledger with the reason (`rejected`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(0), "37"));
    forge.change(|item| item.affixes[0].roll = 120);
    forge.handle.edit(ForgeEditor::apply);
    settle();
    assert!(forge.ui.global::<ForgeState>().get_message().contains("changed in the game after you drafted it"));
    assert_eq!(forge.writes(), 0);
    session.capture(&forge.ui, "item-rejected");
    // The stale row says so in words too, for when the changes pane is closed (`stale`).
    forge.ui.global::<ForgeState>().set_pane_open(false);
    settle();
    session.capture(&forge.ui, "item-stale");
    drop(forge);

    // Maximized: the selected affix's tier ladder beside the table (`tier-ladder`).
    let forge = Forge::open(session, World::boots(), 1920, 1080);
    forge.handle.edit(|editor| editor.pick_tier(0, 7));
    forge.settle();
    session.capture(&forge.ui, "item-tier-ladder");
    drop(forge);

    // A unique item: its modifiers as the game formats them, the shared roll on two lines (`unique-standard`,
    // `read-only`, `unique-maximized`).
    let forge = Forge::open(session, World::unique(), 1100, 700);
    forge.handle.edit(|editor| editor.select_unique(Some(0)));
    settle();
    session.capture(&forge.ui, "item-unique-standard");
    forge.handle.edit(|editor| editor.select_unique(Some(3)));
    settle();
    session.capture(&forge.ui, "item-unique-read-only");
    session.resize(1920, 1080);
    session.capture(&forge.ui, "item-unique-maximized");
    session.resize(1100, 700);
    // Max rolls drafts every roll that can change to the game's perfect roll, a shared roll once (`item-max-rolls`).
    assert!(forge.ui.global::<ForgeState>().get_can_max_rolls());
    forge.handle.edit(ForgeEditor::max_rolls);
    settle();
    forge.handle.edit(|editor| {
        assert!(editor.draft_count() > 0);
        assert!(
            editor
                .uniques
                .iter()
                .flat_map(|line| &line.rolls)
                .all(|roll| roll.value == u8::MAX || !roll.editor.has_mapping())
        );
        assert!(editor.affixes.iter().all(|row| row.roll() == u8::MAX || !row.value.has_mapping()));
    });
    assert!(!forge.ui.global::<ForgeState>().get_can_max_rolls());
    session.capture(&forge.ui, "item-max-rolls");
    drop(forge);

    // A unique modifier with two rolls shows both in its own row, one cell over the other (`unique-rolls-*`).
    for (width, height) in [(1100, 700), (940, 580)] {
        let mut world = World::unique();
        world.unique_lines[1].roll_ids = vec![1, 2];
        let forge = Forge::open(session, world, width, height);
        forge.handle.edit(|editor| editor.select_unique(Some(1)));
        settle();
        let line = forge.ui.global::<ForgeState>().get_unique_modifiers().row_data(1).unwrap();
        assert_eq!(line.rolls.row_count(), 2, "both rolls are value cells in the one row");
        session.capture(&forge.ui, &format!("unique-rolls-{width}"));
    }

    // A Weaver's Will unique: its status strip leads with Weaver's Will, corrupted, the base fixed (`item-status-unique`).
    let world = World::unique_ring().change(|item| {
        item.name = "Fixture Woven Unique".into();
        item.identity.as_mut().unwrap().id = 71;
        item.status.weavers_will = 12;
        item.status.corrupted = true;
    });
    let forge = Forge::open(session, world, 1100, 700);
    let fields = forge.ui.global::<ForgeState>().get_fields();
    assert_eq!(fields.row_data(0).unwrap().name, "Weaver's Will");
    session.capture(&forge.ui, "item-status-unique");
    drop(forge);

    // The ring: the status strip, a flag and the base drafted (`item-status-*`), then its implicits (`implicits*`).
    for (class, width, height) in SIZES {
        let forge = Forge::open(session, World::ring(false), width, height);
        forge.handle.edit(|editor| {
            type_value(editor, Cell::Field(0), "60");
            editor.toggle_flag(2, true);
            editor.pick_base(1);
        });
        forge.settle();
        session.capture(&forge.ui, &format!("item-status-{class}"));
    }
    // The implicits as the game describes them; the app does not write implicit rolls yet, so the base is what changes.
    let forge = Forge::open(session, World::ring(true), 1100, 700);
    session.capture(&forge.ui, "item-implicits");
    forge.handle.edit(|editor| editor.pick_base(1));
    forge.settle();
    session.capture(&forge.ui, "item-implicits-base-draft");
    drop(forge);

    // ----- Real input on the cells: menus, the value popover, the keyboard and the replacement catalogue -----
    let pointer = |forge: &Forge, x: f32, y: f32, kind: &str| {
        let position = LogicalPosition::new(x, y);
        let event = match kind {
            "move" => WindowEvent::PointerMoved { position },
            "down" => WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            },
            "up" => WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            },
            "right" => WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            _ => WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        };
        forge.ui.window().dispatch_event(event);
        settle();
    };
    // Rows of the table at 1100×700, under the status strip: 52px rows 2px apart under the header; the cells' columns.
    let row_y = |row: usize| 262.0 + 54.0 * row as f32;
    const VALUE_X: f32 = 150.0;
    const NAME_X: f32 = 300.0;
    const TIER_X: f32 = 530.0;

    // A click on a tier cell opens the tier menu with the tier the item holds (`tier-menu`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    session.click(TIER_X, row_y(0));
    session.capture(&forge.ui, "item-tier-menu");
    session.key(Key::DownArrow);
    session.key(Key::Return);
    assert_eq!(
        forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().tier,
        "T6",
        "Down and Enter pick the next tier"
    );
    drop(forge);
    // Down from one tier cell to the next: both tips say "Change tier", and the one left behind closing a moment later
    // must not take the new cell's with it (the tip is its opener's, not its text's).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    let tip = forge.ui.global::<Tip>();
    for _ in 0..40 {
        if tip.get_open() && tip.get_text() == "Change tier" {
            break;
        }
        session.key(Key::Tab);
    }
    assert_eq!(tip.get_text(), "Change tier", "Tab reaches a tier cell");
    let owner = tip.get_owner();
    session.key(Key::DownArrow);
    assert!(tip.get_open() && tip.get_owner() != owner, "the next row's tier cell puts its own tip up");
    advance(Duration::from_millis(300));
    assert!(tip.get_open(), "the tier cell's tip stays open under keyboard focus");
    drop(forge);

    let forge = Forge::open(session, World::boots(), 1100, 700);
    // Hovering a value lights only its bar (`value-hover`); a click without dragging opens the popover (`value-popover`).
    pointer(&forge, VALUE_X + 70.0, row_y(1) - 6.0, "move");
    session.capture(&forge.ui, "item-value-hover");
    session.click(VALUE_X, row_y(1) - 6.0);
    assert_eq!(forge.ui.global::<ForgeState>().get_input_text(), "7.3");
    session.capture(&forge.ui, "item-value-popover");
    // A value outside the game's range keeps the popover open with the reason; Escape drops it.
    session.control_key("a");
    session.type_text("12");
    session.key(Key::Return);
    assert_eq!(forge.ui.global::<ForgeState>().get_input_error(), "Enter a value from 4.5% to 8.1%.");
    session.capture(&forge.ui, "item-value-invalid");
    session.key(Key::Escape);
    assert_eq!(forge.ui.global::<ForgeState>().get_drafts().row_count(), 0);
    // A drag scrubs through the values the game can produce, all one draft change (`value-scrub`).
    pointer(&forge, VALUE_X, row_y(0) - 6.0, "move");
    pointer(&forge, VALUE_X, row_y(0) - 6.0, "down");
    for step in 1..=4 {
        pointer(&forge, VALUE_X + 12.0 * step as f32, row_y(0) - 6.0, "move");
    }
    session.capture(&forge.ui, "item-value-scrub");
    pointer(&forge, VALUE_X + 48.0, row_y(0) - 6.0, "up");
    let drafts = forge.ui.global::<ForgeState>().get_drafts();
    assert_eq!(drafts.row_count(), 1);
    assert!(drafts.row_data(0).unwrap().summary.starts_with("Value +34 → +"));
    drop(forge);

    // The game takes an affix away during a drag: the rows that stay are the same rows, so the drag goes on and its
    // release ends it. A second drag is then an Undo step of its own; one Ctrl+Z takes back only that one.
    let forge = Forge::open(session, World::boots(), 1100, 700);
    let drag = |from: f32, steps: usize, release: bool| {
        pointer(&forge, from, row_y(0) - 6.0, "move");
        pointer(&forge, from, row_y(0) - 6.0, "down");
        for step in 1..=steps {
            pointer(&forge, from + 12.0 * step as f32, row_y(0) - 6.0, "move");
        }
        if release {
            pointer(&forge, from + 12.0 * steps as f32, row_y(0) - 6.0, "up");
        }
    };
    drag(VALUE_X, 3, false);
    forge.change(|item| {
        item.affixes.pop();
    });
    settle();
    assert_eq!(forge.ui.global::<ForgeState>().get_affixes().row_count(), 3);
    pointer(&forge, VALUE_X + 36.0, row_y(0) - 6.0, "up");
    let first = forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().value.display;
    drag(VALUE_X + 36.0, 3, true);
    assert_ne!(forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().value.display, first);
    forge.handle.edit(|editor| editor.undo_draft_only());
    settle();
    assert_eq!(
        forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().value.display,
        first,
        "Ctrl+Z takes back the second drag only"
    );
    // The row menu at the pointer (`row-menu`).
    pointer(&forge, NAME_X, row_y(0), "move");
    pointer(&forge, NAME_X, row_y(0), "right");
    pointer(&forge, NAME_X, row_y(0), "right-up");
    session.capture(&forge.ui, "item-row-menu");
    // The row under the pointer takes a ground of its own on the menu's (`row-menu-hover`).
    pointer(&forge, NAME_X + 60.0, row_y(0) + 47.0, "move");
    session.capture(&forge.ui, "item-row-menu-hover");
    session.key(Key::Escape);
    drop(forge);

    // Del on a focused row's cell drafts the affix's removal, as Remove affix does, beside the other drafts; again it
    // leaves the removal (the row menu keeps the affix). Adding and removing wait for Apply.
    let affixes = |forge: &Forge| forge.ui.global::<ForgeState>().get_affixes().row_count();
    let removed = |forge: &Forge, row: usize| forge.ui.global::<ForgeState>().get_affixes().row_data(row).unwrap().removed;
    let forge = Forge::open(session, World::boots(), 1100, 700);
    // The rail's four stops, the rarity and the status strip's four, then the first row's value and its name.
    for _ in 0..11 {
        session.key(Key::Tab);
    }
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(1), "7.5"));
    session.key(Key::Delete);
    assert!(removed(&forge, 0), "Del on the name drafts the removal");
    session.key(Key::Delete);
    assert!(removed(&forge, 0), "Del again leaves the removal drafted");
    // An addition is a row of its own after the game's (`item-affix-list-drafts`); nothing is written until Apply, which
    // removes first and adds last.
    forge.handle.edit(|editor| {
        assert!(editor.open_add());
        editor.picker_search("armour");
        editor.picker_select(0);
        editor.use_picker();
    });
    forge.settle();
    assert_eq!(affixes(&forge), 5);
    assert_eq!(forge.writes(), 0);
    session.capture(&forge.ui, "item-affix-list-drafts");
    forge.apply();
    assert_eq!(forge.writes(), 3);
    assert_eq!(affixes(&forge), 4);
    assert!(!removed(&forge, 0));
    drop(forge);

    // The name opens the affix picker: a search, the incompatible toggle, the first entry (`replace`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    session.click(NAME_X, row_y(3));
    assert!(forge.ui.global::<ForgeState>().get_picker_open());
    session.type_text("res");
    forge.ui.global::<ForgeState>().invoke_picker_allow_changed(true);
    settle();
    session.key(Key::DownArrow);
    session.capture(&forge.ui, "item-replace");
    // The verdict, then the picked affix's tier ladder.
    assert!(
        forge
            .ui
            .global::<ForgeState>()
            .get_picker_status()
            .starts_with("Compatible with this slot\nT1\u{a0}+5–\u{2060}14 · T2")
    );
    session.key(Key::Return);
    assert_eq!(
        forge.ui.global::<ForgeState>().get_drafts().row_data(0).unwrap().summary,
        "Fire Resistance → Cold Resistance"
    );
    forge.settle();
    session.capture(&forge.ui, "item-replaced");
    drop(forge);

    // A value whose mapping the game could not read is edited by its roll position, 0 to 255.
    let mut world = World::boots();
    world.unmapped.push(1148);
    let forge = Forge::open(session, world, 1100, 700);
    session.click(VALUE_X, row_y(1) - 6.0);
    session.key(Key::Tab);
    session.key(Key::Backtab);
    session.capture(&forge.ui, "item-value-positions");
    drop(forge);

    // Item templates: the boots' affixes saved, the item changed in the game, the template drafted back onto it; the
    // flyout lists it (`item-templates`).
    let forge = Forge::open(session, World::boots(), 1100, 700);
    let folder = session.folder().join("forge-templates");
    forge.handle.edit(|editor| {
        editor.templates = Some(eg_trainer::item::template::TemplateStore::new(folder));
        editor.save_template("Wayfarer's set", false);
        assert_eq!(editor.template_message, "Saved \"Wayfarer's set\".");
    });
    forge.change(|item| {
        item.affixes[0].tier = 2;
        item.affixes[0].roll = 10;
    });
    forge.handle.edit(|editor| {
        editor.apply_template(0);
        assert_eq!(editor.template_message, "Drafted \"Wayfarer's set\": 1 draft. Review it, then Apply.");
        assert_eq!((editor.affixes[0].tier(), editor.affixes[0].roll()), (5, 170));
    });
    settle();
    let state = forge.ui.global::<ForgeState>();
    state.invoke_templates_opened();
    assert_eq!(state.get_template_rows().row_count(), 1);
    session.click(445.0, 472.0);
    session.capture(&forge.ui, "item-templates");
    // The name field has the keyboard. A name that shares the kept template's file says what it replaces, and Save
    // becomes Replace; Enter goes to Replace rather than replacing, and a second Enter replaces.
    session.type_text("WAYFARER'S SET");
    assert_eq!(state.get_template_replaces(), "Wayfarer's set");
    session.capture(&forge.ui, "item-templates-replace");
    session.key(Key::Return);
    assert_eq!(state.get_template_message(), "", "Enter in the name field does not replace");
    session.key(Key::Return);
    assert_eq!(state.get_template_message(), "Saved \"WAYFARER'S SET\" in place of \"Wayfarer's set\".");
    assert_eq!((state.get_template_name().as_str(), state.get_template_replaces().as_str()), ("", ""));
    assert_eq!(state.get_template_rows().row_count(), 1);
    // Escape closes it and gives the keyboard back to Templates, whose Enter opens it again.
    session.key(Key::Escape);
    session.capture(&forge.ui, "item-templates-escape");
    session.key(Key::Return);
    // Delete asks first, in the row; the key that asked lands on Keep, and Escape backs out of the question only.
    session.key(Key::Backtab);
    session.key(Key::Return);
    session.capture(&forge.ui, "item-templates-delete");
    session.key(Key::Escape);
    session.key(Key::Return);
    session.key(Key::Backtab);
    session.key(Key::Return);
    assert_eq!(state.get_template_rows().row_count(), 0);
    assert_eq!(state.get_template_message(), "Deleted \"WAYFARER'S SET\".");
    session.key(Key::Escape);
    drop(forge);

    // Many templates in a short window on a one-affix item: the flyout has room on neither side of Templates, so it
    // stays inside the window.
    let forge = Forge::open(session, World::boots().change(|item| item.affixes.truncate(1)), 940, 580);
    let folder = session.folder().join("forge-templates-many");
    forge.handle.edit(|editor| {
        editor.templates = Some(eg_trainer::item::template::TemplateStore::new(folder));
        for name in [
            "Boots of Haste",
            "Cold resist",
            "Crit boots",
            "Fire resist",
            "Health stack",
            "Movement",
            "A name long enough that the row has to cut it short",
        ] {
            editor.save_template(name, false);
        }
    });
    settle();
    session.click(445.0, 310.0);
    session.capture(&forge.ui, "item-templates-many-940");
    session.key(Key::Escape);
    drop(forge);
    // The affix list's row as a run with templates has it, at the narrowest window.
    let forge = Forge::open(session, World::boots(), 680, 580);
    forge
        .handle
        .edit(|editor| editor.templates = Some(eg_trainer::item::template::TemplateStore::new(session.folder().join("forge-templates-many"))));
    settle();
    session.capture(&forge.ui, "item-templates-narrow");
    drop(forge);
    // A whole-item template on the legendary: its unique rolls come back with its affixes.
    let forge = Forge::open(session, World::unique(), 1100, 700);
    let folder = session.folder().join("forge-templates");
    forge.handle.edit(|editor| {
        editor.templates = Some(eg_trainer::item::template::TemplateStore::new(folder));
        editor.save_template("Haste legendary", true);
    });
    forge.change(|item| {
        item.identity.as_mut().unwrap().rolls[1] = 20;
        item.affixes[0].roll = 3;
    });
    forge.handle.edit(|editor| {
        editor.apply_template(0);
        assert!(editor.template_message.starts_with("Drafted \"Haste legendary\""), "{}", editor.template_message);
        assert_eq!(editor.affixes[0].roll(), 170);
        assert!(
            editor
                .uniques
                .iter()
                .flat_map(|line| &line.rolls)
                .filter(|roll| roll.index == 1)
                .all(|roll| roll.value == 200)
        );
    });
    drop(forge);

    // Add affix opens the picker to add, its caption the free slots.
    let forge = Forge::open(session, World::boots(), 1100, 700);
    session.click(145.0, row_y(4) - 2.0);
    assert_eq!(forge.ui.global::<ForgeState>().get_picker_title(), "Add affix");
    session.capture(&forge.ui, "item-add-affix");
    // The kind and class filters narrow the list; each entry shows its T1 range (`item-add-affix-filtered`).
    let state = forge.ui.global::<ForgeState>();
    let all = state.get_picker_rows().row_count();
    state.invoke_picker_category_changed(1);
    settle();
    assert!(state.get_picker_rows().row_count() <= all);
    assert!(state.get_picker_rows().row_data(0).unwrap().detail.contains("T1: +5–14"));
    state.invoke_picker_select(0);
    settle();
    session.capture(&forge.ui, "item-add-affix-filtered");
    // The kind choice opens its list over the picker (`item-add-affix-kind-list`).
    session.click(200.0, 387.0);
    session.capture(&forge.ui, "item-add-affix-kind-list");
    session.key(Key::Escape);
    state.invoke_picker_category_changed(0);
    state.invoke_picker_class_changed(2);
    settle();
    assert_eq!(state.get_picker_rows().row_count(), 0, "no fixture affix is Primalist-only");
    state.invoke_picker_class_changed(0);
    settle();
    session.key(Key::Escape);
    // A corrupted affix takes the item's one corrupted slot: the affix holding it is drafted back to a normal one first.
    // The game adds corrupted affixes only by corruption, so the forge adds one with "I know what I'm doing".
    command(&forge.backend, Command::SetOverride(true), || forge.backend.snapshot().override_enabled);
    forge.handle.edit(|editor| {
        assert!(editor.open_add());
        editor.picker_allow(true);
        editor.picker_search("Void");
        editor.picker_select(0);
        assert!(editor.picker_status().contains("Fire Resistance becomes a normal affix"), "{}", editor.picker_status());
        editor.use_picker();
        assert_eq!(editor.affixes[3].seal(), Seal::Normal);
        assert_eq!(editor.affixes[4].seal(), Seal::Corrupted);
        assert_eq!(editor.draft_count(), 2);
    });
    drop(forge);

    // The rarity in the facts line opens the rarity menu (`item-rarity-menu`); the base, the base picker with the implicits
    // each base grants (`implicits-base-picker`) and, over the status drafts with the base drafted, on the drafted base
    // (`item-base-picker`). Exalted is drafted first, so the menu opens over the rarity in draft colour; Escape closes it and
    // keeps the draft.
    let forge = Forge::open(session, World::ring(false), 1100, 700);
    forge.handle.edit(|editor| editor.pick_rarity(true));
    settle();
    assert_eq!(forge.ui.global::<ForgeState>().get_rarity_text(), "Exalted");
    session.click(95.0, 117.0);
    session.capture(&forge.ui, "item-rarity-menu");
    session.key(Key::Escape);
    assert_eq!(forge.ui.global::<ForgeState>().get_rarity_text(), "Exalted");
    drop(forge);
    let forge = Forge::open(session, World::ring(true), 1100, 700);
    session.click(155.0, 117.0);
    session.capture(&forge.ui, "item-implicits-base-picker");
    drop(forge);
    let forge = Forge::open(session, World::ring(false), 1100, 700);
    forge.handle.edit(|editor| {
        type_value(editor, Cell::Field(0), "60");
        editor.toggle_flag(2, true);
        editor.pick_base(1);
    });
    forge.settle();
    session.click(155.0, 117.0);
    session.capture(&forge.ui, "item-base-picker");
    drop(forge);

    // A base that would make affixes incompatible says how many in the base picker (`item-base-picker-incompatible`); Apply
    // refuses it whole until the picker allows it (`item-base-incompatible-refused`).
    let forge = Forge::open(session, World::class_ring(), 1100, 700);
    let options = forge.ui.global::<ForgeState>().get_base_options();
    assert_eq!(options.row_data(1).unwrap().detail, "Level 28 · 2 incompatible affixes");
    assert_eq!(options.row_data(2).unwrap().detail, "Level 40 · corrupted base · 1 incompatible affix");
    session.click(155.0, 117.0);
    session.capture(&forge.ui, "item-base-picker-incompatible");
    session.key(Key::Escape);
    forge.handle.edit(|editor| {
        editor.pick_base(1);
        editor.apply();
    });
    settle();
    assert_eq!(
        forge.ui.global::<ForgeState>().get_message(),
        "The new base would make affixes incompatible. Allow incompatible affixes in the base picker to apply it anyway."
    );
    assert_eq!(forge.writes(), 0);
    session.capture(&forge.ui, "item-base-incompatible-refused");
    drop(forge);

    // While a drafted identity holds the item, its flags wait: a checked one keeps its trim-gold frame, an unchecked one has
    // none (`item-status-disabled`).
    let forge = Forge::open(session, World::unique_ring().change(|item| item.status.corrupted = true), 1100, 700);
    forge.handle.edit(|editor| editor.pick_identity(2));
    settle();
    let fields = forge.ui.global::<ForgeState>().get_fields();
    let corruption = (0..fields.row_count())
        .map(|index| fields.row_data(index).unwrap())
        .find(|field| field.on_name == "Corrupted")
        .unwrap();
    assert!(corruption.checked && !corruption.can_edit);
    session.capture(&forge.ui, "item-status-disabled");
    drop(forge);

    // A unique's name opens the identity picker; a pick drafts the unique alone (`item-identity-picker-*`,
    // `item-identity-draft-*`).
    for (width, height) in [(1100, 700), (940, 580)] {
        let forge = Forge::open(session, World::unique_ring(), width, height);
        session.click(200.0, 84.0);
        session.capture(&forge.ui, &format!("item-identity-picker-{width}"));
        session.key(Key::Escape);
        forge.handle.edit(|editor| editor.pick_identity(2));
        settle();
        assert!(forge.ui.global::<ForgeState>().get_identity_drafted());
        session.capture(&forge.ui, &format!("item-identity-draft-{width}"));
    }

    // Keeping legendary potential on an identity that never holds it needs the override, with the risk in words
    // (`item-identity-override-*`).
    for (width, height) in [(1100, 700), (940, 580)] {
        let forge = Forge::open(session, World::unique_ring().change(|item| item.status.legendary_potential = 2), width, height);
        let state = forge.ui.global::<ForgeState>();
        let option = |name: &str| {
            (0..state.get_identity_options().row_count())
                .map(|index| state.get_identity_options().row_data(index).unwrap())
                .find(|option| option.name == name)
                .unwrap()
        };
        assert_eq!(option("Fixture Ember Unique").detail, "Unique · Gold Ring · legendary potential");
        assert!(option("Fixture Ember Unique").available);
        assert_eq!(
            option("Fixture Woven Unique").detail,
            "Unique · Gold Ring · Weaver's Will · keeps legendary potential it never holds, which may crash the game and reset the item state; needs \"I know what I'm doing\""
        );
        assert!(!option("Fixture Woven Unique").available);
        assert!(option("Fixture Set Ring").detail.contains("keeps legendary potential it never holds"));
        assert!(!option("Fixture Set Ring").available);
        session.click(200.0, 84.0);
        session.capture(&forge.ui, &format!("item-identity-override-{width}"));
        // The keyboard starts on the unique the item is and skips what can't be picked: Down twice stops at the Sapphire
        // unique (the Woven unique and the set ring can't be picked), Up twice reaches the Ember unique, and Enter drafts it.
        session.key(Key::DownArrow);
        session.key(Key::DownArrow);
        session.key(Key::UpArrow);
        session.key(Key::UpArrow);
        session.key(Key::Return);
        settle();
        assert!(state.get_identity_drafted());
        assert_eq!(state.get_item_name(), "Fixture Ember Unique", "Up from the Sapphire unique reaches the Ember unique");
    }

    // A click outside the affix picker cancels the replacement, so Apply comes back.
    let forge = Forge::open(session, World::boots(), 1100, 700);
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(0), "37"));
    session.click(NAME_X, row_y(3));
    assert!(forge.ui.global::<ForgeState>().get_picker_open());
    assert!(!forge.ui.global::<ForgeState>().get_can_apply());
    session.click(900.0, 500.0);
    assert!(!forge.ui.global::<ForgeState>().get_picker_open(), "a click outside cancels the picker");
    assert!(forge.ui.global::<ForgeState>().get_can_apply());
}
