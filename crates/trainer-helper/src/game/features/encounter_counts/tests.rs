use super::*;
use native_hooks::postfix::PostfixGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tally {
    pub done: bool,
    pub total: i32,
    pub remaining: i32,
    pub multiplier: f32,
}
impl Default for Tally {
    fn default() -> Self {
        Self {
            done: false,
            total: 0,
            remaining: 0,
            multiplier: 0.0,
        }
    }
}
#[repr(C)]
#[derive(Default)]
pub(crate) struct Context {
    header: [usize; 2],
    pub tallies: [Tally; 2],
}
#[repr(C)]
pub(crate) struct Config {
    header: [usize; 2],
    rate: f32,
}
#[repr(C)]
pub(crate) struct Scene {
    header: [usize; 2],
    config: *const Config,
}
#[repr(C)]
pub(crate) struct Placement {
    header: [usize; 2],
    pub context: *mut Context,
    scene: *const Scene,
}
pub(crate) struct Fixture {
    pub context: Box<Context>,
    pub config: Box<Config>,
    _scene: Box<Scene>,
    pub placement: Box<Placement>,
}
impl Fixture {
    pub(crate) fn new(rate: f32) -> Self {
        let mut context = Box::<Context>::default();
        let config = Box::new(Config { header: [0; 2], rate });
        let scene = Box::new(Scene { header: [0; 2], config: &*config });
        let placement = Box::new(Placement {
            header: [0; 2],
            context: &mut *context,
            scene: &*scene,
        });
        Self {
            context,
            config,
            _scene: scene,
            placement,
        }
    }
}
pub(crate) fn layout(online: &AtomicU8) -> Layout {
    let fields = |index| {
        let base = offset_of!(Context, tallies) + index * std::mem::size_of::<Tally>();
        Counts {
            done: (base + offset_of!(Tally, done)) as u32,
            total: (base + offset_of!(Tally, total)) as u32,
            remaining: (base + offset_of!(Tally, remaining)) as u32,
            multiplier: (base + offset_of!(Tally, multiplier)) as u32,
        }
    };
    Layout {
        online: NonNull::from(online).cast(),
        context: offset_of!(Placement, context) as u32,
        scene: offset_of!(Placement, scene) as u32,
        config: offset_of!(Scene, config) as u32,
        rate: offset_of!(Config, rate) as u32,
        counts: [fields(0), fields(1)],
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Partial,
    Replace,
    Throw,
    Nested,
}
thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Normal) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static OTHER: Cell<*mut Placement> = const { Cell::new(std::ptr::null_mut()) };
    static GATE: Cell<usize> = const { Cell::new(0) };
}
pub(crate) type Call = unsafe extern "C-unwind" fn(usize, *mut Placement, usize, usize, usize);
pub(crate) unsafe extern "C-unwind" fn original(level: usize, placement: *mut Placement, data: usize, weaver: usize, method: usize) {
    assert_eq!((level, data, weaver, method), (0x1234, 0x5678, 0x9abc, 0xdef0));
    CALLS.set(CALLS.get() + 1);
    // SAFETY: the fixture retains each nonnull placement/context through nested calls and any unwind.
    unsafe {
        if placement.is_null() {
            return;
        }
        if MODE.get() == Mode::Nested {
            MODE.set(Mode::Normal);
            invoke(GATE.get(), OTHER.get());
        }
        if MODE.get() == Mode::Replace {
            (*placement).context = (*OTHER.get()).context;
        }
        let context = (*placement).context;
        if context.is_null() {
            return;
        }
        for (index, tally) in (*context).tallies.iter_mut().enumerate() {
            if MODE.get() == Mode::Partial && index == 1 {
                continue;
            }
            if tally.done {
                tally.remaining = (tally.remaining - 1).max(0);
            } else {
                *tally = Tally {
                    done: true,
                    total: 4,
                    remaining: 4,
                    multiplier: 2.0,
                };
            }
        }
        assert!(MODE.get() != Mode::Throw, "original threw");
        core::arch::asm!("mov rdx,1", out("rdx") _);
    }
}
pub(crate) unsafe fn invoke(entry: usize, placement: *mut Placement) {
    // SAFETY: exact five-position static void ABI over retained fixture storage.
    unsafe {
        std::mem::transmute::<usize, Call>(entry)(0x1234, placement, 0x5678, 0x9abc, 0xdef0);
    }
}
fn set(gate: &PostfixGate, hook: GameplayHook, on: bool, value: f64) {
    let active = SETTINGS.target_on(gate, hook, on);
    gate.set_enabled(false);
    SETTINGS.set(gate, hook, on, value.to_bits());
    gate.set_enabled(active);
}
fn gate(online: &AtomicU8) -> PostfixGate {
    // SAFETY: exact fixture ABI, metadata-equivalent field offsets and retained online storage; no hidden return buffer.
    unsafe {
        let gate = PostfixGate::prepare(&layout(online).effect(), None).unwrap();
        gate.set_original(original as *const ());
        gate
    }
}
#[test]
fn independent_counts_adjust_only_new_calculations_on_the_original_context() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let gate = gate(online);
    set(&gate, GameplayHook::LootLizards, true, 5.0);
    let run = |fixture: &mut Fixture| {
        let before = CALLS.get();
        // SAFETY: every box/graph is retained throughout the synchronous call.
        unsafe {
            invoke(gate.entry() as usize, &mut *fixture.placement);
        }
        assert_eq!(CALLS.get(), before + 1);
    };
    let mut f = Fixture::new(2.0);
    run(&mut f);
    assert_eq!(
        f.context.tallies[0],
        Tally {
            done: true,
            total: 5,
            remaining: 5,
            multiplier: 2.5
        }
    );
    assert_eq!(f.context.tallies[1].total, 4);
    run(&mut f);
    assert_eq!(f.context.tallies[0].remaining, 4, "consumed encounters stay consumed");
    set(&gate, GameplayHook::RuneWardens, true, 7.0);
    set(&gate, GameplayHook::LootLizards, false, 5.0);
    f = Fixture::new(2.0);
    run(&mut f);
    assert_eq!(f.context.tallies[0].total, 4);
    assert_eq!(f.context.tallies[1].total, 7);
    set(&gate, GameplayHook::LootLizards, true, 0.0);
    f = Fixture::new(2.0);
    f.config.rate = 0.0;
    run(&mut f);
    assert_eq!(f.context.tallies[0].remaining, 0);
    assert_eq!(f.context.tallies[1].multiplier, 7.0 / 0.0001);
    set(&gate, GameplayHook::LootLizards, true, 20.0);
    MODE.set(Mode::Partial);
    f = Fixture::new(1.0);
    run(&mut f);
    MODE.set(Mode::Normal);
    assert_eq!(f.context.tallies[0].total, 20);
    assert!(!f.context.tallies[1].done);
    let mut replacement = Fixture::new(1.0);
    OTHER.set(&mut *replacement.placement);
    MODE.set(Mode::Replace);
    f = Fixture::new(1.0);
    run(&mut f);
    MODE.set(Mode::Normal);
    assert!(!f.context.tallies[0].done);
    assert_eq!(replacement.context.tallies[0].total, 4);
    f = Fixture::new(1.0);
    f.placement.scene = std::ptr::null();
    run(&mut f);
    assert_eq!(f.context.tallies[0].total, 4);
    f = Fixture::new(1.0);
    f.placement.context = std::ptr::null_mut();
    run(&mut f);
    // SAFETY: the null-placement path must forward without reading a graph.
    unsafe {
        invoke(gate.entry() as usize, std::ptr::null_mut());
    }
    online.store(1, SeqCst);
    f = Fixture::new(1.0);
    run(&mut f);
    assert_eq!(f.context.tallies[0].total, 4);
    online.store(0, SeqCst);
    gate.set_enabled(false);
    f = Fixture::new(1.0);
    run(&mut f);
    assert_eq!(f.context.tallies[0].total, 4);
}
#[test]
fn snapshots_are_per_invocation_and_original_exceptions_do_not_run_or_leak_adjustments() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let gate = gate(online);
    set(&gate, GameplayHook::LootLizards, true, 8.0);
    set(&gate, GameplayHook::RuneWardens, true, 6.0);
    GATE.set(gate.entry() as usize);
    let mut outer = Fixture::new(1.0);
    let mut inner = Fixture::new(1.0);
    outer.context.tallies[0] = Tally {
        done: true,
        total: 10,
        remaining: 3,
        multiplier: 10.0,
    };
    OTHER.set(&mut *inner.placement);
    MODE.set(Mode::Nested);
    // SAFETY: both call-local graphs remain owned through the nested original call.
    unsafe {
        invoke(gate.entry() as usize, &mut *outer.placement);
    }
    assert_eq!(inner.context.tallies[0].total, 8);
    assert_eq!(outer.context.tallies[0].remaining, 2);
    assert_eq!(outer.context.tallies[1].total, 6);
    MODE.set(Mode::Throw);
    let mut f = Fixture::new(1.0);
    let before = CALLS.get();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the unwind leaves the retained gateway/frame through registered Windows unwind data.
            unsafe {
                invoke(gate.entry() as usize, &mut *f.placement);
            }
        }))
        .is_err()
    );
    MODE.set(Mode::Normal);
    assert_eq!(CALLS.get(), before + 1);
    assert_eq!(f.context.tallies[0].total, 4);
    f = Fixture::new(1.0);
    // SAFETY: a fresh context after the throwing call must receive only its own snapshot.
    unsafe {
        invoke(gate.entry() as usize, &mut *f.placement);
    }
    assert_eq!(f.context.tallies[0].total, 8);
    let entry = gate.entry() as usize;
    let threads: Vec<_> = (0..6)
        .map(|_| {
            std::thread::spawn(move || {
                let mut f = Fixture::new(2.0);
                // SAFETY: each thread owns a distinct graph, and all share only retained immutable code and atomic settings.
                unsafe {
                    invoke(entry, &mut *f.placement);
                }
                assert_eq!(f.context.tallies[0].total, 8);
                assert_eq!(f.context.tallies[1].multiplier, 3.0);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}
