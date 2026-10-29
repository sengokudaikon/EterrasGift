use super::*;
use crate::hooks::postfix::PostfixGate;
use std::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering::SeqCst},
};

#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    value: AtomicI32,
    throws: AtomicBool,
}
fn enter(probe: &Probe) {
    probe.calls.fetch_add(1, SeqCst);
    assert!(!probe.throws.load(SeqCst), "original result failed");
}
fn gate(effect: PostfixEffect, original: *const ()) -> PostfixGate {
    // SAFETY: every test supplies its effect's exact scalar ABI and keeps referenced storage/graphs alive.
    unsafe {
        let gate = PostfixGate::prepare(&effect, None).unwrap();
        gate.set_original(original);
        gate
    }
}
unsafe extern "C-unwind" fn byte(probe: &Probe, method: usize) -> u8 {
    assert_eq!(method, 0x1234);
    enter(probe);
    probe.value.fetch_add(1, SeqCst) as u8
}
unsafe extern "C-unwind" fn boss(probe: &Probe, rarity: f32, corruption: f32, tier: i32, island: i32, maximum: i32, method: usize) -> f32 {
    assert_eq!((rarity, corruption, tier, island, maximum, method), (1.25, 2.5, 3, 4, 5, 0x1234));
    enter(probe);
    0.25
}
unsafe extern "C-unwind" fn tier(
    probe: &Probe,
    item: usize,
    affix: usize,
    level: i32,
    uncraftable: bool,
    rarity: i32,
    effect: f32,
    context: usize,
    modifiers: usize,
    method: usize,
) -> i32 {
    assert_eq!(
        (item, affix, level, uncraftable, rarity, effect, context, modifiers, method),
        (11, 22, 47, true, 7, 1.25, 33, 44, 0x1234)
    );
    enter(probe);
    2
}

#[test]
fn loot_result_overrides_preserve_draws_all_ten_argument_positions_and_original_returns_when_closed_or_online() {
    let online = AtomicU8::new(0);
    let at = NonNull::from(&online).cast();
    let probe = Probe::default();
    let byte_gate = gate(max_random_rolls::effect(at), byte as *const ());
    let boss_gate = gate(boss_loot_chance::effect(at), boss as *const ());
    let tier_gate = gate(tier_7_affixes::effect(at), tier as *const ());
    // SAFETY: exact authored ABIs, including XMM1/XMM2 float registers and the sixth stack argument of RollTier.
    unsafe {
        let draw: unsafe extern "C-unwind" fn(&Probe, usize) -> u8 = std::mem::transmute(byte_gate.entry());
        let chance: unsafe extern "C-unwind" fn(&Probe, f32, f32, i32, i32, i32, usize) -> f32 = std::mem::transmute(boss_gate.entry());
        let roll: unsafe extern "C-unwind" fn(&Probe, usize, usize, i32, bool, i32, f32, usize, usize, usize) -> i32 = std::mem::transmute(tier_gate.entry());
        for (enabled, network) in [(false, 0), (true, 0), (true, 1), (false, 0)] {
            online.store(network, SeqCst);
            byte_gate.set_enabled(enabled);
            boss_gate.set_enabled(enabled);
            tier_gate.set_enabled(enabled);
            let before = probe.calls.load(SeqCst);
            let expected_draw = probe.value.load(SeqCst) as u8;
            let active = enabled && network == 0;
            assert_eq!(draw(&probe, 0x1234), if active { 255 } else { expected_draw });
            assert_eq!(probe.value.load(SeqCst) as u8, expected_draw.wrapping_add(1));
            assert_eq!(chance(&probe, 1.25, 2.5, 3, 4, 5, 0x1234), if active { 1.0 } else { 0.25 });
            assert_eq!(roll(&probe, 11, 22, 47, true, 7, 1.25, 33, 44, 0x1234), if active { 6 } else { 2 });
            assert_eq!(probe.calls.load(SeqCst), before + 3);
        }
        tier_gate.set_enabled(true);
        probe.throws.store(true, SeqCst);
        let before = probe.calls.load(SeqCst);
        let failed = std::panic::catch_unwind(|| roll(&probe, 11, 22, 47, true, 7, 1.25, 33, 44, 0x1234));
        assert!(failed.is_err());
        assert_eq!(probe.calls.load(SeqCst), before + 1);
        probe.throws.store(false, SeqCst);
        assert_eq!(roll(&probe, 11, 22, 47, true, 7, 1.25, 33, 44, 0x1234), 6);
    }
    tier_gate.set_enabled(false);
}

#[repr(C)]
struct Zone {
    header: [usize; 2],
    maximum: Cell<i32>,
}
unsafe extern "C-unwind" fn stability(_: *const Zone, probe: &Probe) -> i32 {
    enter(probe);
    probe.value.load(SeqCst)
}
unsafe extern "C-unwind" fn echo(probe: &Probe, tier: i32, surrounding: usize, chain: i32, status: i32, woven: u8, method: usize) -> i32 {
    assert_eq!((tier, surrounding, chain, status, woven, method), (9, 0x7777, 2, 3, 4, 0x1234));
    enter(probe);
    17
}
#[test]
fn stability_keeps_higher_results_and_echo_choices_map_every_catalog_value_after_the_original() {
    let online = AtomicU8::new(0);
    let at = NonNull::from(&online).cast();
    let probe = Probe::default();
    let zone = Zone {
        header: [0; 2],
        maximum: Cell::new(99),
    };
    let stable = gate(max_bonus_stability::effect(at, std::mem::offset_of!(Zone, maximum) as u32), stability as *const ());
    let echoes = gate(one_echo_type::effect(at), echo as *const ());
    // SAFETY: exact two/seven-position scalar ABIs; the zone remains readable after each original return.
    unsafe {
        let bonus: unsafe extern "C-unwind" fn(*const Zone, &Probe) -> i32 = std::mem::transmute(stable.entry());
        let choose: unsafe extern "C-unwind" fn(&Probe, i32, usize, i32, i32, u8, usize) -> i32 = std::mem::transmute(echoes.entry());
        stable.set_enabled(true);
        echoes.set_enabled(true);
        for original in [-10, 20, 99, 150] {
            probe.value.store(original, SeqCst);
            assert_eq!(bonus(&zone, &probe), original.max(99));
        }
        assert_eq!(bonus(std::ptr::null(), &probe), 150);
        for (index, expected) in one_echo_type::TYPES.iter().enumerate() {
            echoes.set_value(index as u64);
            let before = probe.calls.load(SeqCst);
            assert_eq!(choose(&probe, 9, 0x7777, 2, 3, 4, 0x1234), *expected);
            assert_eq!(probe.calls.load(SeqCst), before + 1);
        }
        echoes.set_value(99);
        assert_eq!(choose(&probe, 9, 0x7777, 2, 3, 4, 0x1234), 17);
        online.store(1, SeqCst);
        probe.value.store(7, SeqCst);
        echoes.set_value(1);
        assert_eq!(bonus(&zone, &probe), 7);
        assert_eq!(choose(&probe, 9, 0x7777, 2, 3, 4, 0x1234), 17);
        stable.set_enabled(false);
        echoes.set_enabled(false);
        online.store(0, SeqCst);
        assert_eq!(bonus(&zone, &probe), 7);
        assert_eq!(choose(&probe, 9, 0x7777, 2, 3, 4, 0x1234), 17);
    }
}

#[repr(C)]
struct Controller {
    header: [usize; 2],
    actor: usize,
}
#[repr(C)]
struct State {
    header: [usize; 2],
    controller: *const Controller,
}
unsafe extern "C-unwind" fn speed(_: *const State, info: usize, tags: i32, increase: f32, probe: &Probe) -> f32 {
    assert_eq!((info, tags, increase), (0x5678, 31, 1.25));
    enter(probe);
    2.5 + increase
}
#[test]
fn ability_speed_uses_current_player_identity_and_keeps_nonplayer_null_online_and_zero_multiplier_results() {
    let online = AtomicU8::new(0);
    let player = AtomicUsize::new(42);
    let probe = Probe::default();
    let ours = Controller { header: [0; 2], actor: 42 };
    let theirs = Controller { header: [0; 2], actor: 43 };
    let mine = State {
        header: [0; 2],
        controller: &ours,
    };
    let other = State {
        header: [0; 2],
        controller: &theirs,
    };
    let empty = State {
        header: [0; 2],
        controller: std::ptr::null(),
    };
    let layout = ability_speed::Layout {
        online: NonNull::from(&online).cast(),
        player: NonNull::from(&player).cast(),
        controller: std::mem::offset_of!(State, controller) as u32,
        actor: std::mem::offset_of!(Controller, actor) as u32,
    };
    let gate = gate(layout.effect(), speed as *const ());
    // SAFETY: five-position mixed ABI; the original uses XMM3 and its stack MethodInfo position.
    unsafe {
        let call: unsafe extern "C-unwind" fn(*const State, usize, i32, f32, &Probe) -> f32 = std::mem::transmute(gate.entry());
        gate.set_enabled(true);
        for multiplier in [0.0f64, 1.0, 2.0, 2.5, 100.0] {
            gate.set_value(multiplier.to_bits());
            let before = probe.calls.load(SeqCst);
            assert_eq!(call(&mine, 0x5678, 31, 1.25, &probe), (3.75 * if multiplier > 0.0 { multiplier } else { 1.0 }) as f32);
            assert_eq!(call(&other, 0x5678, 31, 1.25, &probe), 3.75);
            assert_eq!(probe.calls.load(SeqCst), before + 2);
        }
        assert_eq!(call(&empty, 0x5678, 31, 1.25, &probe), 3.75);
        assert_eq!(call(std::ptr::null(), 0x5678, 31, 1.25, &probe), 3.75);
        gate.set_value(2.0f64.to_bits());
        player.store(43, SeqCst);
        assert_eq!(call(&mine, 0x5678, 31, 1.25, &probe), 3.75);
        assert_eq!(call(&other, 0x5678, 31, 1.25, &probe), 7.5);
        player.store(0, SeqCst);
        assert_eq!(call(&other, 0x5678, 31, 1.25, &probe), 3.75);
        player.store(42, SeqCst);
        online.store(1, SeqCst);
        assert_eq!(call(&mine, 0x5678, 31, 1.25, &probe), 3.75);
        online.store(0, SeqCst);
        gate.set_enabled(false);
        assert_eq!(call(&mine, 0x5678, 31, 1.25, &probe), 3.75);
    }
}

#[test]
fn every_supported_frame_size_has_matching_windows_unwind_metadata() {
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, RtlLookupFunctionEntry, RtlVirtualUnwind};
    for (positions, frame) in [(2, 40), (5, 56), (7, 72), (10, 88), (12, 104)] {
        let effect = PostfixEffect {
            code: vec![],
            slots: [0; 2],
            positions,
        };
        // SAFETY: the unpublished gateway is not called; inspect its real registered unwind data against an authored stack.
        unsafe {
            let gate = PostfixGate::prepare(&effect, None).unwrap();
            let mut stack = [0u64; 32];
            stack[frame / 8] = 0x12345678;
            let mut context: CONTEXT = std::mem::zeroed();
            context.Rip = gate.entry() as u64 + 4;
            context.Rsp = stack.as_mut_ptr() as u64;
            let initial = context.Rsp;
            let mut base = 0;
            let function = RtlLookupFunctionEntry(context.Rip, &raw mut base, std::ptr::null_mut());
            assert!(!function.is_null());
            let mut handler = std::ptr::null_mut();
            let mut establisher = 0;
            RtlVirtualUnwind(
                0,
                base,
                context.Rip,
                function,
                &raw mut context,
                &raw mut handler,
                &raw mut establisher,
                std::ptr::null_mut(),
            );
            assert_eq!(context.Rip, 0x12345678);
            assert_eq!(context.Rsp, initial + frame as u64 + 8);
        }
    }
    for positions in [0, 13] {
        // SAFETY: rejected shapes must fail before any gateway is made callable.
        let rejected = unsafe {
            PostfixGate::prepare(
                &PostfixEffect {
                    code: vec![],
                    slots: [0; 2],
                    positions,
                },
                None,
            )
        };
        assert!(rejected.is_err());
    }
}
