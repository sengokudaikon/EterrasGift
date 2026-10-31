use super::*;
use crate::hooks::{
    leaf::LeafGate,
    patch::{self, CodePatch},
};
use std::{
    cell::Cell,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering::SeqCst},
};

thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; }
extern "C-unwind" fn original(actor: usize, method: usize) -> bool {
    assert_eq!(method, 0x12345678);
    CALLS.set(CALLS.get() + 1);
    actor == 0x2222
}
#[test]
fn actor_check_is_static_and_follows_the_current_player_reference() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let player = Box::leak(Box::new(AtomicUsize::new(0x1111)));
    let receiver = Receiver {
        online: NonNull::from(&*online).cast(),
        storage: NonNull::from(&*player).cast(),
        component: None,
    };
    // SAFETY: the actor is compared only; exact static actor/MethodInfo -> Boolean ABI and retained static storage.
    let gate = unsafe { LeafGate::prepare_override(effect(receiver), None, None).unwrap() };
    // SAFETY: matching original set before use.
    unsafe { gate.set_original(original as *const ()) };
    gate.set_enabled(true);
    // SAFETY: RCX is the static Actor parameter, not an instance receiver plus another argument.
    let call: extern "C-unwind" fn(usize, usize) -> bool = unsafe { std::mem::transmute(gate.entry()) };
    let check = |actor, result, originals| {
        let before = CALLS.get();
        assert_eq!(call(actor, 0x12345678), result);
        assert_eq!(CALLS.get(), before + originals);
    };
    check(0x1111, true, 0);
    check(0x2222, true, 1);
    check(0x3333, false, 1);
    check(0, false, 1);
    player.store(0x3333, SeqCst);
    check(0x1111, false, 1);
    check(0x3333, true, 0);
    player.store(0, SeqCst);
    check(0x3333, false, 1);
    player.store(0x3333, SeqCst);
    online.store(1, SeqCst);
    check(0x3333, false, 1);
    online.store(0, SeqCst);
    gate.set_enabled(false);
    check(0x3333, false, 1);
    gate.set_enabled(true);
    check(0x3333, true, 0);
}

#[test]
fn checked_tier_table_uses_the_games_existing_fallback_without_lowering_deep_tiers_and_restores() {
    use windows_sys::Win32::System::{
        Diagnostics::Debug::FlushInstructionCache,
        Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualFree},
        Threading::GetCurrentProcess,
    };
    struct Page(*mut u8);
    impl Drop for Page {
        fn drop(&mut self) {
            // SAFETY: this private fixture page drops after every call and CodePatch owner, including on assertion failure.
            unsafe {
                VirtualFree(self.0.cast(), 0, MEM_RELEASE);
            }
        }
    }
    // SAFETY: private complete x64 Int32(Int32,MethodInfo) decision table, initialized before publishing calls.
    unsafe {
        let allocation = Page(VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE).cast::<u8>());
        let page = allocation.0;
        assert!(!page.is_null());
        let code: Vec<u8> = TIER_PATTERN.iter().map(|byte| *byte as u8).collect();
        std::ptr::copy_nonoverlapping(code.as_ptr(), page, code.len());
        assert_ne!(FlushInstructionCache(GetCurrentProcess(), page.cast(), code.len()), 0);
        let call: extern "C-unwind" fn(i32, usize) -> i32 = std::mem::transmute(page);
        let owners: Vec<_> = [21, 27, 33]
            .into_iter()
            .map(|at| CodePatch::prepare(NonNull::new(page.add(at)).unwrap(), &code[at..at + 4], &[0xc8, 0, 0, 0]).unwrap())
            .collect();
        let expected = [(-1, 200), (0, 50), (1, 65), (2, 75), (3, 200), (i32::MAX, 200)];
        for (tier, cap) in expected {
            assert_eq!(call(tier, 0x1234), cap);
        }
        patch::commit(&owners.iter().map(|owner| (owner, true)).collect::<Vec<_>>()).unwrap();
        for (tier, _) in expected {
            assert_eq!(call(tier, 0x1234), 200);
        }
        patch::commit(&owners.iter().map(|owner| (owner, false)).collect::<Vec<_>>()).unwrap();
        for (tier, cap) in expected {
            assert_eq!(call(tier, 0x1234), cap);
        }
        assert_eq!(std::slice::from_raw_parts(page, code.len()), code);
    }
}
