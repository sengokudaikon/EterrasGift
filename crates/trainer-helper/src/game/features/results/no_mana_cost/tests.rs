use super::*;
use native_hooks::postfix::PostfixGate;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};

#[repr(C)]
struct Actor {
    header: [usize; 2],
    mana: usize,
}
struct Probe {
    player: AtomicUsize,
    online: AtomicU8,
    calls: AtomicUsize,
    added: AtomicU32,
    flags: AtomicUsize,
    throws: AtomicBool,
    block: AtomicBool,
    waiting: AtomicBool,
}
impl Probe {
    fn new(actor: &Actor) -> Self {
        Self {
            player: AtomicUsize::new(actor as *const Actor as usize),
            online: AtomicU8::new(0),
            calls: AtomicUsize::new(0),
            added: AtomicU32::new(0),
            flags: AtomicUsize::new(0),
            throws: AtomicBool::new(false),
            block: AtomicBool::new(false),
            waiting: AtomicBool::new(false),
        }
    }
    fn effect(&self) -> PostfixEffect {
        ManaCostLayout {
            player_storage: NonNull::from(&self.player).cast(),
            online_storage: NonNull::from(&self.online).cast(),
            mana_offset: std::mem::offset_of!(Actor, mana) as u32,
        }
        .effect()
    }
}
type Cost = unsafe extern "C-unwind" fn(usize, *const Probe, f32, bool, usize, bool, usize) -> f32;
unsafe extern "C-unwind" fn original(_: usize, probe: *const Probe, added: f32, provided: bool, mutator: usize, channel: bool, method: usize) -> f32 {
    // SAFETY: test keeps the probe alive until every call returns, including the delayed call.
    let probe = unsafe { &*probe };
    assert_eq!(mutator, 0x12345678);
    assert_eq!(method, 0xabcdef01);
    probe.calls.fetch_add(1, SeqCst);
    probe.added.store(added.to_bits(), SeqCst);
    probe.flags.store(usize::from(provided) | (usize::from(channel) << 1), SeqCst);
    assert!(!probe.throws.load(SeqCst), "owned cost calculation failed");
    while probe.block.load(SeqCst) {
        probe.waiting.store(true, SeqCst);
        std::thread::yield_now();
    }
    added + if provided { 100.0 } else { 10.0 } + if channel { 2.0 } else { 0.0 }
}
fn gateway(probe: &Probe) -> (PostfixGate, Cost) {
    // SAFETY: authored exact seven-position ABI; receiver/actor and static-like storage outlive every call.
    unsafe {
        let gate = PostfixGate::prepare(&probe.effect(), None).unwrap();
        gate.set_original(original as *const ());
        let call = std::mem::transmute::<*const (), Cost>(gate.entry());
        (gate, call)
    }
}
fn cost(call: Cost, probe: &Probe, receiver: usize, added: f32, provided: bool, channel: bool) -> f32 {
    let before = probe.calls.load(SeqCst);
    // SAFETY: exact authored ABI, with a live probe and scalar receiver identity.
    let result = unsafe { call(receiver, probe, added, provided, 0x12345678, channel, 0xabcdef01) };
    assert_eq!(probe.calls.load(SeqCst), before + 1);
    assert_eq!(probe.added.load(SeqCst), added.to_bits());
    assert_eq!(probe.flags.load(SeqCst), usize::from(provided) | (usize::from(channel) << 1));
    result
}

#[test]
fn original_runs_once_with_all_arguments_and_only_current_offline_player_cost_is_zero() {
    let actor = Actor { header: [0; 2], mana: 41 };
    let replacement = Actor { header: [0; 2], mana: 42 };
    let probe = Probe::new(&actor);
    let (gate, call) = gateway(&probe);
    assert_eq!(cost(call, &probe, 41, 1.25, false, false), 11.25);
    gate.set_enabled(true);
    for provided in [false, true] {
        for channel in [false, true] {
            assert_eq!(cost(call, &probe, 41, 1.25, provided, channel), 0.0);
            let normal = 1.25 + if provided { 100.0 } else { 10.0 } + if channel { 2.0 } else { 0.0 };
            assert_eq!(cost(call, &probe, 42, 1.25, provided, channel), normal);
        }
    }
    assert_eq!(cost(call, &probe, 0, 1.25, false, false), 11.25);
    probe.online.store(1, SeqCst);
    assert_eq!(cost(call, &probe, 41, 1.25, false, false), 11.25);
    probe.online.store(0, SeqCst);
    probe.player.store(0, SeqCst);
    assert_eq!(cost(call, &probe, 41, 1.25, false, false), 11.25);
    probe.player.store(&replacement as *const Actor as usize, SeqCst);
    assert_eq!(cost(call, &probe, 41, 1.25, false, false), 11.25);
    assert_eq!(cost(call, &probe, 42, 1.25, false, false), 0.0);
    gate.set_enabled(false);
    assert_eq!(cost(call, &probe, 42, 1.25, false, false), 11.25);
}

#[test]
fn original_unwinds_through_the_retained_frame_without_retrying_or_poisoning_later_calls() {
    let actor = Actor { header: [0; 2], mana: 41 };
    let probe = Probe::new(&actor);
    let (gate, call) = gateway(&probe);
    for enabled in [false, true] {
        gate.set_enabled(enabled);
        probe.throws.store(true, SeqCst);
        let before = probe.calls.load(SeqCst);
        let result = std::panic::catch_unwind(|| cost(call, &probe, 41, 1.25, false, false));
        assert!(result.is_err());
        assert_eq!(probe.calls.load(SeqCst), before + 1);
        probe.throws.store(false, SeqCst);
        assert_eq!(cost(call, &probe, 41, 1.25, false, false), if enabled { 0.0 } else { 11.25 });
    }
    gate.set_enabled(false);
}

#[test]
fn closing_and_dropping_the_owner_while_original_is_pending_keeps_its_result_and_return_path() {
    let actor = Actor { header: [0; 2], mana: 41 };
    let probe = Arc::new(Probe::new(&actor));
    probe.block.store(true, SeqCst);
    let (gate, call) = gateway(&probe);
    let gate = Box::new(gate);
    gate.set_enabled(true);
    let waiting = probe.clone();
    let worker = std::thread::spawn(move || cost(call, &waiting, 41, 1.25, false, false));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !probe.waiting.load(SeqCst) && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let entered = probe.waiting.load(SeqCst);
    gate.set_enabled(false);
    drop(gate);
    probe.block.store(false, SeqCst);
    assert_eq!(worker.join().unwrap(), 11.25);
    assert!(entered, "original did not enter before the close deadline");
}
