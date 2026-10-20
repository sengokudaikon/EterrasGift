//! The fixture game behind the Item Forge in captures and tests: `spec/forge-fixture.json` (the C# app's
//! `FakeGameSession`, `ShowcaseSession` and `SeedTierDataAsync`). Synthetic, not game data; never shipped. Writes are
//! accepted and read back as the edit's projection.
//!
//! Shared by the captures (`crates/ui/tests/captures.rs`) and the editor's unit tests (`src/forge/tests.rs`), which each
//! bring `forge` into scope before declaring this module.

#![allow(dead_code)]

use super::forge::game::{BaseDefinition, Catalog, ForgeGame, GameError, ItemLimits, ItemTypeDefinition, LegendaryType, UniqueDefinition};
use super::forge::game::{EditOutcome, WriteOutcome, refused};
use super::forge::item::EditPresentation;
use super::forge::item::{
    AffixDefinition, AffixKind, AffixState, Category, ForgeItem, ItemEdit, ItemStatus, ModifierState, RollRequest, RollTarget, Seal, TierProperty, UniqueState, ValueSample,
    ValueTable, format_tier_range,
};
use std::cell::RefCell;
use std::rc::Rc;

pub const BOOTS: u16 = 3;
pub const RING: u16 = 20;
pub const AMULET: u16 = 21;

/// What a capture can change behind the editor: the item the game holds, and tables it cannot read.
#[derive(Default)]
pub struct FixtureState {
    pub item: Option<ForgeItem>,
    /// Affix ids whose value tables the game cannot read ("Actual values unavailable: …").
    pub unmapped: Vec<u16>,
    /// The tables are the fixture's identity mapping (value = roll), as the C# `FakeGameSession`.
    pub identity: bool,
    pub unrestricted: bool,
    pub writes: Vec<ItemEdit>,
    history: eg_trainer::undo::ForgeUndoHistory,
}

pub struct FixtureGame {
    catalog: Catalog,
    pub state: Rc<RefCell<FixtureState>>,
}

impl FixtureGame {
    /// Authored display fixtures carry full canonical planner inputs. They do not
    /// implement compatibility, projection or edit validation themselves.
    fn bind(&self, mut item: ForgeItem) -> ForgeItem {
        use eg_trainer::{backend as native, catalog as data, edits::ExpectedObservation};
        let name = |text: &str| data::DefinitionName {
            text: text.into(),
            source: data::DefinitionNameSource::RuntimeDisplay,
        };
        let unavailable = || data::Availability::Unavailable {
            reason: "Authored fixture has no numeric catalog.".into(),
        };
        let pair = |(minimum, maximum)| data::ValueRange { minimum, maximum };
        let limits = self.catalog.limits.unwrap();
        let catalog = data::GameCatalog {
            identity: data::CatalogIdentity {
                binary_hash: "A".repeat(64),
                metadata_hash: "B".repeat(64),
                extraction_version: data::EXTRACTION_VERSION,
                locale: data::GameLocale {
                    language: "en".into(),
                    game_version: "fixture".into(),
                },
            },
            limits: data::ItemLimits {
                forging_potential: pair(limits.forging_potential),
                legendary_potential: pair(limits.legendary_potential),
                weavers_will: pair(limits.weavers_will),
                weavers_touch: pair(limits.weavers_touch),
                level_requirement: pair(limits.level_requirement),
            },
            bases: self
                .catalog
                .item_types
                .iter()
                .map(|t| {
                    (
                        t.item_type as u8,
                        data::ItemTypeDefinition {
                            item_type: t.item_type as u8,
                            name: name(&t.name),
                            sub_types: t
                                .bases
                                .iter()
                                .map(|b| data::SubTypeDefinition {
                                    sub_type: b.sub_type,
                                    name: name(&b.name),
                                    level_requirement: i32::from(b.level),
                                    class_requirement: 0,
                                    cannot_drop: false,
                                    is_corrupted: b.corrupted,
                                    implicits: data::Availability::Available(vec![]),
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),
            affixes: self
                .catalog
                .definitions
                .iter()
                .filter(|d| d.verified)
                .map(|d| {
                    (
                        d.id,
                        data::AffixDefinition {
                            id: d.id,
                            raw_name: d.name.clone(),
                            raw_display_name: d.name.clone(),
                            game_display_name: data::Availability::Available(d.name.clone()),
                            kind: d.kind as i32,
                            special: d.category.as_raw(),
                            class_requirement: 0,
                            item_types: Some(d.item_types.iter().map(|t| i32::from(*t)).collect()),
                            tier_count: u16::from(d.max_tier),
                            standard_effect: 1.0,
                            numeric: unavailable(),
                        },
                    )
                })
                .collect(),
            uniques: self
                .catalog
                .uniques
                .iter()
                .map(|u| {
                    (
                        u.id,
                        data::UniqueDefinition {
                            id: u.id,
                            name: name(&u.name),
                            item_type: u.item_type as u8,
                            sub_types: u.sub_types.iter().map(|t| *t as u8).collect(),
                            is_set_item: u.is_set,
                            set_id: 0,
                            legendary_type: i32::from(u.legendary_type == LegendaryType::WeaversWill),
                            can_drop_as_legendary: u.can_drop_as_legendary,
                            is_pre_corrupted: false,
                            numeric: data::Availability::Unavailable {
                                reason: "Authored fixture has no unique numeric catalog.".into(),
                            },
                        },
                    )
                })
                .collect(),
            rarities: Default::default(),
        };
        let status = item.status.unwrap_or_default();
        let mut snapshot = native::ForgeSnapshot {
            item: 1,
            individual_id: 1,
            fingerprint: 0,
            name: item.name.clone(),
            item_type: item.item_type as u8,
            sub_type: item.sub_type,
            rarity: item.rarity,
            class_requirement: 0,
            affixes: item
                .affixes
                .iter()
                .map(|a| native::AffixSnapshot {
                    slot: a.slot as u8,
                    id: a.id,
                    tier: u16::from(a.tier),
                    roll: a.roll,
                    kind: a.kind as i32,
                    seal: a.seal as i32,
                    special: a.category.as_raw(),
                    name: self.catalog.definition(a.id).name,
                })
                .collect(),
            implicit_rolls: item.implicit_rolls.clone(),
            identity: item
                .unique
                .as_ref()
                .or(item.set_item.as_ref())
                .map(|u| native::UniqueIdentity { id: u.id, rolls: u.rolls.clone() }),
            status: native::ItemStatus {
                forging_potential: status.forging_potential,
                forging_potential_type: 0,
                legendary_potential: status.legendary_potential,
                weavers_will: status.weavers_will,
                weavers_touch: status.weavers_touch,
                level_requirement: i32::from(status.level_requirement),
                corrupted: status.corrupted,
                ruined: status.ruined,
            },
        };
        snapshot.fingerprint = snapshot.legacy_fingerprint();
        let expected = ExpectedObservation::new(
            1,
            native::ProcessIdentity {
                pid: 1,
                created: 1,
                executable: std::env::current_exe().unwrap(),
            },
            catalog.identity.clone(),
            snapshot,
        )
        .unwrap();
        item.context = Some(std::sync::Arc::new(super::forge::item::ForgeContext {
            expected,
            catalog: std::sync::Arc::new(catalog),
        }));
        item
    }
    /// The showcase session on the fixture's boots: four affixes, value tables scaled by tier.
    pub fn boots(seed_tiers: bool) -> FixtureGame {
        FixtureGame::new(Some(boots()), seed_tiers, false, false)
    }

    /// The legendary showcase item with its unique modifiers and a shared roll.
    pub fn unique() -> FixtureGame {
        FixtureGame::new(Some(unique_boots()), false, false, false)
    }

    /// The ring of the C# header tests (`ItemHeaderTests`), or with implicits and bases that grant them (`ImplicitTests`);
    /// identity tables as `FakeGameSession`.
    pub fn ring(implicits: bool) -> FixtureGame {
        FixtureGame::new(Some(ring(implicits)), false, true, implicits)
    }

    /// A unique ring (`UniqueIdentityTests`): its name opens the identity picker.
    pub fn unique_ring() -> FixtureGame {
        let mut item = ring(false);
        item.token = "123:456:1122334455667788:01020304".into();
        item.name = "Fixture Potential Unique".into();
        item.rarity = 7;
        item.affixes.clear();
        item.unique = Some(UniqueState {
            id: 70,
            rolls: vec![100, 7],
            modifiers: Vec::new(),
            formatting: crate::forge::game::Lookup::Ready(()),
        });
        item.status = Some(ItemStatus::default());
        FixtureGame::new(Some(item), false, true, false)
    }

    pub fn new(item: Option<ForgeItem>, seed_tiers: bool, identity: bool, base_implicits: bool) -> FixtureGame {
        FixtureGame {
            catalog: catalog(seed_tiers, base_implicits),
            state: Rc::new(RefCell::new(FixtureState {
                item,
                identity,
                ..FixtureState::default()
            })),
        }
    }

    /// The game changes the item behind the editor (another player action, a reload).
    pub fn change(&self, change: impl FnOnce(&mut ForgeItem)) {
        if let Some(item) = self.state.borrow_mut().item.as_mut() {
            change(item);
        }
    }
}

fn unique_definition(id: u16, name: &str, item_type: u16, sub_types: &[u16], legendary_type: LegendaryType, is_set: bool, legendary: bool) -> UniqueDefinition {
    UniqueDefinition {
        id,
        name: name.into(),
        item_type,
        sub_types: sub_types.to_vec(),
        legendary_type,
        is_set,
        can_drop_as_legendary: legendary,
    }
}

fn definition(id: u16, name: &str, kind: AffixKind, category: Category) -> AffixDefinition {
    AffixDefinition {
        id,
        name: name.into(),
        kind,
        category,
        item_types: vec![BOOTS],
        max_tier: 8,
        verified: true,
    }
}

fn catalog(seed_tiers: bool, base_implicits: bool) -> Catalog {
    let mut catalog = Catalog {
        definitions: vec![
            definition(25, "Health", AffixKind::Suffix, Category::Standard),
            definition(1148, "Caretaker's Regrowth Reforged", AffixKind::Prefix, Category::Set),
            definition(4000, "Movement Speed", AffixKind::Prefix, Category::Standard),
            definition(4001, "Fire Resistance", AffixKind::Suffix, Category::Standard),
            definition(4002, "Cold Resistance", AffixKind::Suffix, Category::Standard),
            definition(4003, "Increased Armour", AffixKind::Prefix, Category::Standard),
            definition(4004, "Sentinel's Resolve Reforged", AffixKind::Prefix, Category::Set),
        ],
        rarity_names: ["Normal", "Magic", "Magic", "Rare", "Rare", "", "", "Unique", "Set", "Legendary"]
            .map(String::from)
            .to_vec(),
        limits: Some(ItemLimits {
            forging_potential: (0, 255),
            legendary_potential: (0, 4),
            weavers_will: (5, 28),
            weavers_touch: (5, 14),
            level_requirement: (0, 100),
        }),
        item_types: vec![ItemTypeDefinition {
            item_type: RING,
            name: "Ring".into(),
            bases: vec![
                BaseDefinition {
                    sub_type: 2,
                    name: "Gold Ring".into(),
                    level: 20,
                    corrupted: false,
                    implicits: if base_implicits {
                        vec!["+10–16 Armor".into(), "+5% Cold Resistance".into()]
                    } else {
                        Vec::new()
                    },
                },
                BaseDefinition {
                    sub_type: 5,
                    name: "Sapphire Ring".into(),
                    level: 28,
                    corrupted: false,
                    implicits: if base_implicits { vec!["+12–18% Fire Resistance".into()] } else { Vec::new() },
                },
                BaseDefinition {
                    sub_type: 9,
                    name: "Obsidian Ring".into(),
                    level: 40,
                    corrupted: true,
                    implicits: Vec::new(),
                },
            ],
        }],
        uniques: vec![
            unique_definition(77, "Wayfarer's Boots", BOOTS, &[0], LegendaryType::LegendaryPotential, false, true),
            // The ring uniques of the C# `UniqueIdentityTests`: one on another base and never legendary, one with Weaver's
            // Will, a set item; the amulet's must not be offered.
            unique_definition(70, "Fixture Potential Unique", RING, &[2], LegendaryType::LegendaryPotential, false, false),
            unique_definition(71, "Fixture Woven Unique", RING, &[2, 5], LegendaryType::WeaversWill, false, true),
            unique_definition(72, "Fixture Set Ring", RING, &[2], LegendaryType::LegendaryPotential, true, false),
            unique_definition(73, "Fixture Sapphire Unique", RING, &[5], LegendaryType::LegendaryPotential, false, false),
            unique_definition(74, "Fixture Ember Unique", RING, &[2], LegendaryType::LegendaryPotential, false, true),
            unique_definition(80, "Fixture Amulet Unique", AMULET, &[0], LegendaryType::LegendaryPotential, false, false),
        ],
        ..Catalog::default()
    };
    for definition in &mut catalog.definitions {
        // The ring fixtures roll Health too.
        if definition.id == 25 {
            definition.item_types.push(RING);
        }
    }
    if seed_tiers {
        let health = [
            (5.0, 15.0),
            (16.0, 25.0),
            (26.0, 40.0),
            (41.0, 60.0),
            (61.0, 90.0),
            (100.0, 130.0),
            (131.0, 180.0),
            (275.0, 325.0),
        ];
        for tier in 1..=8u8 {
            let t = f64::from(tier);
            let (min, max) = health[tier as usize - 1];
            let ranges = [
                (25, TierProperty::Flat { min, max, whole: true }),
                (
                    1148,
                    TierProperty::Increased {
                        min: 0.02 * t,
                        max: 0.02 * t + 0.03,
                    },
                ),
                (
                    4000,
                    TierProperty::Increased {
                        min: 0.02 + 0.01 * t,
                        max: 0.04 + 0.01 * t,
                    },
                ),
                (
                    4001,
                    TierProperty::AddedPercent {
                        min: 0.05 * t,
                        max: 0.05 * t + 0.04,
                    },
                ),
                (
                    4002,
                    TierProperty::AddedPercent {
                        min: 0.05 * t,
                        max: 0.05 * t + 0.04,
                    },
                ),
                (
                    4003,
                    TierProperty::Flat {
                        min: 10.0 * t,
                        max: 10.0 * t + 15.0,
                        whole: true,
                    },
                ),
                (
                    4004,
                    TierProperty::Flat {
                        min: 2.0 * t,
                        max: 2.0 * t + 3.0,
                        whole: true,
                    },
                ),
            ];
            for (id, property) in ranges {
                if let Some(range) = format_tier_range(&[property]) {
                    catalog.tier_ranges.insert((id, tier), range);
                }
            }
        }
    }
    catalog
}

fn affix(slot: usize, id: u16, tier: u8, roll: u8, kind: AffixKind, seal: Seal, category: Category) -> AffixState {
    AffixState {
        slot,
        id,
        tier,
        roll,
        kind,
        seal,
        category,
    }
}

pub fn boots() -> ForgeItem {
    ForgeItem {
        context: None,
        token: "test-item".into(),
        name: "Wayfarer's Boots".into(),
        item_type: BOOTS,
        sub_type: 0,
        idol: false,
        rarity: 0,
        affixes: vec![
            affix(0, 25, 5, 170, AffixKind::Suffix, Seal::Normal, Category::Standard),
            affix(1, 1148, 3, 200, AffixKind::Prefix, Seal::Normal, Category::Set),
            affix(2, 4000, 1, 100, AffixKind::Prefix, Seal::Sealed, Category::Standard),
            affix(3, 4001, 4, 230, AffixKind::Suffix, Seal::Corrupted, Category::Standard),
        ],
        unique: None,
        set_item: None,
        implicits: None,
        implicit_rolls: Vec::new(),
        status: None,
    }
}

fn modifier(index: usize, text: &str, range: &str, roll_ids: &[usize], editable: bool) -> ModifierState {
    ModifierState {
        index,
        text: text.into(),
        range: range.into(),
        roll_ids: roll_ids.to_vec(),
        editable,
    }
}

/// The unique's lines as the fixture's game formats them from the rolls.
fn format_unique(rolls: &[u8], modifiers: &[ModifierState]) -> Vec<ModifierState> {
    let speed = (10.0 + 15.0 * f64::from(rolls[0]) / 255.0).round_ties_even();
    let void = (30.0 + 30.0 * f64::from(rolls[1]) / 255.0).round_ties_even();
    modifiers
        .iter()
        .map(|modifier| ModifierState {
            text: match modifier.index {
                0 => format!("+{speed}% Movement Speed"),
                1 => format!("{void}% increased Void Damage over Time"),
                2 => format!("Your minions gain {speed}% Movement Speed"),
                _ => modifier.text.clone(),
            },
            ..modifier.clone()
        })
        .collect()
}

pub fn unique_boots() -> ForgeItem {
    let rolls = vec![140, 200, 50];
    let modifiers = vec![
        modifier(0, "", "10% to 25%", &[0], true),
        modifier(1, "", "30% to 60%", &[1], true),
        modifier(2, "", "", &[0], true),
        modifier(3, "Cannot be slowed below base movement speed", "", &[], true),
        modifier(4, "Unresolved modifier", "", &[2], false),
    ];
    ForgeItem {
        context: None,
        token: "showcase-unique".into(),
        name: "Wayfarer's Boots".into(),
        item_type: BOOTS,
        sub_type: 0,
        idol: false,
        rarity: 9,
        affixes: vec![affix(0, 25, 7, 170, AffixKind::Suffix, Seal::Normal, Category::Standard)],
        set_item: None,
        unique: Some(UniqueState {
            id: 77,
            modifiers: format_unique(&rolls, &modifiers),
            rolls,
            formatting: crate::forge::game::Lookup::Ready(()),
        }),
        implicits: None,
        implicit_rolls: vec![80, 90, 100],
        status: None,
    }
}

pub fn ring(implicits: bool) -> ForgeItem {
    ForgeItem {
        context: None,
        token: "ring".into(),
        name: "Gold Ring".into(),
        item_type: RING,
        sub_type: 2,
        idol: false,
        rarity: 3,
        affixes: vec![affix(0, 25, 5, 170, AffixKind::Suffix, Seal::Normal, Category::Standard)],
        unique: None,
        set_item: None,
        implicits: implicits.then(|| vec![modifier(0, "+14 Armor", "(10 to 16)", &[0], true), modifier(1, "+5% Cold Resistance", "", &[], true)]),
        implicit_rolls: if implicits { vec![128, 0] } else { vec![128] },
        status: Some(ItemStatus {
            forging_potential: 30,
            level_requirement: 24,
            ..ItemStatus::default()
        }),
    }
}

/// value(roll) = round(min + (max - min) * roll / 255, decimals), half to even as .NET rounds.
fn table(label: &str, min: f64, max: f64, decimals: i32, format: &str, effect: &str) -> ValueTable {
    let scale = 10f64.powi(decimals);
    ValueTable {
        label: label.into(),
        samples: (0..256)
            .map(|roll| {
                let value = ((min + (max - min) * f64::from(roll) / 255.0) * scale).round_ties_even() / scale;
                let shown = format!("{value:.*}", decimals as usize);
                ValueSample {
                    value,
                    display: format.replace("{v}", &shown),
                    effects: if effect.is_empty() { Vec::new() } else { vec![effect.replace("{v}", &shown)] },
                }
            })
            .collect(),
    }
}

fn identity() -> ValueTable {
    ValueTable {
        label: "Fixture value".into(),
        samples: (0..256)
            .map(|roll| ValueSample {
                value: f64::from(roll),
                display: roll.to_string(),
                effects: vec![format!("Fixture effect {roll}")],
            })
            .collect(),
    }
}

impl ForgeGame for FixtureGame {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    fn read(&self) -> Option<ForgeItem> {
        self.state.borrow().item.clone().map(|item| self.bind(item))
    }

    fn unrestricted(&self) -> bool {
        self.state.borrow().unrestricted
    }

    fn resolve(&self, item: &ForgeItem, request: &RollRequest) -> crate::forge::game::Lookup<ValueTable> {
        let result: Result<ValueTable, GameError> = (|| {
            let state = self.state.borrow();
            if state.identity {
                return Ok(identity());
            }
            match request.target {
                RollTarget::Unique => Ok(match request.index {
                    0 => table("Increased movement speed", 10.0, 25.0, 0, "{v}%", "{v}% increased Movement Speed"),
                    1 => table("Void damage over time", 30.0, 60.0, 0, "{v}%", "{v}% increased Void Damage over Time"),
                    _ => table("Value", 1.0, 5.0, 0, "{v}", ""),
                }),
                RollTarget::Implicit => Ok(table("Value", 1.0, 10.0, 0, "{v}", "")),
                RollTarget::Affix => {
                    let (id, tier) = match request.draft {
                        Some(ItemEdit::Affix(eg_trainer::edits::AffixEdit { id, tier, .. })) => (id as u16, Some(tier as u8)),
                        _ => item.affix(request.index).map_or((0, None), |affix| (affix.id, Some(affix.tier))),
                    };
                    if state.unmapped.contains(&id) {
                        return Err("Actual values are unavailable for this affix.".into());
                    }
                    let (label, min, max, decimals, format, effect) = match id {
                        25 => ("Added health", 20.0, 36.0, 0, "+{v}", "+{v} Health"),
                        1148 => ("Health regeneration", 5.0, 9.0, 1, "{v}%", "{v}% increased Health Regen"),
                        4000 => ("Increased movement speed", 4.0, 8.0, 0, "{v}%", "{v}% increased Movement Speed"),
                        4001 => ("Fire resistance", 15.0, 25.0, 0, "+{v}%", "+{v}% Fire Resistance"),
                        _ => ("Value", 1.0, 10.0, 0, "{v}", ""),
                    };
                    // Synthetic tier scaling so a tier draft changes the value, as tiers do in the game.
                    let scale = tier.map_or(1.0, |tier| 0.6 + 0.1 * f64::from(tier));
                    Ok(table(label, min * scale, max * scale, decimals, format, effect))
                }
            }
        })();
        result.into()
    }

    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        self.apply_edit(expected, edit, false)
    }
    fn begin_batch(&mut self) -> Result<(), GameError> {
        self.state.borrow_mut().history.begin_batch();
        Ok(())
    }
    fn undo_depth(&self) -> usize {
        self.state.borrow().history.len()
    }
    fn undo_needs_override(&self) -> bool {
        self.state.borrow().history.needs_override()
    }
    fn undo_intent(&self) -> Option<ItemEdit> {
        let item = self.read()?;
        let context = item.context.as_ref()?;
        let state = self.state.borrow();
        let undo = state.history.current()?;
        if undo.after() != &context.expected {
            return None;
        }
        match undo.prepare_with_override(
            &context.expected,
            1,
            &context.catalog,
            &eg_trainer::edits::CatalogProvenance::RuntimeRead {
                generation: 1,
                process: context.expected.process().clone(),
            },
            state.unrestricted,
        ) {
            Ok(plan) => Some(plan.edit_intent().clone()),
            Err(_) if matches!(undo.reverse_edit(), ItemEdit::Roll(e) if e.target != RollTarget::Affix) => Some(undo.reverse_edit().clone()),
            Err(_) => None,
        }
    }
    fn undo(&mut self, expected: &ForgeItem) -> EditOutcome {
        let Some(edit) = self.undo_intent() else {
            return refused("Undo is unavailable for the current item.");
        };
        self.apply_edit(expected, &edit, true)
    }

    fn preview_unique(&self, expected: &ForgeItem, edit: &ItemEdit) -> crate::forge::game::Lookup<Vec<ModifierState>> {
        let result: Result<Vec<ModifierState>, GameError> = (|| {
            let after = edit.project(expected).map_err(|e| e.to_string())?;
            let unique = after.unique.ok_or_else(|| "Preview unavailable.".to_string())?;
            Ok(format_unique(&unique.rolls, &unique.modifiers))
        })();
        result.into()
    }
}

impl FixtureGame {
    fn apply_edit(&mut self, expected: &ForgeItem, edit: &ItemEdit, undoing: bool) -> EditOutcome {
        let Some(item) = self.read() else {
            return refused("Place an item in the forge before editing.");
        };
        let mut state = self.state.borrow_mut();
        if !item.same_state(expected) {
            return refused("Item changed. Reset edit fields before trying again.");
        }
        let prepared = match edit.prepare_view_with_override(&item, state.unrestricted) {
            Ok(prepared) => prepared,
            Err(e) => return refused(e.to_string()),
        };
        let mut after = match item.with_snapshot(prepared.projected().clone()) {
            Ok(after) => after,
            Err(e) => return refused(e.to_string()),
        };
        // The game names the item after its new identity.
        if let ItemEdit::UniqueIdentity(eg_trainer::edits::UniqueIdentityEdit { unique_id, .. }) = edit
            && let Some(unique) = self.catalog.unique(*unique_id as u16)
        {
            after.name = unique.name.clone();
        }
        if let Some(unique) = after.unique.as_mut() {
            unique.modifiers = format_unique(&unique.rolls, &unique.modifiers);
        }
        state.writes.push(edit.clone());
        let after = self.bind(after);
        let _ = state.history.confirmed(&prepared, 1, after.context.as_ref().unwrap().expected.snapshot(), undoing);
        if let Some(undo) = state.history.current() {
            let reverse = undo.reverse_edit();
            let regular = !matches!(reverse, ItemEdit::Roll(e) if e.target != RollTarget::Affix);
            if regular
                && undo
                    .prepare_with_override(
                        undo.after(),
                        1,
                        &after.context.as_ref().unwrap().catalog,
                        &eg_trainer::edits::CatalogProvenance::RuntimeRead {
                            generation: 1,
                            process: undo.after().process().clone(),
                        },
                        state.unrestricted,
                    )
                    .is_err()
            {
                state.history.clear();
            }
        }
        state.item = Some(after.clone());
        WriteOutcome::Confirmed(Some(after))
    }
}
