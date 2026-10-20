#![cfg(feature = "il2cpp")]
#![cfg(all(windows, target_arch = "x86_64"))]
//! Owned native target only. Relocation/registered unwind and real suspension/restore tests.
use eg_game_access::il2cpp::raw::{ExecuteTasks, MethodInfo};
use eg_game_access::scheduler::{HookError, SchedulerHook};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, RtlLookupFunctionEntry, RtlVirtualUnwind};

std::arch::global_asm!(
    r#"
    .text
    .p2align 4
    .global eg_owned_dispatch
    .seh_proc eg_owned_dispatch
eg_owned_dispatch:
    sub rsp,40
    .seh_stackalloc 40
    .seh_endprologue
    cmp byte ptr [rip + eg_owned_flag],0
    add rsp,40
    ret
    .seh_endproc
    .p2align 4
    .global eg_handler_dispatch
    .seh_proc eg_handler_dispatch
eg_handler_dispatch:
    sub rsp,40
    .seh_stackalloc 40
    .seh_endprologue
    cmp byte ptr [rip + eg_owned_flag],0
    add rsp,40
    ret
    .seh_handler eg_owned_handler,@except
    .seh_handlerdata
    .long 0
    .text
    .seh_endproc
eg_owned_handler:
    mov eax,1
    ret
    .data
eg_owned_flag:
    .byte 0
"#
);
unsafe extern "C-unwind" {
    fn eg_owned_dispatch(method: *const MethodInfo);
    fn eg_handler_dispatch(method: *const MethodInfo);
}
static SERIAL: Mutex<()> = Mutex::new(());
static ORIGINAL: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static CALLED: AtomicUsize = AtomicUsize::new(0);
struct Callers {
    running: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl Drop for Callers {
    fn drop(&mut self) {
        self.running.store(false, SeqCst);
        for worker in self.workers.drain(..) {
            // Even a failed transaction assertion must leave no private-target caller running.
            // Preserve the original test failure if a worker also panicked during unwinding.
            let _ = worker.join();
        }
    }
}
unsafe extern "C-unwind" fn callback(method: *const MethodInfo) {
    CALLED.fetch_add(1, SeqCst);
    // SAFETY: test publishes the retained exact-signature original before installation.
    let original: ExecuteTasks = unsafe { std::mem::transmute(ORIGINAL.load(SeqCst)) };
    // SAFETY: this authored target ignores but correctly forwards its argument.
    unsafe { original(method) };
}
fn prepare() -> SchedulerHook {
    let target = eg_owned_dispatch as *const () as *mut u8;
    // SAFETY: compiled owned target has the exact assembly/signature/unwind above. Serialized
    // tests exclusively own its patch, and no other thread enters it or manipulates contexts.
    unsafe {
        let bytes = std::slice::from_raw_parts(target, 11).try_into().unwrap();
        SchedulerHook::prepare(NonNull::new(target).unwrap(), callback, bytes, 16).unwrap()
    }
}

/// The test harness itself creates/finishes threads while another test holds SERIAL.
/// A changed census rejects before commit, after all acquired suspensions have been
/// released (a failed resume overrides that rejection with its own fatal error).
/// Only this exact precommit outcome permits another bounded transaction attempt.
fn commit_with_stable_census(hook: &mut SchedulerHook, installing: bool) {
    let target = eg_owned_dispatch as *const () as *const u8;
    let was_installed = hook.is_installed();
    assert_ne!(was_installed, installing);
    // SAFETY: SERIAL protects this authored target; all transaction callers are owned.
    let before: [u8; 11] = unsafe { std::slice::from_raw_parts(target, 11) }.try_into().unwrap();
    for attempt in 0..64 {
        // SAFETY: same exclusive private target/retained callback contract as each caller;
        // no target calls run here and every previous attempt has fully released suspension.
        let result = unsafe { if installing { hook.install() } else { hook.restore_entry() } };
        match result {
            Ok(()) => {
                assert_eq!(hook.is_installed(), installing);
                return;
            }
            Err(HookError::ThreadsChanged) => {
                assert_eq!(hook.is_installed(), was_installed, "rejected census attempt {attempt} changed installed state");
                // SAFETY: ThreadsChanged is returned before patching and after successful resume.
                assert_eq!(
                    // SAFETY: ThreadsChanged is returned before patching and after successful resume.
                    unsafe { std::slice::from_raw_parts(target, 11) },
                    before,
                    "rejected census attempt {attempt} changed entry bytes"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("unexpected transaction failure, never retried: {error}"),
        }
    }
    panic!("thread census did not stabilize within 64 precommit attempts");
}

#[test]
fn real_transaction_installs_forwards_restores_and_retains_the_original() {
    let _guard = SERIAL.lock().unwrap();
    CALLED.store(0, SeqCst);
    let mut hook = prepare();
    ORIGINAL.store(hook.original() as *const () as *mut u8, SeqCst);
    commit_with_stable_census(&mut hook, true);
    assert!(hook.is_installed());
    // SAFETY: authored target and callback implement the exact declared C-unwind signature.
    unsafe { eg_owned_dispatch(std::ptr::null()) };
    assert_eq!(CALLED.load(SeqCst), 1);
    commit_with_stable_census(&mut hook, false);
    // SAFETY: restored authored original and retained trampoline remain executable.
    unsafe {
        eg_owned_dispatch(std::ptr::null());
        (hook.original())(std::ptr::null());
    }
    assert_eq!(CALLED.load(SeqCst), 1);
    assert!(!hook.is_installed());
}

#[test]
fn every_relocated_instruction_boundary_unwinds_for_both_branch_outcomes() {
    let _guard = SERIAL.lock().unwrap();
    let hook = prepare();
    let body = hook.trampoline_address() as u64;
    let mut image_base = 0;
    // SAFETY: registered immutable table and process-retained code prepared by the adapter.
    let entry = unsafe { RtlLookupFunctionEntry(body, &raw mut image_base, std::ptr::null_mut()) };
    assert!(!entry.is_null());
    let mut stack = [0u64; 32];
    let caller_sp = (&raw mut stack[16]) as u64;
    let return_pc = 0x12345678u64;
    // SAFETY: owned slot; make the foreign stack read's backing store explicit.
    unsafe { (&raw mut stack[16]).write_volatile(return_pc) };
    for offset in [0, 4, 11, 17] {
        for zf in [0, 0x40] {
            // SAFETY: initialized Windows context and sufficiently large owned synthetic stack.
            #[repr(C, align(16))]
            struct Aligned(CONTEXT);
            // SAFETY: initialized Windows context and sufficiently large owned synthetic stack.
            let mut storage = Aligned(unsafe { std::mem::zeroed() });
            let context = &mut storage.0;
            context.Rip = body + offset;
            context.Rsp = if offset == 0 { caller_sp } else { caller_sp - 40 };
            context.EFlags = zf;
            let mut handler = std::ptr::null_mut();
            let mut frame = 0;
            // SAFETY: real registered entry plus initialized context/stack and output storage.
            unsafe { RtlVirtualUnwind(0, image_base, context.Rip, entry, context, &raw mut handler, &raw mut frame, std::ptr::null_mut()) };
            assert_eq!(context.Rip, return_pc, "PC +{offset}, ZF {zf}");
            assert_eq!(context.Rsp, caller_sp + 8, "PC +{offset}, ZF {zf}");
        }
    }
}

#[test]
fn stale_expected_bytes_are_rejected_without_changing_the_entry() {
    let _guard = SERIAL.lock().unwrap();
    let target = eg_owned_dispatch as *const () as *mut u8;
    // SAFETY: live immutable owned compiled target under exclusive test ownership.
    let original: [u8; 11] = unsafe { std::slice::from_raw_parts(target, 11) }.try_into().unwrap();
    let mut stale = original;
    stale[6] ^= 1;
    // SAFETY: valid owned metadata/code; intentional stale snapshot must be rejected before patching.
    let rejected = unsafe { SchedulerHook::prepare(NonNull::new(target).unwrap(), callback, stale, 16) };
    assert!(matches!(rejected, Err(HookError::OwnershipLost)));
    // SAFETY: still the same live owned target.
    assert_eq!(unsafe { std::slice::from_raw_parts(target, 11) }, original);
}

#[test]
fn handler_unwind_is_rejected_before_allocating_or_patching() {
    let _guard = SERIAL.lock().unwrap();
    let target = eg_handler_dispatch as *const () as *mut u8;
    // SAFETY: valid owned function with authentic handler-bearing unwind metadata; never invoked.
    let bytes: [u8; 11] = unsafe { std::slice::from_raw_parts(target, 11) }.try_into().unwrap();
    // SAFETY: live owned target, expected prefix/ABI; unsupported handler record must be rejected.
    let result = unsafe { SchedulerHook::prepare(NonNull::new(target).unwrap(), callback, bytes, 16) };
    assert!(matches!(result, Err(HookError::UnsupportedUnwind)));
}

#[test]
fn concurrent_known_callers_survive_repeated_transactions_and_join_before_cleanup() {
    let _guard = SERIAL.lock().unwrap();
    let mut hook = prepare();
    ORIGINAL.store(hook.original() as *const () as *mut u8, SeqCst);
    let running = Arc::new(AtomicBool::new(true));
    let mut callers = Callers {
        running: running.clone(),
        workers: Vec::new(),
    };
    for _ in 0..8 {
        let running = running.clone();
        callers.workers.push(std::thread::spawn(move || {
            while running.load(SeqCst) {
                // SAFETY: exact owned target/ABI; transaction accounts for these explicit callers.
                unsafe { eg_owned_dispatch(std::ptr::null()) };
                std::thread::yield_now();
            }
        }));
    }
    let mut committed = 0;
    for _ in 0..40 {
        // SAFETY: exclusive target, explicit joined caller set, retained published code; no special
        // APC/context restoration, other patchers or outside actors can enter this private target.
        let result = unsafe { if hook.is_installed() { hook.restore_entry() } else { hook.install() } };
        match result {
            Ok(()) => committed += 1,
            Err(HookError::ThreadInEntry | HookError::ThreadsChanged) => {}
            Err(error) => panic!("unexpected transaction failure: {error}"),
        }
    }
    running.store(false, SeqCst);
    for worker in callers.workers.drain(..) {
        worker.join().unwrap();
    }
    if hook.is_installed() {
        // Known callers have joined; unrelated test-harness threads may still finish.
        commit_with_stable_census(&mut hook, false);
    }
    assert!(committed >= 2, "must actually exercise both installation and restoration");
}
