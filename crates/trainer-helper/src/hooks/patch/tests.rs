use super::*;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, VirtualAlloc, VirtualFree};

struct Page(*mut u8);
impl Page {
    fn new() -> Self {
        // SAFETY: private authored executable page; no caller sees it until initialization/protection completes.
        unsafe {
            let page = Self(VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE).cast());
            assert!(!page.0.is_null());
            let code = [0x31, 0xc0, 0x85, 0xc9, 0x74, 0x05, 0xb8, 1, 0, 0, 0, 0xc3];
            for offset in [0, 64] {
                std::ptr::copy_nonoverlapping(code.as_ptr(), page.0.add(offset), code.len());
            }
            let mut old = 0;
            assert_ne!(VirtualProtect(page.0.cast(), 4096, PAGE_EXECUTE_READ, &raw mut old), 0);
            assert_ne!(FlushInstructionCache(GetCurrentProcess(), page.0.cast(), 4096), 0);
            page
        }
    }
    fn patch(&self, offset: usize) -> CodePatch {
        // SAFETY: live authored Jcc at a known complete-instruction span, no aliases or interior return addresses.
        unsafe { CodePatch::prepare(NonNull::new(self.0.add(offset + 4)).unwrap(), &[0x74, 5], &[0x90, 0x90]).unwrap() }
    }
    fn call(&self, offset: usize, condition: bool) -> u32 {
        // SAFETY: authored Windows x64 u32(bool) function at each initialized entry.
        unsafe { std::mem::transmute::<*mut u8, unsafe extern "C-unwind" fn(bool) -> u32>(self.0.add(offset))(condition) }
    }
}
impl Drop for Page {
    fn drop(&mut self) {
        // SAFETY: private allocation, after all calls and code owners in this fixture have ended.
        unsafe {
            VirtualFree(self.0.cast(), 0, MEM_RELEASE);
        }
    }
}

#[test]
fn resolver_requires_one_complete_bounded_pattern_and_a_contained_change() {
    let bytes = [0xaa, 0xbb, 0xcc, 0xdd];
    assert_eq!(find_unique(&bytes, &[0xaa, -1, 0xcc], 1, &[0x90]).unwrap(), 1);
    assert!(matches!(find_unique(&bytes, &[0xff], 0, &[0x90]), Err(PatchError::Missing)));
    assert!(matches!(find_unique(&[0xaa, 0xaa], &[0xaa], 0, &[0x90]), Err(PatchError::Ambiguous)));
    assert!(matches!(find_unique(&bytes, &[0xaa, -1], 1, &[0x90, 0x90]), Err(PatchError::InvalidPattern)));
    assert!(matches!(find_unique(&bytes, &[256], 0, &[0x90]), Err(PatchError::InvalidPattern)));
}

#[test]
fn grouped_writes_read_actual_bytes_and_independent_owners_restore_only_their_ranges() {
    let page = Page::new();
    let first = page.patch(0);
    let second = page.patch(64);
    assert_eq!(page.call(0, false), 0);
    assert_eq!(page.call(64, false), 0);
    assert_eq!(page.call(0, true), 1);
    // SAFETY: authored ranges, this thread outside them; peer census is applied by the transaction.
    unsafe {
        commit(&[(&first, true), (&second, true)]).unwrap();
    }
    assert_eq!(first.state(), HookState::Enabled);
    assert_eq!(second.state(), HookState::Enabled);
    assert_eq!(page.call(0, false), 1);
    assert_eq!(page.call(64, false), 1);
    // SAFETY: same authored exclusive ranges, with no caller in the changing instructions.
    unsafe {
        commit(&[(&first, false)]).unwrap();
    }
    assert_eq!(page.call(0, false), 0);
    assert_eq!(page.call(64, false), 1);
    // SAFETY: the second independent owner still retains its unchanged executable span.
    unsafe {
        commit(&[(&second, false)]).unwrap();
    }
    assert_eq!(first.state(), HookState::Disabled);
    assert_eq!(second.state(), HookState::Disabled);
    assert_eq!(page.call(64, false), 0);
}

#[test]
fn failed_second_write_recovers_the_group_and_keeps_its_owners_usable() {
    let page = Page::new();
    let first = page.patch(0);
    let second = page.patch(64);
    FAIL_WRITE.set(Some(1));
    // SAFETY: authored code/ownership contracts; the fixture injects failure before the second store.
    let failed = unsafe { commit(&[(&first, true), (&second, true)]) };
    FAIL_WRITE.set(None);
    assert!(failed.is_err());
    assert_eq!(first.state(), HookState::Disabled);
    assert_eq!(second.state(), HookState::Disabled);
    assert_eq!(page.call(0, false), 0);
    assert_eq!(page.call(64, false), 0);
    // SAFETY: fixture retains both ranges after recovery and submits fresh explicit transactions.
    unsafe {
        commit(&[(&first, true), (&second, true)]).unwrap();
        commit(&[(&first, false), (&second, false)]).unwrap();
    }
}

#[test]
fn overlap_and_foreign_bytes_refuse_without_overwriting_the_other_owner() {
    let page = Page::new();
    let first = page.patch(0);
    // SAFETY: same readable span, deliberately aliased to test the ownership refusal.
    let overlapping = unsafe { CodePatch::prepare(NonNull::new(page.0.add(4)).unwrap(), &[0x74, 5], &[0x90, 0x90]) };
    assert!(matches!(overlapping, Err(PatchError::Overlap)));
    // SAFETY: no calls are in flight; simulate an external writer changing this private fixture.
    unsafe {
        let mut old = 0;
        assert_ne!(VirtualProtect(page.0.cast(), 4096, PAGE_EXECUTE_READWRITE, &raw mut old), 0);
        *page.0.add(4) = 0x75;
        let mut ignored = 0;
        assert_ne!(VirtualProtect(page.0.cast(), 4096, old, &raw mut ignored), 0);
        assert!(matches!(commit(&[(&first, true)]), Err(PatchError::OwnershipLost)));
        assert_eq!(*page.0.add(4), 0x75);
        assert_ne!(VirtualProtect(page.0.cast(), 4096, PAGE_EXECUTE_READWRITE, &raw mut old), 0);
        *page.0.add(4) = 0x74;
        assert_ne!(VirtualProtect(page.0.cast(), 4096, old, &raw mut ignored), 0);
    }
    assert_eq!(first.state(), HookState::Disabled);
}

#[test]
fn a_peer_inside_the_range_refuses_the_write_and_is_resumed_unchanged() {
    use std::{
        sync::{Arc, atomic::AtomicBool},
        time::{Duration, Instant},
    };
    #[unsafe(naked)]
    unsafe extern "C-unwind" fn spin(_: &AtomicBool, _: &AtomicBool) -> u32 {
        core::arch::naked_asm!("mov byte ptr [rdx],1", "2:", "cmp byte ptr [rcx],0", "jne 2b", "mov eax,42", "ret", "nop");
    }
    let block = Arc::new(AtomicBool::new(true));
    let entered = Arc::new(AtomicBool::new(false));
    // SAFETY: the five-byte loop has no interior return address; peers are suspended before considering a write.
    let patch = unsafe {
        let address = (spin as *mut u8).add(3);
        CodePatch::prepare(NonNull::new(address).unwrap(), std::slice::from_raw_parts(address, 5), &[0x90; 5]).unwrap()
    };
    let (running, ready) = (block.clone(), entered.clone());
    // SAFETY: atomic byte storage and authored function remain live until the worker joins.
    let worker = std::thread::spawn(move || unsafe { std::hint::black_box(spin)(&running, &ready) });
    let deadline = Instant::now() + Duration::from_secs(3);
    while !entered.load(SeqCst) && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let started = entered.load(SeqCst);
    let result = if started {
        // SAFETY: owned function and range are retained; the backend must refuse the peer observed inside them.
        unsafe { commit(&[(&patch, true)]) }
    } else {
        Err(PatchError::OwnershipLost)
    };
    block.store(false, SeqCst);
    assert_eq!(worker.join().unwrap(), 42);
    assert!(started);
    assert!(matches!(result, Err(PatchError::ThreadInPatch)));
    assert_eq!(patch.state(), HookState::Disabled);
}
