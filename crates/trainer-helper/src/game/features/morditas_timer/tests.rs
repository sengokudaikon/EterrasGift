use super::*;
use native_hooks::postfix::PostfixGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

pub(crate) const HUNT: i32 = 7; // deliberately differs from the installed enum: the effect must use its resolved value
pub(crate) const METHOD: usize = 0x1234abcd;
#[repr(C)]
pub(crate) struct Hunt {
    header: [usize; 2],
    pub elapsed: f32,
    pub length: f32,
    pub state: i32,
    pub initial: f32,
    pub tick_speed: f32,
    pub bonus: f32,
    pub popup: f32,
    pub secondary: bool,
    pub end_tick: bool,
    pub calls: [usize; 3],
}
impl Default for Hunt {
    fn default() -> Self {
        Self {
            header: [0; 2],
            elapsed: 0.0,
            length: 0.0,
            state: HUNT,
            initial: 100.0,
            tick_speed: 1.0,
            bonus: 3.0,
            popup: 0.0,
            secondary: false,
            end_tick: false,
            calls: [0; 3],
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Throw,
    Nested,
    Close,
    Open,
}
thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Normal) };
    static GATE: Cell<usize> = const { Cell::new(0) };
    static OTHER: Cell<*mut Hunt> = const { Cell::new(std::ptr::null_mut()) };
    static TOTAL: Cell<usize> = const { Cell::new(0) };
}
fn begin(method: usize) {
    assert_eq!(method, METHOD);
    TOTAL.set(TOTAL.get() + 1);
}
fn finish() {
    let mode = MODE.get();
    if matches!(mode, Mode::Close | Mode::Open) {
        // SAFETY: the synchronous test retains its first gateway while the original runs.
        unsafe { (&*(GATE.get() as *const PostfixGate)).set_enabled(mode == Mode::Open) };
    }
    assert!(mode != Mode::Throw, "original threw");
}
pub(crate) unsafe extern "C-unwind" fn start(hunt: *mut Hunt, method: usize) {
    begin(method);
    // SAFETY: the fixture owns a live receiver or passes null for original forwarding.
    unsafe {
        if let Some(hunt) = hunt.as_mut() {
            hunt.calls[0] += 1;
            if hunt.secondary {
                hunt.state = 99;
            } else {
                hunt.length = hunt.initial;
                hunt.state = HUNT;
            }
        }
    }
    finish();
}
pub(crate) unsafe extern "C-unwind" fn tick(hunt: *mut Hunt, delta: f32, method: usize) {
    begin(method);
    // SAFETY: receivers and nested test storage survive both original invocations.
    unsafe {
        if let Some(hunt) = hunt.as_mut() {
            hunt.calls[1] += 1;
            hunt.elapsed += delta * hunt.tick_speed;
            if hunt.end_tick {
                hunt.state = 99;
            }
            if MODE.get() == Mode::Nested {
                MODE.set(Mode::Normal);
                invoke(Stage::Tick, (&*(GATE.get() as *const PostfixGate)).entry(), OTHER.get(), delta * 2.0);
                MODE.set(Mode::Nested);
            }
        }
    }
    finish();
}
pub(crate) unsafe extern "C-unwind" fn kill(hunt: *mut Hunt, dead: usize, unsummoned: bool, method: usize) {
    begin(method);
    assert_eq!((dead, unsummoned), (0x5678, true));
    // SAFETY: the fixture owns the passed receiver for this synchronous call.
    unsafe {
        if let Some(hunt) = hunt.as_mut() {
            hunt.calls[2] += 1;
            hunt.length += hunt.bonus;
            hunt.popup = hunt.bonus; // retain the original's pre-adjustment notification timing
        }
    }
    finish();
}
pub(crate) fn layout(online: &AtomicU8) -> Layout {
    Layout {
        online: NonNull::from(online).cast(),
        elapsed: offset_of!(Hunt, elapsed) as u32,
        length: offset_of!(Hunt, length) as u32,
        state: offset_of!(Hunt, state) as u32,
        hunt: HUNT,
    }
}
pub(crate) unsafe fn invoke(stage: Stage, entry: *const (), hunt: *mut Hunt, delta: f32) {
    // SAFETY: stage selects the authored original's exact ABI, including XMM1 for a tick's time.
    unsafe {
        match stage {
            Stage::Start => std::mem::transmute::<*const (), unsafe extern "C-unwind" fn(*mut Hunt, usize)>(entry)(hunt, METHOD),
            Stage::Tick => std::mem::transmute::<*const (), unsafe extern "C-unwind" fn(*mut Hunt, f32, usize)>(entry)(hunt, delta, METHOD),
            Stage::Kill => std::mem::transmute::<*const (), unsafe extern "C-unwind" fn(*mut Hunt, usize, bool, usize)>(entry)(hunt, 0x5678, true, METHOD),
        }
    }
}
pub(crate) fn value(values: [f64; 3]) -> eg_trainer_wire::hooks::HookValue {
    GameplayHook::MorditasTimer.value_kind().encode(&values).unwrap()
}
fn gates(online: &AtomicU8, values: [f64; 3]) -> Vec<PostfixGate> {
    let mut gates = Vec::new();
    for (stage, original) in STAGES.into_iter().zip([start as *const (), tick as *const (), kill as *const ()]) {
        // SAFETY: matched authored signatures/field offsets and retained static veto byte. All three share admission.
        let gate = unsafe { PostfixGate::prepare(&layout(online).effect(stage), gates.first()).unwrap() };
        // SAFETY: configured before admission, with the exact corresponding ABI.
        unsafe { gate.set_original(original) };
        SETTINGS.set(&gate, GameplayHook::MorditasTimer, true, value(values));
        gates.push(gate);
    }
    gates[0].set_enabled(true);
    gates
}
fn call(gates: &[PostfixGate], stage: Stage, hunt: *mut Hunt, delta: f32) {
    let calls = TOTAL.get();
    // SAFETY: caller retains its receiver or deliberately passes null to the authored original.
    unsafe { invoke(stage, gates[stage as usize].entry(), hunt, delta) };
    assert_eq!(TOTAL.get(), calls + 1);
}

#[test]
fn morditas_timer_scales_each_original_delta_and_keeps_ended_or_unchanged_hunts() {
    let online = AtomicU8::new(0);
    let gates = gates(&online, [2.5, 0.25, 3.0]);
    let mut hunt = Hunt::default();
    call(&gates, Stage::Start, &mut hunt, 0.0);
    assert_eq!(hunt.length, 250.0);
    hunt.elapsed = 10.0;
    hunt.tick_speed = 4.0;
    call(&gates, Stage::Tick, &mut hunt, 2.5);
    assert_eq!(hunt.elapsed, 12.5, "scale the original's accelerated delta, not the input tick");
    call(&gates, Stage::Kill, &mut hunt, 0.0);
    assert_eq!((hunt.length, hunt.popup, hunt.calls), (259.0, 3.0, [1, 1, 1]));
    hunt.secondary = true;
    call(&gates, Stage::Start, &mut hunt, 0.0);
    assert_eq!(hunt.length, 259.0, "unchanged secondary duration must not compound");
    hunt.end_tick = true;
    call(&gates, Stage::Tick, &mut hunt, 2.5);
    assert_eq!((hunt.elapsed, hunt.state), (22.5, 99), "never rewind the tick that ended a hunt");
    hunt.end_tick = false;
    hunt.state = HUNT;
    call(&gates, Stage::Tick, &mut hunt, 0.0);
    call(&gates, Stage::Tick, &mut hunt, -2.0);
    assert_eq!(hunt.elapsed, 14.5, "zero/negative elapsed deltas remain the game's");
    hunt.bonus = 0.0;
    call(&gates, Stage::Kill, &mut hunt, 0.0);
    hunt.bonus = -5.0;
    call(&gates, Stage::Kill, &mut hunt, 0.0);
    assert_eq!(hunt.length, 254.0, "only positive kill bonuses scale");
    for gate in &gates {
        SETTINGS.set(gate, GameplayHook::MorditasTimer, true, value([1.0, 0.0, 1.0]));
    }
    call(&gates, Stage::Tick, &mut hunt, 2.5);
    assert_eq!(hunt.elapsed, 14.5, "zero speed holds elapsed time without skipping original work");
    gates[0].set_enabled(false);
    call(&gates, Stage::Tick, &mut hunt, 2.5);
    assert_eq!(hunt.elapsed, 24.5, "off resumes from current elapsed time");
    gates[0].set_enabled(true);
    online.store(1, SeqCst);
    hunt.secondary = false;
    call(&gates, Stage::Start, &mut hunt, 0.0);
    call(&gates, Stage::Tick, &mut hunt, 2.5);
    assert_eq!((hunt.length, hunt.elapsed), (100.0, 34.5));
    for stage in STAGES {
        call(&gates, stage, std::ptr::null_mut(), 2.5);
    }
}

#[test]
fn morditas_timer_snapshots_are_local_and_exceptions_or_midcall_admission_changes_do_not_adjust() {
    let online = AtomicU8::new(0);
    let gates = gates(&online, [2.0, 0.5, 4.0]);
    GATE.set(&gates[1] as *const _ as usize);
    let mut outer = Hunt { elapsed: 10.0, ..Hunt::default() };
    let mut inner = Hunt { elapsed: 50.0, ..Hunt::default() };
    OTHER.set(&mut inner);
    MODE.set(Mode::Nested);
    // SAFETY: nested calls have different retained receivers and invocation-local snapshots.
    unsafe { invoke(Stage::Tick, gates[1].entry(), &mut outer, 4.0) };
    assert_eq!((outer.elapsed, inner.elapsed, outer.calls[1], inner.calls[1]), (12.0, 54.0, 1, 1));
    for stage in STAGES {
        MODE.set(Mode::Throw);
        let mut hunt = Hunt::default();
        let before = TOTAL.get();
        let thrown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: exact ABI, registered retained unwind frame; no helper return address or retry.
            unsafe { invoke(stage, gates[stage as usize].entry(), &mut hunt, 4.0) }
        }));
        assert!(thrown.is_err());
        assert_eq!(TOTAL.get(), before + 1);
        assert_eq!(
            (hunt.length, hunt.elapsed),
            match stage {
                Stage::Start => (100.0, 0.0),
                Stage::Tick => (0.0, 4.0),
                Stage::Kill => (3.0, 0.0),
            }
        );
    }
    for (mode, enabled) in [(Mode::Close, true), (Mode::Open, false)] {
        MODE.set(mode);
        gates[0].set_enabled(enabled);
        let mut hunt = Hunt::default();
        call(&gates, Stage::Tick, &mut hunt, 4.0);
        assert_eq!(hunt.elapsed, 4.0, "both entry and return must be admitted");
    }
    MODE.set(Mode::Normal);
    gates[0].set_enabled(true);
    let entries = gates.iter().map(|gate| gate.entry() as usize).collect::<Vec<_>>();
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..3)
            .map(|index| {
                let entries = &entries;
                scope.spawn(move || {
                    let mut hunt = Hunt {
                        initial: 100.0 + index as f32,
                        ..Hunt::default()
                    };
                    for stage in STAGES {
                        // SAFETY: each thread owns its receiver; code and atomic settings are shared and retained until join.
                        unsafe { invoke(stage, entries[stage as usize] as *const (), &mut hunt, 4.0) };
                    }
                    assert_eq!((hunt.length, hunt.elapsed, hunt.calls), (hunt.initial * 2.0 + 12.0, 2.0, [1; 3]));
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    });
}
