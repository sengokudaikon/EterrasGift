use super::*;
use native_hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering::SeqCst},
};
type DropCall = extern "C-unwind" fn(usize, usize, *const u8, i32, usize) -> bool;
thread_local! {
    static AMOUNT: Cell<i32> = const { Cell::new(0) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static THROW: Cell<bool> = const { Cell::new(false) };
}
extern "C-unwind" fn original(_: usize, actor: usize, _: *const u8, amount: i32, method: usize) -> bool {
    assert_eq!((actor, method), (42, 0x1234));
    AMOUNT.set(amount);
    CALLS.set(CALLS.get() + 1);
    assert!(!THROW.get(), "original exception");
    false
}
#[inline(never)]
fn invoke(call: DropCall, manager: usize, item: *const u8, amount: i32) -> bool {
    let result = std::hint::black_box(call)(manager, 42, item, amount, 0x1234);
    std::hint::black_box(result)
}
fn return_address() -> u64 {
    let start = invoke as *const u8;
    // SAFETY: the authored function and its neighbouring executable page are readable; stop at its first return.
    let code = unsafe { std::slice::from_raw_parts(start, 512) };
    let mut calls = Vec::new();
    for instruction in Decoder::with_ip(64, code, start as u64, DecoderOptions::NONE) {
        if instruction.flow_control() == FlowControl::Return {
            break;
        }
        if instruction.flow_control() == FlowControl::IndirectCall {
            calls.push(instruction.next_ip());
        }
    }
    assert_eq!(calls.len(), 1);
    calls[0]
}
#[test]
fn only_the_selected_local_drop_scales_positive_amounts_and_preserves_failures_and_unwind() {
    let online = AtomicU8::new(0);
    let manager = AtomicUsize::new(7);
    // SAFETY: authored five-position Boolean method and process storage retained until every call finishes.
    let gate = unsafe { LeafGate::prepare_override(effect(NonNull::from(&online).cast(), NonNull::from(&manager).cast(), return_address(), 0), None, None).unwrap() };
    // SAFETY: exact original configured before first call.
    unsafe {
        gate.set_original(original as *const ());
    }
    gate.set_value(3.7f64.to_bits());
    gate.set_enabled(true);
    // SAFETY: prepared gateway has exactly DropCall's ABI.
    let call: DropCall = unsafe { std::mem::transmute(gate.entry()) };
    for (kind, amount, wanted) in [
        (1u8, 2, 4),
        (104, 2, 6),
        (107, 2, 6),
        (1, 0, 0),
        (104, -2, -2),
        (1, i32::MAX, i32::MAX),
        (107, i32::MAX, i32::MAX),
    ] {
        let before = CALLS.get();
        assert!(!invoke(call, 7, &kind, amount));
        assert_eq!((AMOUNT.get(), CALLS.get()), (wanted, before + 1));
    }
    let kind = 104;
    assert!(!call(7, 42, &kind, 2, 0x1234));
    assert_eq!(AMOUNT.get(), 2, "another caller");
    invoke(call, 8, &kind, 2);
    assert_eq!(AMOUNT.get(), 2, "another manager");
    invoke(call, 7, std::ptr::null(), 2);
    assert_eq!(AMOUNT.get(), 2, "null item");
    online.store(1, SeqCst);
    invoke(call, 7, &kind, 2);
    assert_eq!(AMOUNT.get(), 2);
    online.store(0, SeqCst);
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(|| invoke(call, 7, &kind, 2)).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1);
    invoke(call, 7, &kind, 2);
    assert_eq!(AMOUNT.get(), 6);
    gate.set_enabled(false);
    invoke(call, 7, &kind, 2);
    assert_eq!(AMOUNT.get(), 2);
}

#[test]
fn recipe_requires_the_held_entry_fields_and_exact_drop_destination() {
    let start = 0x1000;
    let destination = 0x2000;
    let mut code = vec![
        0x44, 0x8b, 0x4f, 0x28, 0x33, 0xf6, 0x4c, 0x8b, 0x47, 0x10, 0x48, 0x8b, 0xcb, 0x48, 0x8b, 0x93, 0x70, 2, 0, 0, 0x48, 0x89, 0x74, 0x24, 0x20, 0xe8,
    ];
    let end = start + code.len() as u64 + 4;
    code.extend(((destination - end) as i32).to_le_bytes());
    code.push(0xcc);
    assert_eq!(caller(&code, start, destination, 0x10, 0x28, 0x270).unwrap(), end);
    assert!(caller(&code, start, destination + 1, 0x10, 0x28, 0x270).is_err());
    assert!(caller(&code, start, destination, 0x18, 0x28, 0x270).is_err());
    code[0] = 0x90;
    assert!(caller(&code, start, destination, 0x10, 0x28, 0x270).is_err());
}
