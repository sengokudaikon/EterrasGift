use super::*;
use crate::hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering::SeqCst},
};

thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static THROW: Cell<bool> = const { Cell::new(false) };
}
const METHOD: usize = 0x1234abcd;
fn called(method: usize) {
    assert_eq!(method, METHOD);
    CALLS.set(CALLS.get() + 1);
    assert!(!THROW.get(), "original threw");
}
fn static_state() -> (&'static AtomicU8, &'static AtomicUsize) {
    (Box::leak(Box::new(AtomicU8::new(0))), Box::leak(Box::new(AtomicUsize::new(0))))
}
fn gate(effect: LeafEffect, original: *const ()) -> LeafGate {
    // SAFETY: each test retains its authored static/receiver graph and exact ABI for every scoped call.
    unsafe {
        let gate = LeafGate::prepare_override(effect, None, None).unwrap();
        gate.set_original(original);
        gate.set_enabled(true);
        gate
    }
}
fn receiver(online: &AtomicU8, storage: &AtomicUsize, component: Option<u32>) -> Receiver {
    Receiver {
        online: NonNull::from(online).cast(),
        storage: NonNull::from(storage).cast(),
        component,
    }
}

#[test]
fn primordials_skip_only_the_current_local_count_and_follow_manager_replacement() {
    extern "C-unwind" fn original(manager: *const i32, method: usize) -> i32 {
        called(method);
        if manager.is_null() {
            -1
        } else {
            // SAFETY: every nonnull fixture receiver is the retained Int32 count below.
            unsafe { *manager }
        }
    }
    let (online, storage) = static_state();
    let local = 3i32;
    let other = 7i32;
    storage.store(&local as *const _ as usize, SeqCst);
    let gate = gate(several_primordials::effect(receiver(online, storage, None)), original as *const ());
    // SAFETY: exact instance/MethodInfo -> Int32 ABI over the authored count fixture.
    let call: extern "C-unwind" fn(*const i32, usize) -> i32 = unsafe { std::mem::transmute(gate.entry()) };
    let count = |manager, expected, originals| {
        let before = CALLS.get();
        assert_eq!(call(manager, METHOD), expected);
        assert_eq!(CALLS.get(), before + originals);
    };
    count(&local, 0, 0);
    count(&other, 7, 1);
    count(std::ptr::null(), -1, 1);
    storage.store(&other as *const _ as usize, SeqCst);
    count(&local, 3, 1);
    count(&other, 0, 0);
    storage.store(0, SeqCst);
    count(&other, 7, 1);
    storage.store(&other as *const _ as usize, SeqCst);
    online.store(1, SeqCst);
    count(&other, 7, 1);
    online.store(0, SeqCst);
    gate.set_enabled(false);
    count(&other, 7, 1);
    gate.set_enabled(true);
    count(&other, 0, 0);
    storage.store(0, SeqCst);
}

#[repr(C)]
struct Actor {
    header: [usize; 2],
    component: Cell<usize>,
}
const COMPONENT: u32 = offset_of!(Actor, component) as u32;
#[test]
fn fixed_area_skips_the_player_calculation_and_forwards_other_tags_and_method_info() {
    extern "C-unwind" fn original(_: usize, tags: i32, method: usize) -> f32 {
        called(method);
        assert_eq!(tags, 0x1234567);
        2.25
    }
    let (online, storage) = static_state();
    let actor = Actor {
        header: [0; 2],
        component: Cell::new(42),
    };
    storage.store(&actor as *const _ as usize, SeqCst);
    let gate = gate(fixed_area::effect(receiver(online, storage, Some(COMPONENT))), original as *const ());
    // SAFETY: exact instance, Int32 enum, MethodInfo -> Single ABI; component values are compared only.
    let call: extern "C-unwind" fn(usize, i32, usize) -> f32 = unsafe { std::mem::transmute(gate.entry()) };
    let area = |stats, expected, originals| {
        let before = CALLS.get();
        assert_eq!(call(stats, 0x1234567, METHOD), expected);
        assert_eq!(CALLS.get(), before + originals);
    };
    for value in [0.0f64, 0.125, 3.0, 49.875, 50.0] {
        gate.set_value(value.to_bits());
        area(42, value as f32, 0);
        area(43, 2.25, 1);
    }
    area(0, 2.25, 1);
    actor.component.set(43);
    area(42, 2.25, 1);
    area(43, 50.0, 0);
    actor.component.set(0);
    area(43, 2.25, 1);
    actor.component.set(43);
    storage.store(0, SeqCst);
    area(43, 2.25, 1);
    storage.store(&actor as *const _ as usize, SeqCst);
    online.store(1, SeqCst);
    area(43, 2.25, 1);
    online.store(0, SeqCst);
    gate.set_enabled(false);
    area(43, 2.25, 1);
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(|| call(43, 0x1234567, METHOD)).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1);
    storage.store(0, SeqCst);
}

#[repr(C)]
struct Mana {
    maximum: Cell<i32>,
    current: Cell<f32>,
    observed: Cell<f32>,
}
#[test]
fn mana_refills_before_original_converts_the_integer_max_and_keeps_normal_drain() {
    extern "C-unwind" fn original(mana: *const Mana, delta: f32, method: usize) {
        called(method);
        assert_eq!(delta.to_bits(), 1.25f32.to_bits());
        if let Some(mana) = {
            // SAFETY: every nonnull receiver is one retained Mana below; null is explicitly forwarded.
            unsafe { mana.as_ref() }
        } {
            mana.observed.set(mana.current.get());
            mana.current.set(mana.current.get() - delta);
        }
    }
    let (online, storage) = static_state();
    let local = Mana {
        maximum: Cell::new(101),
        current: Cell::new(2.0),
        observed: Cell::new(0.0),
    };
    let other = Mana {
        maximum: Cell::new(200),
        current: Cell::new(5.0),
        observed: Cell::new(0.0),
    };
    let actor = Actor {
        header: [0; 2],
        component: Cell::new(&local as *const _ as usize),
    };
    storage.store(&actor as *const _ as usize, SeqCst);
    let layout = refill_mana::Layout {
        receiver: receiver(online, storage, Some(COMPONENT)),
        maximum: offset_of!(Mana, maximum) as u32,
        current: offset_of!(Mana, current) as u32,
    };
    let gate = gate(layout.effect(), original as *const ());
    // SAFETY: exact instance, Single in XMM1, MethodInfo in R8 -> Void ABI and matching scalar field layout.
    let call: extern "C-unwind" fn(*const Mana, f32, usize) = unsafe { std::mem::transmute(gate.entry()) };
    let tick = |mana| {
        let before = CALLS.get();
        call(mana, 1.25, METHOD);
        assert_eq!(CALLS.get(), before + 1);
    };
    tick(&local);
    assert_eq!((local.observed.get(), local.current.get()), (101.0, 99.75));
    tick(&other);
    assert_eq!((other.observed.get(), other.current.get()), (5.0, 3.75));
    local.maximum.set(16777217);
    tick(&local);
    assert_eq!(local.observed.get(), 16777217i32 as f32);
    local.maximum.set(301);
    tick(&local);
    assert_eq!(local.observed.get(), 301.0);
    actor.component.set(&other as *const _ as usize);
    local.current.set(10.0);
    tick(&local);
    assert_eq!(local.observed.get(), 10.0);
    tick(&other);
    assert_eq!(other.observed.get(), 200.0);
    for unavailable in 0..4 {
        other.current.set(11.0);
        match unavailable {
            0 => actor.component.set(0),
            1 => {
                actor.component.set(&other as *const _ as usize);
                storage.store(0, SeqCst);
            }
            2 => {
                storage.store(&actor as *const _ as usize, SeqCst);
                online.store(1, SeqCst);
            }
            _ => {
                online.store(0, SeqCst);
                gate.set_enabled(false);
            }
        }
        tick(&other);
        assert_eq!(other.observed.get(), 11.0);
    }
    tick(std::ptr::null());
    gate.set_enabled(true);
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(&other, 1.25, METHOD))).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1);
    assert_eq!(other.current.get(), 200.0, "the pre-tick refill persists when the original throws");
    storage.store(0, SeqCst);
}
