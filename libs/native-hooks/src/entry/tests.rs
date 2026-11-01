use super::*;
use std::ptr::NonNull;
use windows_sys::Win32::System::{
    Diagnostics::Debug::FlushInstructionCache,
    Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect},
    Threading::GetCurrentProcess,
};

/// A function of its own for each use, so that tests running together share no target: five nops a detour can take,
/// then `mov eax,1; ret`, whose immediate an interior patch can change. The line number keeps identical code from being
/// folded into one function by the linker.
macro_rules! target {
    ($name:ident) => {
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn $name() -> u32 {
            core::arch::naked_asm!(".rept 5", "nop", ".endr", "mov eax,1", "ret", concat!(".long ", line!()), ".rept 16", "nop", ".endr");
        }
    };
}
type Target = unsafe extern "C-unwind" fn() -> u32;

unsafe extern "C-unwind" fn replacement() -> u32 {
    2
}
fn detour(target: Target) -> Entry {
    let address = target as *mut u8;
    // SAFETY: an authored process-lifetime entry of this exact ABI, owned by the one test that names it.
    let hook = unsafe { RetainedHook::prepare(NonNull::new(address).unwrap(), replacement as *const (), &address.cast::<[u8; 16]>().read_unaligned()) }.unwrap();
    Entry::Detour(hook)
}
fn patch(target: Target) -> Entry {
    let address = target as *mut u8;
    // SAFETY: the immediate of `mov eax,1`, owned by the one test that names it, with no return address inside it.
    let patch = unsafe { CodePatch::prepare(NonNull::new(address.add(6)).unwrap(), &[1], &[3]) }.unwrap();
    Entry::Patch(patch)
}
fn call(target: Target) -> u32 {
    // SAFETY: the authored ABI, kept by every state of the entry; black_box keeps the call from being folded across writes.
    unsafe { std::hint::black_box(target)() }
}
fn commit_all(entries: &[&Entry], on: bool) -> Result<(), String> {
    let changes: Vec<_> = entries.iter().map(|entry| (*entry, on)).collect();
    // SAFETY: authored ranges and ABIs owned by the calling test; this thread is outside them while committing.
    unsafe { commit(&changes) }
}
fn states(entries: &[&Entry]) -> Vec<HookState> {
    entries.iter().map(|entry| entry.state()).collect()
}

#[test]
fn a_mixed_group_goes_on_and_off_together() {
    target!(detoured);
    target!(interior);
    let (first, second) = (detour(detoured), patch(interior));
    let group = [&first, &second];
    assert_eq!(states(&group), [HookState::Disabled; 2]);
    commit_all(&group, true).unwrap();
    assert_eq!(states(&group), [HookState::Enabled; 2]);
    assert_eq!((call(detoured), call(interior)), (2, 3));
    commit_all(&group, false).unwrap();
    assert_eq!(states(&group), [HookState::Disabled; 2]);
    assert_eq!((call(detoured), call(interior)), (1, 1));
}

#[test]
fn a_group_of_one_family_uses_that_familys_transaction() {
    target!(detoured);
    target!(interior);
    let (first, second) = (detour(detoured), patch(interior));
    commit_all(&[&first], true).unwrap();
    assert_eq!(call(detoured), 2);
    commit_all(&[&second], true).unwrap();
    assert_eq!(call(interior), 3);
    commit_all(&[&first], false).unwrap();
    commit_all(&[&second], false).unwrap();
    assert_eq!((call(detoured), call(interior)), (1, 1));
}

#[test]
fn a_failed_patch_restores_the_detours_already_written_and_says_so() {
    target!(detoured);
    target!(interior);
    target!(failing);
    let (first, second, third) = (detour(detoured), patch(interior), patch(failing));
    let group = [&first, &second, &third];
    let error = crate::patch::with_write_failure(1, || commit_all(&group, true)).unwrap_err();
    assert_eq!(
        error,
        "Mixed hook: Code-patch transaction: Code-patch bytes or protection no longer agree; recovery failures: []; detour recovery: Ok(()); previous bytes restored: true"
    );
    assert_eq!(states(&group), [HookState::Disabled; 3]);
    assert_eq!((call(detoured), call(interior), call(failing)), (1, 1, 1));
    // Every owner is retained and usable after the failure.
    commit_all(&group, true).unwrap();
    assert_eq!((call(detoured), call(interior), call(failing)), (2, 3, 3));
    commit_all(&group, false).unwrap();
}

#[test]
fn overlapping_ranges_refuse_before_anything_is_written() {
    target!(both);
    let first = detour(both);
    // SAFETY: a nop inside the detour's own range, owned by this test, which never commits it.
    let second = Entry::Patch(unsafe { CodePatch::prepare(NonNull::new((both as *mut u8).add(2)).unwrap(), &[0x90], &[0xcc]) }.unwrap());
    assert_eq!(commit_all(&[&first, &second], true).unwrap_err(), "Hook entry ranges overlap");
    assert_eq!(states(&[&first, &second]), [HookState::Disabled; 2]);
    assert_eq!(call(both), 1);
}

#[test]
fn foreign_bytes_refuse_a_mixed_activation_before_anything_is_written() {
    target!(detoured);
    target!(interior);
    let (first, second) = (detour(detoured), patch(interior));
    let immediate = (interior as *mut u8).wrapping_add(6);
    let forge = |value: u8| {
        // SAFETY: this test's own immediate, with nothing running it: another writer's change, then putting it back.
        unsafe {
            let mut old = 0;
            assert_ne!(VirtualProtect(immediate.cast(), 1, PAGE_EXECUTE_READWRITE, &raw mut old), 0);
            immediate.write_volatile(value);
            let mut ignored = 0;
            assert_ne!(VirtualProtect(immediate.cast(), 1, old, &raw mut ignored), 0);
            assert_ne!(FlushInstructionCache(GetCurrentProcess(), immediate.cast(), 1), 0);
        }
    };
    forge(7);
    assert_eq!(second.state(), HookState::Unknown);
    assert_eq!(commit_all(&[&first, &second], true).unwrap_err(), "Mixed hook bytes no longer agree; nothing was written");
    assert_eq!(first.state(), HookState::Disabled);
    assert_eq!(call(detoured), 1);
    forge(1);
    commit_all(&[&first, &second], true).unwrap();
    commit_all(&[&first, &second], false).unwrap();
}

#[test]
fn cleanup_attempts_both_families_and_reports_the_one_that_failed() {
    target!(detoured);
    target!(interior);
    target!(failing);
    let (first, second, third) = (detour(detoured), patch(interior), patch(failing));
    let group = [&first, &second, &third];
    commit_all(&group, true).unwrap();
    let error = crate::patch::with_write_failure(1, || commit_all(&group, false)).unwrap_err();
    assert_eq!(error, "Code-patch transaction: Code-patch bytes or protection no longer agree; recovery failures: []");
    assert_eq!(first.state(), HookState::Disabled, "the detours were restored regardless");
    assert_eq!(states(&[&second, &third]), [HookState::Enabled; 2], "the patch transaction recovered what it had before");
    commit_all(&group, false).unwrap();
    assert_eq!(states(&group), [HookState::Disabled; 3]);
}
