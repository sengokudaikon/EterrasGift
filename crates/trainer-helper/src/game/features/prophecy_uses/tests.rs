use super::*;
use native_hooks::postfix::PostfixGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

#[repr(C)]
pub(crate) struct Slot {
    header: [usize; 2],
    pub charges: u8,
    pub calls: usize,
}
impl Slot {
    pub(crate) fn new(charges: u8) -> Self {
        Self {
            header: [0; 2],
            charges,
            calls: 0,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Success,
    Fail,
    Increase,
    Throw,
    Nested,
    Close,
    Open,
}
thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Success) };
    static GATE: Cell<usize> = const { Cell::new(0) };
    static OTHER: Cell<*mut Slot> = const { Cell::new(std::ptr::null_mut()) };
}
pub(crate) type Call = unsafe extern "C-unwind" fn(*mut Slot, i32, bool, usize, *mut i32, usize) -> bool;
pub(crate) unsafe extern "C-unwind" fn original(slot: *mut Slot, target: i32, doubled: bool, player: usize, consumed: *mut i32, method: usize) -> bool {
    assert_eq!((target, doubled, player, method), (-1234567, true, 0x5678, 0xabcdef01));
    // SAFETY: fixture owns all passed objects and the actual out parameter throughout nested calls and unwind.
    unsafe {
        *consumed = 4;
        if slot.is_null() {
            return false;
        }
        (*slot).calls += 1;
        (*slot).charges = if MODE.get() == Mode::Increase {
            (*slot).charges + 1
        } else {
            (*slot).charges.saturating_sub(4)
        };
        let mode = MODE.get();
        if mode == Mode::Nested {
            MODE.set(Mode::Success);
            let mut inner_consumed = -1;
            assert!(invoke((&*(GATE.get() as *const PostfixGate)).entry(), OTHER.get(), &mut inner_consumed));
            assert_eq!(inner_consumed, 4);
            MODE.set(mode);
        }
        if matches!(mode, Mode::Close | Mode::Open) {
            (&*(GATE.get() as *const PostfixGate)).set_enabled(mode == Mode::Open);
        }
        assert!(mode != Mode::Throw, "original threw");
        mode != Mode::Fail
    }
}
pub(crate) unsafe fn invoke(entry: *const (), slot: *mut Slot, consumed: &mut i32) -> bool {
    // SAFETY: the caller provides the exact six-position fixture ABI.
    unsafe { std::mem::transmute::<*const (), Call>(entry)(slot, -1234567, true, 0x5678, consumed, 0xabcdef01) }
}
pub(crate) fn charges_offset() -> u32 {
    offset_of!(Slot, charges) as u32
}
fn gate(online: &AtomicU8) -> PostfixGate {
    // SAFETY: matching six-position Boolean ABI, with the output pointer on the stack; static online byte is retained.
    unsafe {
        let gate = PostfixGate::prepare(&effect(NonNull::from(online).cast(), charges_offset()), None).unwrap();
        gate.set_original(original as *const ());
        gate
    }
}

#[test]
fn prophecy_uses_restores_only_successful_decreases_and_preserves_the_actual_output() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let gate = gate(online);
    GATE.set(&gate as *const _ as usize);
    for (mode, enabled, veto, before, after, result) in [
        (Mode::Success, true, 0, 6, 6, true),
        (Mode::Success, true, 0, 255, 255, true),
        (Mode::Success, true, 0, 0, 0, true),
        (Mode::Fail, true, 0, 6, 2, false),
        (Mode::Increase, true, 0, 6, 7, true),
        (Mode::Success, false, 0, 6, 2, true),
        (Mode::Success, true, 1, 6, 2, true),
        (Mode::Close, true, 0, 6, 2, true),
        (Mode::Open, false, 0, 6, 2, true),
    ] {
        MODE.set(mode);
        online.store(veto, SeqCst);
        gate.set_enabled(enabled);
        let mut slot = Slot::new(before);
        let mut consumed = -1;
        // SAFETY: exact ABI over locally retained slot/output storage.
        assert_eq!(unsafe { invoke(gate.entry(), &mut slot, &mut consumed) }, result);
        assert_eq!((slot.charges, slot.calls, consumed), (after, 1, 4));
    }
    gate.set_enabled(true);
    online.store(0, SeqCst);
    MODE.set(Mode::Success);
    let mut consumed = -1;
    // SAFETY: original explicitly accepts null; no hook dereference occurs.
    assert!(!unsafe { invoke(gate.entry(), std::ptr::null_mut(), &mut consumed) });
    assert_eq!(consumed, 4);
}

#[test]
fn prophecy_uses_snapshots_are_invocation_local_and_original_unwind_never_retries_or_restores() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let gate = gate(online);
    GATE.set(&gate as *const _ as usize);
    gate.set_enabled(true);
    let mut outer = Slot::new(6);
    let mut inner = Slot::new(5);
    OTHER.set(&mut inner);
    MODE.set(Mode::Nested);
    let mut consumed = -1;
    // SAFETY: nested slots and output storage survive the synchronous calls.
    assert!(unsafe { invoke(gate.entry(), &mut outer, &mut consumed) });
    assert_eq!((outer.charges, inner.charges, outer.calls, inner.calls, consumed), (6, 5, 1, 1, 4));
    MODE.set(Mode::Throw);
    let thrown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: the registered gateway unwind frame forwards the original's unwind unchanged.
        unsafe { invoke(gate.entry(), &mut outer, &mut consumed) }
    }));
    assert!(thrown.is_err());
    assert_eq!((outer.charges, outer.calls, consumed), (2, 2, 4));
    MODE.set(Mode::Success);
}
