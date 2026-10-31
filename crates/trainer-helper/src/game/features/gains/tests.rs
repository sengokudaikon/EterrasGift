use super::*;
use crate::hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering::SeqCst},
};

thread_local! {
    static OBSERVED: Cell<(i64, i64, i64, usize)> = const { Cell::new((0, 0, 0, 0)) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static THROW: Cell<bool> = const { Cell::new(false) };
}
const METHOD: usize = 0x1234abcd;
fn record(a: i64, b: i64, c: i64, method: usize) {
    assert_eq!(method, METHOD);
    OBSERVED.set((a, b, c, method));
    CALLS.set(CALLS.get() + 1);
    assert!(!THROW.get(), "original exception");
}
fn pair(first: f64, second: f64) -> u64 {
    GameplayHook::ExperienceMultiplier.value_kind().encode(&[first, second]).unwrap()
}
fn online() -> &'static AtomicU8 {
    Box::leak(Box::new(AtomicU8::new(0)))
}
fn gate(effect: LeafEffect, original: *const ()) -> LeafGate {
    // SAFETY: every test authors its exact target ABI and keeps its receiver/static graph live for all calls.
    unsafe {
        let gate = LeafGate::prepare_override(effect, None, None).unwrap();
        gate.set_original(original);
        gate.set_enabled(true);
        gate
    }
}
#[repr(C)]
struct Tracker {
    actor: usize,
    current: i64,
    next: i64,
}
extern "C-unwind" fn xp(_: *const Tracker, character: i64, skill: i64, favor: i64, method: usize) {
    record(character, skill, favor, method);
}
fn xp_layout(online: &AtomicU8, player: &AtomicUsize) -> experience_multiplier::Layout {
    experience_multiplier::Layout {
        online: NonNull::from(online).cast(),
        player: NonNull::from(player).cast(),
        actor: offset_of!(Tracker, actor) as u32,
        current: offset_of!(Tracker, current) as u32,
        next: offset_of!(Tracker, next) as u32,
    }
}

#[test]
fn experience_is_player_only_rounds_caps_and_keeps_skill_multiplier_when_level_wins() {
    let online = online();
    let player = Box::leak(Box::new(AtomicUsize::new(42)));
    let gate = gate(xp_layout(online, player).effect(), xp as *const ());
    // SAFETY: the leaf only transforms this exact five-position void ABI.
    let call: extern "C-unwind" fn(*const Tracker, i64, i64, i64, usize) = unsafe { std::mem::transmute(gate.entry()) };
    let mut tracker = Tracker {
        actor: 42,
        current: 90,
        next: 100,
    };
    let settings = match experience_multiplier::FEATURE.behavior {
        registry::Behavior::SharedLeaf(settings) => settings,
        _ => unreachable!(),
    };
    let hit = |tracker: *const Tracker, character, skill| {
        let before = CALLS.get();
        call(tracker, character, skill, 91, METHOD);
        assert_eq!(CALLS.get(), before + 1);
        let (a, b, favor, _) = OBSERVED.get();
        assert_eq!(favor, 91);
        (a, b)
    };
    settings.set(&gate, GameplayHook::ExperienceMultiplier, true, pair(1.5, 2.5));
    assert_eq!(hit(&tracker, 3, 3), (5, 8));
    assert_eq!(hit(&tracker, 0, -3), (0, -3));
    settings.set(&gate, GameplayHook::ExperienceMultiplier, true, pair(100.0, 100.0));
    assert_eq!(hit(&tracker, i32::MAX as i64 - 1, i32::MAX as i64), (i32::MAX as i64, i32::MAX as i64));
    assert_eq!(hit(&tracker, i64::MAX, i64::MAX - 1), (i64::MAX, i64::MAX - 1));
    settings.set(&gate, GameplayHook::ExperienceMultiplier, true, pair(100.0, 2.5));
    settings.set(&gate, GameplayHook::LevelUpPerGain, true, 1);
    assert_eq!(hit(&tracker, 1, 3), (10, 8));
    assert_eq!(hit(&tracker, 20, 0), (20, 0));
    assert_eq!(hit(&tracker, 0, 3), (0, 8));
    tracker.current = i64::MIN;
    tracker.next = i64::MAX;
    assert_eq!(hit(&tracker, 1, 3), (1, 8), "invalid overflowing requirement forwards character XP");
    tracker.current = 90;
    tracker.next = 100;
    settings.set(&gate, GameplayHook::ExperienceMultiplier, false, pair(100.0, 2.5));
    assert_eq!(hit(&tracker, 1, 3), (10, 3));
    settings.set(&gate, GameplayHook::LevelUpPerGain, false, 1);
    assert_eq!(hit(&tracker, 1, 3), (1, 3));
    settings.set(&gate, GameplayHook::ExperienceMultiplier, true, pair(2.0, 3.0));
    tracker.actor = 43;
    assert_eq!(hit(&tracker, 1, 3), (1, 3));
    player.store(43, SeqCst);
    assert_eq!(hit(&tracker, 1, 3), (2, 9));
    player.store(0, SeqCst);
    assert_eq!(hit(&tracker, 1, 3), (1, 3));
    player.store(43, SeqCst);
    online.store(1, SeqCst);
    assert_eq!(hit(&tracker, 1, 3), (1, 3));
    online.store(0, SeqCst);
    assert_eq!(hit(std::ptr::null(), 1, 3), (1, 3));
    gate.set_enabled(false);
    assert_eq!(hit(&tracker, 1, 3), (1, 3));
    gate.set_enabled(true);
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(|| call(&tracker, 1, 3, 91, METHOD)).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1, "an original exception is never retried");
    player.store(0, SeqCst);
}

#[test]
fn experience_reads_one_complete_pair_while_settings_change() {
    let online = online();
    let player = Box::leak(Box::new(AtomicUsize::new(42)));
    let gate = gate(xp_layout(online, player).effect(), xp as *const ());
    gate.set_value(pair(2.0, 3.0));
    // SAFETY: exact authored five-position ABI; all storage outlives the scoped threads.
    let call: extern "C-unwind" fn(*const Tracker, i64, i64, i64, usize) = unsafe { std::mem::transmute(gate.entry()) };
    std::thread::scope(|scope| {
        let gate = &gate;
        scope.spawn(move || {
            for i in 0..10000 {
                gate.set_value(if i % 2 == 0 { pair(2.0, 3.0) } else { pair(5.0, 7.0) });
            }
        });
        for _ in 0..3 {
            scope.spawn(move || {
                let tracker = Tracker { actor: 42, current: 0, next: 100 };
                for _ in 0..10000 {
                    call(&tracker, 1, 1, 91, METHOD);
                    assert!(matches!(OBSERVED.get(), (2, 3, 91, METHOD) | (5, 7, 91, METHOD)));
                }
            });
        }
    });
    gate.set_enabled(false);
    player.store(0, SeqCst);
}

extern "C-unwind" fn favor(_: usize, favor: i32, ignore_rep: bool, ignore_multiplier: bool, method: usize) {
    record(i64::from(favor), i64::from(ignore_rep), i64::from(ignore_multiplier), method);
}
#[test]
fn favor_respects_flags_and_nonpositive_gains_rounds_and_clamps_before_original() {
    let online = online();
    let gate = gate(faction_gains::effect(NonNull::from(online).cast()), favor as *const ());
    // SAFETY: exact instance, Int32, two Boolean, MethodInfo ABI, including MethodInfo on the stack.
    let call: extern "C-unwind" fn(usize, i32, bool, bool, usize) = unsafe { std::mem::transmute(gate.entry()) };
    for (on, network, receiver, amount, multiplier, ignore_rep, ignore_multiplier, expected) in [
        (true, 0, 1, 3, 1.5f64, false, false, 5),
        (true, 0, 1, 3, 2.5, true, false, 8),
        (true, 0, 1, 3, 2.5, false, true, 3),
        (true, 0, 1, 0, 2.5, true, false, 0),
        (true, 0, 1, -3, 2.5, false, false, -3),
        (true, 0, 1, 3, 0.0, false, false, 0),
        (true, 0, 1, 1, 0.49999999999999994, false, false, 0),
        (true, 0, 1, 1, 0.5, false, false, 1),
        (true, 0, 1, i32::MAX, 100.0, false, false, i32::MAX),
        (true, 0, 0, 3, 2.5, false, false, 3),
        (true, 1, 1, 3, 2.5, false, false, 3),
        (false, 0, 1, 3, 2.5, false, false, 3),
    ] {
        gate.set_value(multiplier.to_bits());
        gate.set_enabled(on);
        online.store(network, SeqCst);
        let before = CALLS.get();
        call(receiver, amount, ignore_rep, ignore_multiplier, METHOD);
        assert_eq!(CALLS.get(), before + 1);
        assert_eq!(OBSERVED.get(), (i64::from(expected), i64::from(ignore_rep), i64::from(ignore_multiplier), METHOD));
    }
}

#[repr(C)]
struct Rank {
    required: i32,
}
#[repr(C)]
struct Ranks {
    header: [usize; 3],
    len: usize,
    entries: [*const Rank; 2],
}
#[repr(C)]
struct Data {
    ranks: Cell<*const Ranks>,
}
#[repr(C)]
struct Faction {
    rank: i32,
    reputation: i32,
    data: *const Data,
}
extern "C-unwind" fn reputation(_: *const Faction, amount: i32, method: usize) {
    record(i64::from(amount), 0, 0, method);
}
#[test]
fn rank_replaces_even_a_larger_gain_but_preserves_invalid_missing_or_nonpositive_cases() {
    let online = online();
    let layout = rank_up_per_gain::Layout {
        online: NonNull::from(online).cast(),
        rank: offset_of!(Faction, rank) as u32,
        reputation: offset_of!(Faction, reputation) as u32,
        data: offset_of!(Faction, data) as u32,
        requirements: offset_of!(Data, ranks) as u32,
        required: offset_of!(Rank, required) as u32,
    };
    let gate = gate(layout.effect(), reputation as *const ());
    // SAFETY: exact three-position reputation ABI over the authored reference-array graph.
    let call: extern "C-unwind" fn(*const Faction, i32, usize) = unsafe { std::mem::transmute(gate.entry()) };
    let rank = Rank { required: 100 };
    let ranks = Ranks {
        header: [0; 3],
        len: 2,
        entries: [&rank, std::ptr::null()],
    };
    let data = Data { ranks: Cell::new(&ranks) };
    let mut faction = Faction {
        rank: 0,
        reputation: 90,
        data: &data,
    };
    let hit = |faction: *const Faction, amount| {
        let before = CALLS.get();
        call(faction, amount, METHOD);
        assert_eq!(CALLS.get(), before + 1);
        OBSERVED.get().0
    };
    assert_eq!(hit(&faction, 1), 10);
    assert_eq!(hit(&faction, 10000), 10);
    assert_eq!(hit(&faction, 0), 0);
    assert_eq!(hit(&faction, -1), -1);
    faction.rank = -1;
    assert_eq!(hit(&faction, 1), 1);
    faction.rank = 2;
    assert_eq!(hit(&faction, 1), 1);
    faction.rank = 1;
    assert_eq!(hit(&faction, 1), 1);
    faction.rank = 0;
    faction.reputation = 100;
    assert_eq!(hit(&faction, 1), 1);
    faction.reputation = i32::MIN;
    assert_eq!(hit(&faction, 1), 1, "overflowing remainder is not converted back to Int32");
    faction.reputation = 90;
    data.ranks.set(std::ptr::null());
    assert_eq!(hit(&faction, 1), 1);
    faction.data = std::ptr::null();
    assert_eq!(hit(&faction, 1), 1);
    faction.data = &data;
    data.ranks.set(&ranks);
    online.store(1, SeqCst);
    assert_eq!(hit(&faction, 1), 1);
    online.store(0, SeqCst);
    gate.set_enabled(false);
    assert_eq!(hit(&faction, 1), 1);
    gate.set_enabled(true);
    assert_eq!(hit(std::ptr::null(), 1), 1);
}
