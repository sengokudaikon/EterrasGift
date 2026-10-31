use super::*;
use crate::hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; static THROW: Cell<bool> = const { Cell::new(false) }; }
extern "C-unwind" fn original(drops: usize, actor: usize, method: usize) -> f32 {
    CALLS.set(CALLS.get() + 1);
    assert_eq!((drops, actor, method), (0x11223344, 0x55667788, 0xabcddcba));
    assert!(!THROW.get(), "original threw");
    0.125
}
#[test]
fn chance_skips_original_only_while_enabled_offline_and_forwards_exact_arguments_otherwise() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    // SAFETY: exact three-position float-return ABI; process-lifetime online byte and original function.
    let gate = unsafe { LeafGate::prepare_override(effect(NonNull::from(&*online).cast()), None, None).unwrap() };
    // SAFETY: matching original set before any generated call.
    unsafe { gate.set_original(original as *const ()) };
    // SAFETY: the generated leaf returns Single or tail-calls the original with the same three arguments.
    let call: extern "C-unwind" fn(usize, usize, usize) -> f32 = unsafe { std::mem::transmute(gate.entry()) };
    for (enabled, is_online, expected, originals) in [(false, 0, 0.125, 1), (true, 0, 1.0, 0), (true, 1, 0.125, 1), (true, 0, 1.0, 0), (false, 0, 0.125, 1)] {
        gate.set_enabled(enabled);
        online.store(is_online, SeqCst);
        let before = CALLS.get();
        assert_eq!(call(0x11223344, 0x55667788, 0xabcddcba), expected);
        assert_eq!(CALLS.get(), before + originals);
    }
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(|| call(0x11223344, 0x55667788, 0xabcddcba)).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1);
}

#[test]
fn removing_the_failure_branch_preserves_the_later_item_level_rejection() {
    use crate::hooks::patch::{self, CodePatch};
    #[unsafe(naked)]
    unsafe extern "C-unwind" fn spawn(_: bool, _: i32, _: i32) -> bool {
        core::arch::naked_asm!(
            "xor eax,eax",
            "test cl,cl",
            ".byte 0x0f,0x85",
            ".long 2f - . - 4",
            "cmp edx,r8d",
            "jg 2f",
            "mov eax,1",
            "2:",
            "ret"
        );
    }
    // SAFETY: a complete authored six-byte failure Jcc, no incoming edge or return address in its interior.
    unsafe {
        let site = (spawn as *mut u8).add(4);
        assert_eq!(std::slice::from_raw_parts(site, 2), [0x0f, 0x85]);
        let patch = CodePatch::prepare(NonNull::new(site).unwrap(), std::slice::from_raw_parts(site, 6), &[0x90; 6]).unwrap();
        assert!(!std::hint::black_box(spawn)(true, 20, 30));
        assert!(std::hint::black_box(spawn)(false, 20, 30));
        patch::commit(&[(&patch, true)]).unwrap();
        assert!(std::hint::black_box(spawn)(true, 20, 30));
        assert!(!std::hint::black_box(spawn)(true, 31, 30));
        assert!(!std::hint::black_box(spawn)(false, 31, 30));
        patch::commit(&[(&patch, false)]).unwrap();
        assert!(!std::hint::black_box(spawn)(true, 20, 30));
    }
}
