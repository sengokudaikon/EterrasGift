//! The exact bytes of every gateway the features build. This code runs inside the game and most of it can't be run
//! here, so a change to how it is written must not change one byte of it: only a change to what a feature does may.
//!
//! Each gateway is prepared from fixed inputs, as `Group::prepare` prepares it, and neither published nor called. Its
//! two pages are compared with `golden.txt`, the addresses this run's allocations happened to get replaced by
//! placeholders: the code page (the gate's frame, the feature's code, the return path) and the data page (settings,
//! slots, the receiver class, the unwind entry). A feature's change is recorded with `EG_BLESS_GENERATED_CODE=1`.
//!
//! Not here: code patches, whose replacement bytes are constants in each feature's table rather than generated code,
//! and trampolines, which the detour crate writes per target (`libs/native-hooks/tests/hooks.rs` runs those).
use super::{
    any_waypoint, champions,
    damage::{DamageLayout, OffenseLayout},
    death_drops, duplicate_on_drop,
    encounter_counts::{self, Counts},
    eternity_cache_anywhere, experimental_uniques,
    gains::{experience_multiplier, faction_gains, rank_up_per_gain},
    morditas_timer, new_item_rarity,
    player_only::{Receiver, fixed_area, refill_mana, several_primordials},
    prophecy_any_distance, prophecy_uses,
    registry::{GameplayHook, Targets, definition},
    results::{ability_speed, boss_loot_chance, max_bonus_stability, max_random_rolls, movement_speed, no_mana_cost, one_echo_type, tier_7_affixes},
    reveal_map, reveal_objective, runes_glyphs_to_99, shards_to_99, skip_quest_echoes,
};
use native_hooks::{
    callback::CallbackGate,
    leaf::{LeafEffect, LeafGate},
    postfix::{PostfixEffect, PostfixGate},
    scalar::ScalarGate,
};
use std::{ffi::c_void, ptr::NonNull};

const GOLDEN: &str = include_str!("generated_code/golden.txt");
const HEADER: &str = "# Recorded by generated_code.rs: one gateway a line, its code page and its data page.\n";

const ONLINE: usize = 0x1111_0000_0010;
const PLAYER: usize = 0x2222_0000_0020;
const MANAGER: usize = 0x3333_0000_0030;
const CLASS: usize = 0x4444_0000_0040;
const RECEIVER: usize = 0x5555_0000_0050;
const CALLBACK: usize = 0x7777_0000_0070;
const CALLER: u64 = 0x7ff6_1234_5678;

/// What stands for the data page of this gateway, its scalar `original` word and the data page of its group's first gate.
const OWN_DATA: u64 = 0xdddd_dddd_dddd_dddd;
const OWN_ORIGINAL: u64 = 0xeeee_eeee_eeee_eeee;
const GROUP_DATA: u64 = 0xcccc_cccc_cccc_cccc;

fn at(address: usize) -> NonNull<c_void> {
    NonNull::new(address as *mut c_void).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The page without its trailing zeros, each address replaced by what stands for it.
fn page(start: *const u8, addresses: &[(u64, u64)]) -> String {
    // SAFETY: a gateway's two pages are readable and retained until the process exits, and nothing writes them here.
    let mut bytes = unsafe { std::slice::from_raw_parts(start, 4096) }.to_vec();
    bytes.truncate(bytes.iter().rposition(|byte| *byte != 0).map_or(0, |last| last + 1));
    for at in 0..bytes.len().saturating_sub(7) {
        let word = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        if let Some((_, placeholder)) = addresses.iter().find(|(address, _)| *address == word) {
            bytes[at..at + 8].copy_from_slice(&placeholder.to_le_bytes());
        }
    }
    hex(&bytes)
}

#[derive(Default)]
struct Recorded {
    lines: Vec<String>,
    covered: Vec<GameplayHook>,
}
impl Recorded {
    fn add(&mut self, name: &str, hooks: &[GameplayHook], entry: *const (), group: Option<*const ()>) {
        let code = entry.cast::<u8>();
        let own = entry as u64 + 4096;
        let mut addresses = vec![(own, OWN_DATA), (own + 48, OWN_ORIGINAL)];
        addresses.extend(group.map(|first| (first as u64 + 4096, GROUP_DATA)));
        // SAFETY: the data page follows the code page in the gateway's one allocation.
        let data = unsafe { code.add(4096) };
        self.lines.push(format!("{name} code={} data={}", page(code, &addresses), page(data, &addresses)));
        self.covered.extend_from_slice(hooks);
    }
    fn leaf(&mut self, name: &str, hooks: &[GameplayHook], effect: LeafEffect) {
        self.leaf_in(name, hooks, effect, None, None, None);
    }
    /// As `FeatureBinding::prepare`: every leaf of a group after the first shares the first's switch.
    fn leaf_in(&mut self, name: &str, hooks: &[GameplayHook], effect: LeafEffect, class: Option<NonNull<c_void>>, linked: Option<u32>, shared: Option<&LeafGate>) -> LeafGate {
        // SAFETY: prepared and read, never published or called, so no target, receiver or class is reached.
        let gate = unsafe { LeafGate::prepare_shared(effect, class, linked, shared) }.unwrap();
        self.add(name, hooks, gate.entry(), shared.map(LeafGate::entry));
        gate
    }
    fn postfix(&mut self, name: &str, hooks: &[GameplayHook], effect: PostfixEffect) {
        self.postfix_in(name, hooks, effect, None);
    }
    fn postfix_in(&mut self, name: &str, hooks: &[GameplayHook], effect: PostfixEffect, shared: Option<&PostfixGate>) -> PostfixGate {
        // SAFETY: prepared and read, never published or called.
        let gate = unsafe { PostfixGate::prepare(&effect, shared) }.unwrap();
        self.add(name, hooks, gate.entry(), shared.map(PostfixGate::entry));
        gate
    }
    fn scalar(&mut self, name: &str, hooks: &[GameplayHook], offset: u32, stack_argument: bool, shared: Option<&ScalarGate>) -> ScalarGate {
        // SAFETY: prepared and read, never published or called, so the receiver and online byte are never reached.
        let gate = unsafe { ScalarGate::prepare(RECEIVER, offset, ONLINE, 0x3f80_0000, stack_argument, shared) }.unwrap();
        self.add(name, hooks, gate.entry(), shared.map(ScalarGate::entry));
        gate
    }
}

fn record() -> Recorded {
    use GameplayHook::*;
    let (online, player, manager) = (at(ONLINE), at(PLAYER), at(MANAGER));
    let actor_component = |component| Receiver {
        online,
        storage: player,
        component,
    };
    let mut r = Recorded::default();

    r.leaf("mysterious_rift", &[MysteriousRift], LeafEffect::SelectableBoolean);
    let class = Some(at(CLASS));
    let first = r.leaf_in("any_offhand.placement", &[AnyOffhand], LeafEffect::Boolean(true), class, None, None);
    r.leaf_in("any_offhand.two_handed", &[AnyOffhand], LeafEffect::Boolean(false), None, None, Some(&first));
    r.leaf_in("any_offhand.linked_slot", &[AnyOffhand], LeafEffect::Boolean(false), class, Some(0x1a8), Some(&first));
    r.leaf_in("any_offhand.enforce", &[AnyOffhand], LeafEffect::Void, None, None, Some(&first));

    let damage = [IncomingDamage, EnemyDamage];
    let layout = DamageLayout {
        player_storage: player,
        online_storage: online,
        actor_offset: 0x38,
        offense: None,
    };
    r.leaf("damage.incoming_only", &damage, layout.effect());
    let offense = Some(OffenseLayout {
        current_health: 0x44,
        alignment_manager: 0x128,
        alignment: 0x30,
        alignment_id: 0x18,
        is_foe: 0x20,
    });
    r.leaf("damage", &damage, DamageLayout { offense, ..layout }.effect());

    let attack = &[FixedAttackRate];
    let first = r.scalar("fixed_attack_rate.calculate", attack, 0x2c4, false, None);
    r.scalar("fixed_attack_rate.main_hand", attack, 0x2c4, true, Some(&first));
    r.scalar("fixed_attack_rate.off_hand", attack, 0x2c4, false, Some(&first));
    r.scalar("fixed_leech_rate", &[FixedLeechRate], 0x1d0, false, None);

    r.leaf(
        "any_waypoint",
        &[AnyWaypoint],
        any_waypoint::Layout {
            online,
            scene: 0x118,
            no_waypoint: 0x24,
            active: 0x25,
        }
        .effect(),
    );
    r.leaf("champions", &[Champions], champions::effect(online, CALLER));
    r.leaf("experimental_uniques", &[ExperimentalUniques], experimental_uniques::effect(online));
    r.leaf("eternity_cache_anywhere", &[EternityCacheAnywhere], eternity_cache_anywhere::effect(actor_component(None)));
    r.leaf("new_item_rarity", &[NewItemRarity], new_item_rarity::effect(online));
    r.leaf("prophecy_any_distance", &[ProphecyAnyDistance], prophecy_any_distance::effect(online));
    r.postfix("prophecy_uses", &[ProphecyUses], prophecy_uses::effect(online, 0x24));
    let layout = morditas_timer::Layout {
        online,
        elapsed: 0x90,
        length: 0x94,
        state: 0x18,
        hunt: 1,
    };
    let first = r.postfix_in("morditas_timer.start", &[MorditasTimer], layout.effect(morditas_timer::Stage::Start), None);
    r.postfix_in("morditas_timer.tick", &[MorditasTimer], layout.effect(morditas_timer::Stage::Tick), Some(&first));
    r.postfix_in("morditas_timer.kill", &[MorditasTimer], layout.effect(morditas_timer::Stage::Kill), Some(&first));
    r.leaf(
        "skip_quest_echoes",
        &[SkipQuestEchoes],
        skip_quest_echoes::Layout {
            online,
            quests: 0x20,
            stability: 0x24,
            index: 0x28,
            timeline: 0x30,
            difficulties: 0x40,
            items: 0x10,
            size: 0x18,
            maximum: 0x1c,
        }
        .effect(),
    );
    r.leaf("reveal_map", &[RevealMap], reveal_map::effect(online));
    r.leaf("reveal_objective", &[RevealObjective], reveal_objective::effect(online, 0x13c));
    r.leaf(
        "runes_glyphs_to_99",
        &[RunesGlyphsTo99],
        runes_glyphs_to_99::SlotLayout {
            online,
            content: 0x48,
            data: 0x18,
            item_type: 0x10a,
            quantity: 0x2c,
        }
        .effect(),
    );
    r.leaf(
        "shards_to_99",
        &[ShardsTo99],
        shards_to_99::ShardLayout {
            online,
            stash: 0x68,
            shards: 0x70,
            items: 0x10,
            size: 0x18,
            quantity: 0x14,
        }
        .effect(),
    );
    r.leaf(
        "several_primordials",
        &[SeveralPrimordials],
        several_primordials::effect(Receiver {
            online,
            storage: manager,
            component: None,
        }),
    );
    r.leaf("fixed_area", &[FixedArea], fixed_area::effect(actor_component(Some(0x90))));
    r.leaf(
        "refill_mana",
        &[RefillMana],
        refill_mana::Layout {
            receiver: actor_component(Some(0x98)),
            maximum: 0x40,
            current: 0x44,
        }
        .effect(),
    );
    r.leaf(
        "experience_multiplier",
        &[ExperienceMultiplier, LevelUpPerGain],
        experience_multiplier::Layout {
            online,
            player,
            actor: 0x20,
            current: 0x28,
            next: 0x30,
        }
        .effect(),
    );
    r.leaf("faction_gains", &[FactionGains], faction_gains::effect(online));
    r.leaf(
        "rank_up_per_gain",
        &[RankUpPerGain],
        rank_up_per_gain::Layout {
            online,
            rank: 0x28,
            reputation: 0x2c,
            data: 0x18,
            requirements: 0x60,
            required: 0x10,
        }
        .effect(),
    );

    let counts = |start: u32| Counts {
        done: start,
        total: start + 4,
        remaining: start + 8,
        multiplier: start + 12,
    };
    r.postfix(
        "encounter_counts",
        &[LootLizards, RuneWardens],
        encounter_counts::Layout {
            online,
            context: 0x20,
            scene: 0x28,
            config: 0x10,
            rate: 0x54,
            counts: [counts(0x60), counts(0x80)],
        }
        .effect(),
    );
    r.postfix("boss_loot_chance", &[BossLootChance], boss_loot_chance::effect(online));
    r.postfix(
        "ability_speed",
        &[AbilitySpeed],
        ability_speed::Layout {
            online,
            player,
            controller: 0x58,
            actor: 0x60,
        }
        .effect(),
    );
    r.postfix("max_bonus_stability", &[MaxBonusStability], max_bonus_stability::effect(online, 0x7c));
    r.postfix("max_random_rolls", &[MaxRandomRolls], max_random_rolls::effect(online));
    r.postfix(
        "movement_speed",
        &[MovementSpeed],
        movement_speed::Layout {
            online,
            player,
            stats: 0x90,
            property: 0x2a,
            caller: CALLER,
        }
        .effect(),
    );
    r.postfix(
        "no_mana_cost",
        &[NoManaCost],
        no_mana_cost::ManaCostLayout {
            player_storage: player,
            online_storage: online,
            mana_offset: 0x98,
        }
        .effect(),
    );
    r.postfix("one_echo_type", &[OneEchoType], one_echo_type::effect(online));
    r.postfix("tier_7_affixes", &[Tier7Affixes], tier_7_affixes::effect(online));

    r.leaf(
        "death_drops",
        &[DropRates, ExtraRare],
        death_drops::Layout {
            online,
            override_item: 0x20,
            item_chance: 0x24,
            item_multiplier: 0x28,
            override_gold: 0x2c,
            gold_chance: 0x30,
            gold_multiplier: 0x34,
            extra_rare: 0x38,
        }
        .effect(),
    );

    r.leaf("duplicate_on_drop", &[DuplicateOnDrop], duplicate_on_drop::effect(online, at(MANAGER), CALLER, 0x2c));
    // SAFETY: prepared and read, never published or called, so the destination is never reached.
    let dispatcher = unsafe { CallbackGate::prepare(CALLBACK as *const ()) }.unwrap();
    r.add("dispatcher", &[], dispatcher.entry(), None);
    r
}

#[test]
fn every_gateway_is_byte_for_byte_what_was_recorded() {
    let actual = record().lines;
    if std::env::var_os("EG_BLESS_GENERATED_CODE").is_some() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/game/features/generated_code/golden.txt");
        std::fs::write(path, format!("{HEADER}{}\n", actual.join("\n"))).unwrap();
        return;
    }
    let expected: Vec<_> = GOLDEN.lines().filter(|line| !line.starts_with('#')).collect();
    let differing: Vec<_> = actual.iter().map(String::as_str).zip(&expected).filter(|(actual, expected)| actual != *expected).collect();
    assert!(differing.is_empty(), "generated code changed:\n{differing:#?}");
    assert_eq!(actual.len(), expected.len(), "gateways added or removed");
}

#[test]
fn every_feature_that_builds_a_gateway_is_recorded() {
    let covered = record().covered;
    for hook in GameplayHook::ALL {
        let builds = matches!(definition(hook).targets, Targets::Resolve(_));
        assert_eq!(covered.contains(&hook), builds, "{}", hook.id());
    }
}
