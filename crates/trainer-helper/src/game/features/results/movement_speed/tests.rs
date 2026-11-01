use super::*;
use native_hooks::postfix::PostfixGate;
use std::{
    cell::Cell,
    sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering::SeqCst},
};

pub(crate) type Stat = extern "C-unwind" fn(usize, u8, i32, f32, f32, f32, u8, i32, bool, bool, usize) -> f32;
thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static THROW: Cell<bool> = const { Cell::new(false) };
    static NESTED: Cell<Option<(Stat, bool)>> = const { Cell::new(None) };
    static NESTED_VALUE: Cell<f32> = const { Cell::new(0.0) };
}
pub(crate) extern "C-unwind" fn original(
    _: usize,
    _: u8,
    tags: i32,
    added: f32,
    increased: f32,
    more: f32,
    special: u8,
    extra: i32,
    ignore: bool,
    zero: bool,
    method: usize,
) -> f32 {
    assert_eq!(
        (tags, added, increased, more, special, extra, ignore, zero, method),
        (23, 1.25, 2.5, 3.75, 4, 17, true, false, 0x1234abcd)
    );
    CALLS.set(CALLS.get() + 1);
    if let Some((call, selected)) = NESTED.take() {
        NESTED_VALUE.set(if selected { invoke(call, 42, 9) } else { other(call, 42, 9) });
    }
    assert!(!THROW.get(), "original stat exception");
    // SAFETY: intentionally clobber the volatile property register after observing every forwarded argument.
    unsafe {
        core::arch::asm!("mov rdx,0xa5", out("rdx") _, options(nomem, nostack, preserves_flags));
    }
    6.25
}
#[inline(never)]
pub(crate) fn invoke(call: Stat, stats: usize, property: u8) -> f32 {
    let value = std::hint::black_box(call)(stats, property, 23, 1.25, 2.5, 3.75, 4, 17, true, false, 0x1234abcd);
    std::hint::black_box(value) // preserve a real call and its return site, rather than a tail jump
}
#[inline(never)]
fn other(call: Stat, stats: usize, property: u8) -> f32 {
    let value = std::hint::black_box(call)(stats, property, 23, 1.25, 2.5, 3.75, 4, 17, true, false, 0x1234abcd);
    std::hint::black_box(value + 1.0) - 1.0 // distinct caller code, so linker folding cannot merge the sites
}
fn gate(effect: PostfixEffect, original: *const ()) -> PostfixGate {
    // SAFETY: authored exact eleven-position ABI and retained static/receiver graph for every call.
    unsafe {
        let gate = PostfixGate::prepare(&effect, None).unwrap();
        gate.set_original(original);
        gate.set_enabled(true);
        gate
    }
}
fn function(gate: &PostfixGate) -> Stat {
    // SAFETY: each gate in this fixture has the same declared eleven-position stat ABI.
    unsafe { std::mem::transmute(gate.entry()) }
}
/// Record the compiled fixture caller's real return address without assuming Rust compiler instruction offsets.
pub(crate) fn callsite(online: NonNull<c_void>, original: *const ()) -> u64 {
    let observed = Box::leak(Box::new(AtomicU64::new(0)));
    let mut c = Code::new(online);
    c.offset(&[0x4c, 0x8b, 0x9c, 0x24], u32::from(frame_size(SPEC.positions())));
    c.emit(&[0x48, 0x8b, 0x40, FRAME.slots[1], 0x4c, 0x89, 0x18]);
    let probe = gate(c.finish(&SPEC, observed as *const _ as u64), original);
    assert_eq!(invoke(function(&probe), 42, 9), 6.25);
    probe.set_enabled(false);
    let site = observed.load(SeqCst);
    assert_ne!(site, 0);
    site
}

#[test]
fn only_the_selected_player_query_scales_and_all_eleven_arguments_survive() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let stats = Box::leak(Box::new(AtomicUsize::new(42)));
    let player = Box::leak(Box::new(AtomicUsize::new(stats as *const _ as usize)));
    let layout = Layout {
        online: NonNull::from(&*online).cast(),
        player: NonNull::from(&*player).cast(),
        stats: 0,
        property: 9,
        caller: callsite(NonNull::from(&*online).cast(), original as *const ()),
    };
    let gate = gate(layout.effect(), original as *const ());
    let call = function(&gate);
    for multiplier in [0.0f64, 0.5, 1.0, 2.5, 100.0] {
        gate.set_value(multiplier.to_bits());
        let before = CALLS.get();
        assert_eq!(invoke(call, 42, 9), if multiplier > 0.0 { (6.25 * multiplier) as f32 } else { 6.25 });
        assert_eq!(CALLS.get(), before + 1);
        assert_eq!(invoke(call, 42, 8), 6.25);
        assert_eq!(invoke(call, 43, 9), 6.25);
        assert_eq!(invoke(call, 0, 9), 6.25);
        assert_eq!(other(call, 42, 9), 6.25);
    }
    gate.set_value(2.0f64.to_bits());
    stats.store(43, SeqCst);
    assert_eq!(invoke(call, 42, 9), 6.25);
    assert_eq!(invoke(call, 43, 9), 12.5);
    stats.store(0, SeqCst);
    assert_eq!(invoke(call, 43, 9), 6.25);
    stats.store(42, SeqCst);
    player.store(0, SeqCst);
    assert_eq!(invoke(call, 42, 9), 6.25);
    player.store(stats as *const _ as usize, SeqCst);
    online.store(1, SeqCst);
    assert_eq!(invoke(call, 42, 9), 6.25);
    online.store(0, SeqCst);
    gate.set_enabled(false);
    assert_eq!(invoke(call, 42, 9), 6.25);
    gate.set_enabled(true);
    for selected in [true, false] {
        NESTED.set(Some((call, selected)));
        let before = CALLS.get();
        assert_eq!(invoke(call, 42, 9), 12.5);
        assert_eq!(NESTED_VALUE.get(), if selected { 12.5 } else { 6.25 });
        assert_eq!(CALLS.get(), before + 2);
    }
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(|| invoke(call, 42, 9)).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1);
    assert_eq!(other(call, 42, 9), 6.25, "unwinding leaves no scope behind");
    assert_eq!(invoke(call, 42, 9), 12.5);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(move || {
                for _ in 0..200 {
                    assert_eq!(invoke(call, 42, 9), 12.5);
                    assert_eq!(other(call, 42, 9), 6.25);
                    assert_eq!(invoke(call, 43, 9), 6.25);
                }
            });
        }
    });
    gate.set_enabled(false);
    player.store(0, SeqCst);
}

fn query(start: u64, original: u64) -> Vec<u8> {
    let mut code: Vec<u8> = QUERY.iter().map(|b| if *b < 0 { 0 } else { *b as u8 }).collect();
    code[3] = 0x28;
    code[29] = 0x50;
    code[58] = 9;
    let end = code.len();
    for (at, next) in [(22, 26), (35, 39)] {
        code[at..at + 4].copy_from_slice(&((end - next) as i32).to_le_bytes());
    }
    code[end - 4..].copy_from_slice(&(i32::try_from(original as i64 - (start + end as u64) as i64).unwrap()).to_le_bytes());
    code.extend([0xc3, 0xcc, 0xcc, 0xcc, 0xcc]);
    code
}
#[test]
fn query_recipe_requires_one_bounded_nonrepeating_call_to_the_checked_stat_method() {
    let start = 0x100000u64;
    let original = start + 0x1000;
    let code = query(start, original);
    let resolve = |code: &[u8]| caller(code, start, original, 0x28, 0x50, 9);
    assert_eq!(resolve(&code).unwrap(), start + QUERY.len() as u64);
    for (at, value) in [(3, 0x30), (29, 0x58), (58, 8), (QUERY.len() - 1, 0x7f), (QUERY.len() - 5, 0xe9)] {
        let mut changed = code.clone();
        changed[at] = value;
        assert!(resolve(&changed).is_err(), "byte {at}");
    }
    let mut duplicate = code[..QUERY.len()].to_vec();
    duplicate.extend(query(start + duplicate.len() as u64, original));
    assert!(resolve(&duplicate).is_err());
    let mut repeated = code[..QUERY.len()].to_vec();
    repeated.push(0xe9);
    repeated.extend((-(repeated.len() as i32 + 4)).to_le_bytes());
    repeated.extend([0xcc; 4]);
    assert!(resolve(&repeated).is_err());
    let mut guard = code.clone();
    guard[22..26].copy_from_slice(&0i32.to_le_bytes());
    assert!(resolve(&guard).is_err());
    let mut indirect = code[..QUERY.len()].to_vec();
    indirect.extend([0xff, 0xe0, 0xcc]);
    assert!(resolve(&indirect).is_err());
    assert!(resolve(&code[..QUERY.len()]).is_err());
    assert!(caller(&code, start, original, 128, 0x50, 9).is_err());
}
