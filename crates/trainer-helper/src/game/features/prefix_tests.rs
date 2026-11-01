use super::{new_item_rarity, prophecy_any_distance, reveal_map, reveal_objective};
use crate::game::*;
use native_hooks::leaf::{LeafEffect, LeafGate};
use std::{
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

fn online() -> &'static AtomicU8 {
    Box::leak(Box::new(AtomicU8::new(0)))
}
fn gate(effect: LeafEffect, original: *const ()) -> LeafGate {
    // SAFETY: callers supply the exact authored leaf/original ABI; its online byte is retained.
    let gate = unsafe { LeafGate::prepare_override(effect, None, None).unwrap() };
    // SAFETY: configured before any call or publication.
    unsafe {
        gate.set_original(original);
    }
    gate
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vector {
    x: f32,
    y: f32,
    z: f32,
}
const POSITION: Vector = Vector { x: -12.5, y: 7.25, z: 4000.0 };
const METHOD: usize = 0x1234abcdef;
#[derive(Default)]
struct MapProbe {
    calls: usize,
    radius: f32,
}
extern "C-unwind" fn map_original(probe: &mut MapProbe, position: Vector, radius: f32, method: usize) {
    assert_eq!(position, POSITION);
    assert_eq!(method, METHOD);
    probe.calls += 1;
    probe.radius = radius;
}
#[test]
fn map_keeps_the_indirect_vector_and_only_raises_radius_while_enabled_offline() {
    assert_eq!(size_of::<Vector>(), 12);
    let online = online();
    let gate = gate(reveal_map::effect(NonNull::from(online).cast()), map_original as *const ());
    // SAFETY: exact Windows x64 ABI, including the by-value 12-byte aggregate passed indirectly.
    let call: extern "C-unwind" fn(&mut MapProbe, Vector, f32, usize) = unsafe { std::mem::transmute(gate.entry()) };
    let mut probe = MapProbe::default();
    for (on, network, radius, expected) in [(false, 0, 3.5, 3.5), (true, 0, 3.5, 1000.0), (true, 0, 2000.0, 2000.0), (true, 1, 3.5, 3.5)] {
        gate.set_enabled(on);
        online.store(network, SeqCst);
        let before = probe.calls;
        call(&mut probe, POSITION, radius, METHOD);
        assert_eq!(probe.radius, expected);
        assert_eq!(probe.calls, before + 1);
    }
    online.store(0, SeqCst);
    let nan = f32::from_bits(0x7fc01234);
    call(&mut probe, POSITION, nan, METHOD);
    assert_eq!(probe.radius.to_bits(), nan.to_bits());
}

#[repr(C)]
#[derive(Default)]
struct Pulse {
    threshold: f32,
    score: f32,
    calls: usize,
}
extern "C-unwind" fn pulse_original(pulse: *mut Pulse, score: f32, last_kill: usize, method: usize) {
    assert_eq!((last_kill, method), (0x54321, METHOD));
    // SAFETY: authored receiver, or null to verify the hook's null forwarding.
    if let Some(pulse) = unsafe { pulse.as_mut() } {
        pulse.score = score;
        pulse.calls += 1;
    }
}
#[test]
fn objective_uses_the_current_threshold_and_preserves_last_kill_and_original_trigger() {
    let online = online();
    let gate = gate(
        reveal_objective::effect(NonNull::from(online).cast(), offset_of!(Pulse, threshold) as u32),
        pulse_original as *const (),
    );
    // SAFETY: authored four-position void signature and threshold field.
    let call: extern "C-unwind" fn(*mut Pulse, f32, usize, usize) = unsafe { std::mem::transmute(gate.entry()) };
    let mut pulse = Pulse {
        threshold: 50.0,
        ..Pulse::default()
    };
    for (on, network, score, threshold, expected) in [
        (false, 0, 1.0, 50.0, 1.0),
        (true, 0, 1.0, 50.0, 50.0),
        (true, 0, 80.0, 50.0, 80.0),
        (true, 0, 1.0, 90.0, 90.0),
        (true, 1, 1.0, 90.0, 1.0),
    ] {
        pulse.threshold = threshold;
        gate.set_enabled(on);
        online.store(network, SeqCst);
        let before = pulse.calls;
        call(&mut pulse, score, 0x54321, METHOD);
        assert_eq!((pulse.score, pulse.calls), (expected, before + 1));
        assert_eq!(pulse.threshold, threshold, "only the argument changes");
    }
    online.store(0, SeqCst);
    call(std::ptr::null_mut(), 1.0, 0x54321, METHOD);
    pulse.threshold = f32::NAN;
    call(&mut pulse, 2.0, 0x54321, METHOD);
    assert_eq!(pulse.score, 2.0);
    pulse.threshold = 0.0;
    call(&mut pulse, -0.0, 0x54321, METHOD);
    assert_eq!(pulse.score.to_bits(), (-0.0f32).to_bits());
}

extern "C-unwind" fn range_original(calls: &mut usize, position: Vector, method: usize) -> bool {
    assert_eq!(position, POSITION);
    assert_eq!(method, METHOD);
    *calls += 1;
    false
}
#[test]
fn prophecy_distance_skips_only_its_range_query_and_off_paths_keep_the_vector() {
    let online = online();
    let gate = gate(prophecy_any_distance::effect(NonNull::from(online).cast()), range_original as *const ());
    // SAFETY: Boolean-return Windows x64 signature with one indirect Vector3 and MethodInfo in R8.
    let call: extern "C-unwind" fn(&mut usize, Vector, usize) -> bool = unsafe { std::mem::transmute(gate.entry()) };
    let mut calls = 0;
    for (on, network, expected) in [(false, 0, false), (true, 0, true), (true, 1, false), (false, 0, false)] {
        gate.set_enabled(on);
        online.store(network, SeqCst);
        let before = calls;
        assert_eq!(call(&mut calls, POSITION, METHOD), expected);
        assert_eq!(calls, before + usize::from(!expected));
    }
}

type RarityCall =
    extern "C-unwind" fn(&mut RarityProbe, *mut usize, *mut bool, *mut usize, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64);
#[derive(Default)]
struct RarityProbe {
    calls: usize,
    args: [u64; 22],
    panic: bool,
}
#[allow(clippy::too_many_arguments)]
extern "C-unwind" fn rarity_original(
    probe: &mut RarityProbe,
    item: *mut usize,
    improved: *mut bool,
    pre_corrupt: *mut usize,
    ilvl: u64,
    class: u64,
    base: u64,
    sub: u64,
    rarity: u64,
    tags: u64,
    sockets: u64,
    unique: u64,
    reroll: u64,
    exalted: u64,
    lp: u64,
    require_legendary: u64,
    legendary: u64,
    untraded: u64,
    replacement: u64,
    corruption: u64,
    effectiveness: u64,
    modifiers: u64,
    method: u64,
) {
    probe.calls += 1;
    probe.args = [
        item as u64,
        improved as u64,
        pre_corrupt as u64,
        ilvl,
        class,
        base,
        sub,
        rarity,
        tags,
        sockets,
        unique,
        reroll,
        exalted,
        lp,
        require_legendary,
        legendary,
        untraded,
        replacement,
        corruption,
        effectiveness,
        modifiers,
        method,
    ];
    assert!(!probe.panic, "original unwind");
    // SAFETY: the caller supplies separate writable out/ref storage.
    unsafe {
        *item += 1;
        *improved = true;
        *pre_corrupt ^= 0xff;
    }
}
fn roll(call: RarityCall, probe: &mut RarityProbe, rarity: u64) {
    let (mut item, mut improved, mut corrupt) = (100, false, 0x1234);
    call(
        probe,
        &mut item,
        &mut improved,
        &mut corrupt,
        4,
        5,
        6,
        7,
        rarity,
        9,
        10,
        11,
        0x1234567887654321,
        13,
        14,
        15,
        16,
        17,
        0x1122334455667788,
        0xaabbccddeeff1122,
        0x9988776655443322,
        21,
        METHOD as u64,
    );
    assert_eq!((item, improved, corrupt), (101, true, 0x12cb));
    assert_eq!(
        &probe.args[..3],
        &[&item as *const usize as u64, &improved as *const bool as u64, &corrupt as *const usize as u64]
    );
    assert_eq!(&probe.args[3..7], &[4, 5, 6, 7]);
    assert_eq!(
        &probe.args[8..],
        &[
            9,
            10,
            11,
            0x1234567887654321,
            13,
            14,
            15,
            16,
            17,
            0x1122334455667788,
            0xaabbccddeeff1122,
            0x9988776655443322,
            21,
            METHOD as u64
        ]
    );
}
#[test]
fn rarity_changes_only_unspecified_rarity_and_preserves_all_twenty_three_positions_and_outputs() {
    let online = online();
    let gate = gate(new_item_rarity::effect(NonNull::from(online).cast()), rarity_original as *const ());
    // SAFETY: exact 23-position void native signature; opaque eight-byte payloads are deliberately distinct.
    let call: RarityCall = unsafe { std::mem::transmute(gate.entry()) };
    let mut probe = RarityProbe::default();
    let unspecified = 0xabcdef01ffffffff;
    gate.set_enabled(true);
    for choice in [0, 1, 3, 7, 8] {
        gate.set_value(choice);
        roll(call, &mut probe, unspecified);
        assert_eq!(probe.args[7], 0xabcdef0100000000 | choice);
        for explicit in [0, 1, 3, 7, 8, 10] {
            roll(call, &mut probe, explicit);
            assert_eq!(probe.args[7], explicit);
        }
    }
    gate.set_enabled(false);
    roll(call, &mut probe, unspecified);
    assert_eq!(probe.args[7], unspecified);
    gate.set_enabled(true);
    online.store(1, SeqCst);
    roll(call, &mut probe, unspecified);
    assert_eq!(probe.args[7], unspecified);
    online.store(0, SeqCst);
    probe.panic = true;
    let before = probe.calls;
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| roll(call, &mut probe, unspecified))).is_err());
    assert_eq!(probe.calls, before + 1, "unwinding original is never retried");
}
