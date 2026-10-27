#![cfg(feature = "il2cpp")]
#![cfg(all(windows, target_arch = "x86_64"))]
//! Owned native target only. Relocation/registered unwind and real suspension/restore tests.
use eg_game_access::hooks::{self, HookError, HookState, RetainedHook};
use eg_game_access::il2cpp::raw::{ExecuteTasks, MethodInfo};
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
    .p2align 4
    .global eg_batch_dispatch
    .seh_proc eg_batch_dispatch
eg_batch_dispatch:
    sub rsp,40
    .seh_stackalloc 40
    .seh_endprologue
    cmp byte ptr [rip + eg_owned_flag],0
    add rsp,40
    ret
    .seh_endproc
    .p2align 4
    .global eg_call_dispatch
    .seh_proc eg_call_dispatch
eg_call_dispatch:
    sub rsp,40
    .seh_stackalloc 40
    .seh_endprologue
    call eg_batch_dispatch
    add rsp,40
    ret
    .seh_endproc
eg_owned_handler:
    mov eax,1
    ret
    .p2align 4
    .global eg_rex_dispatch
    .seh_proc eg_rex_dispatch
eg_rex_dispatch:
    .byte 0x40
    push rdi
    .seh_pushreg rdi
    sub rsp,32
    .seh_stackalloc 32
    .seh_endprologue
    add rsp,32
    pop rdi
    ret
    .seh_endproc
    .p2align 4
    .global eg_rex_rip_dispatch
    .seh_proc eg_rex_rip_dispatch
eg_rex_rip_dispatch:
    .byte 0x40
    push rdi
    .seh_pushreg rdi
    .seh_endprologue
    cmp byte ptr [rip + eg_owned_flag],0
    pop rdi
    ret
    .seh_endproc
    .data
eg_owned_flag:
    .byte 0
"#
);
unsafe extern "C-unwind" {
    fn eg_owned_dispatch(method: *const MethodInfo);
    fn eg_handler_dispatch(method: *const MethodInfo);
    fn eg_batch_dispatch(method: *const MethodInfo);
    fn eg_call_dispatch(method: *const MethodInfo);
    fn eg_rex_dispatch(method: *const MethodInfo);
    fn eg_rex_rip_dispatch(method: *const MethodInfo);
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
fn original(hook: &RetainedHook) -> ExecuteTasks {
    // SAFETY: these owned fixture hooks all have the exact ExecuteTasks ABI.
    unsafe { std::mem::transmute(hook.trampoline()) }
}
fn prepare() -> RetainedHook {
    let target = eg_owned_dispatch as *const () as *mut u8;
    // SAFETY: compiled owned target has the exact assembly/signature/unwind above. Serialized
    // tests exclusively own its patch, and no other thread enters it or manipulates contexts.
    unsafe {
        let bytes: [u8; 11] = std::slice::from_raw_parts(target, 11).try_into().unwrap();
        RetainedHook::prepare(NonNull::new(target).unwrap(), callback as *const (), &bytes).unwrap()
    }
}

fn toggle(hook: &RetainedHook, installing: bool) {
    // SAFETY: SERIAL protects the owned target; original/callback/unwind storage stays retained.
    unsafe { hooks::commit(&[(hook, installing)]) }.unwrap();
}

#[test]
fn real_transaction_installs_forwards_restores_and_retains_the_original() {
    let _guard = SERIAL.lock().unwrap();
    CALLED.store(0, SeqCst);
    let hook = prepare();
    ORIGINAL.store(original(&hook) as *const () as *mut u8, SeqCst);
    toggle(&hook, true);
    assert!(hook.state() == HookState::Enabled);
    // SAFETY: authored target and callback implement the exact declared C-unwind signature.
    unsafe { eg_owned_dispatch(std::ptr::null()) };
    assert_eq!(CALLED.load(SeqCst), 1);
    toggle(&hook, false);
    // SAFETY: restored authored original and retained trampoline remain executable.
    unsafe {
        eg_owned_dispatch(std::ptr::null());
        (original(&hook))(std::ptr::null());
    }
    assert_eq!(CALLED.load(SeqCst), 1);
    assert_eq!(hook.state(), HookState::Disabled);
}

#[test]
fn every_relocated_instruction_boundary_unwinds_for_both_branch_outcomes() {
    let _guard = SERIAL.lock().unwrap();
    let hook = prepare();
    let body = hook.trampoline() as u64;
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
fn redundant_rex_push_preserves_prologue_offsets_and_unwinds_at_every_boundary() {
    let _guard = SERIAL.lock().unwrap();
    let target = eg_rex_dispatch as *const () as *mut u8;
    // SAFETY: authored prologue and matching retained callback under SERIAL.
    let bytes = unsafe { std::slice::from_raw_parts(target, 16) };
    // SAFETY: exact authored target/callback ABI and unwind data, exclusively owned under SERIAL.
    let hook = unsafe { RetainedHook::prepare(NonNull::new(target).unwrap(), callback as *const (), bytes) }.unwrap();
    // SAFETY: the prepared trampoline retains the six original prologue bytes.
    assert_eq!(unsafe { std::slice::from_raw_parts(hook.trampoline().cast::<u8>(), 6) }, &bytes[..6]);
    ORIGINAL.store(original(&hook) as *const () as *mut u8, SeqCst);
    toggle(&hook, true);
    // SAFETY: owned same-signature target; callback forwards through restored prefix.
    unsafe { eg_rex_dispatch(std::ptr::null()) };
    toggle(&hook, false);
    let body = hook.trampoline() as u64;
    let mut base = 0;
    // SAFETY: immutable registered code/table retained for the process lifetime.
    let entry = unsafe { RtlLookupFunctionEntry(body, &raw mut base, std::ptr::null_mut()) };
    assert!(!entry.is_null());
    let mut stack = [0u64; 32];
    stack[16] = 0x12345678;
    stack[15] = 0xabcdef;
    let caller_sp = (&raw mut stack[16]) as u64;
    for offset in [0, 2, 6, 12] {
        for flags in [0, 0x40] {
            #[repr(C, align(16))]
            struct Aligned(CONTEXT);
            // SAFETY: zeroed Windows context, backed by the owned synthetic stack.
            let mut storage = Aligned(unsafe { std::mem::zeroed() });
            let context = &mut storage.0;
            context.Rip = body + offset;
            context.Rsp = caller_sp
                - match offset {
                    0 => 0,
                    2 => 8,
                    _ => 40,
                };
            context.Rdi = if offset == 0 { 0xabcdef } else { 0 };
            context.EFlags = flags;
            let mut handler = std::ptr::null_mut();
            let mut frame = 0;
            // SAFETY: real retained unwind table and initialized context/stack/output storage.
            unsafe { RtlVirtualUnwind(0, base, context.Rip, entry, context, &raw mut handler, &raw mut frame, std::ptr::null_mut()) };
            assert_eq!(
                (context.Rip, context.Rsp, context.Rdi),
                (0x12345678, caller_sp + 8, 0xabcdef),
                "offset {offset}, flags {flags}"
            );
        }
    }
    let target = eg_rex_rip_dispatch as *const () as *mut u8;
    // SAFETY: authored immutable target; changed offsets plus RIP relocation must still refuse.
    let rejected = unsafe { RetainedHook::prepare(NonNull::new(target).unwrap(), callback as *const (), std::slice::from_raw_parts(target, 16)) };
    assert!(matches!(rejected, Err(HookError::UnsupportedUnwind)));
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
    let rejected = unsafe { RetainedHook::prepare(NonNull::new(target).unwrap(), callback as *const (), &stale) };
    assert!(matches!(rejected, Err(HookError::OwnershipLost)));
    // SAFETY: still the same live owned target.
    assert_eq!(unsafe { std::slice::from_raw_parts(target, 11) }, original);
}

#[test]
fn handler_unwind_keeps_the_stock_trampoline_without_registering_partial_metadata() {
    let _guard = SERIAL.lock().unwrap();
    let target = eg_handler_dispatch as *const () as *mut u8;
    // SAFETY: valid owned function with authentic handler-bearing unwind metadata; never invoked.
    let bytes: [u8; 11] = unsafe { std::slice::from_raw_parts(target, 11) }.try_into().unwrap();
    // SAFETY: live owned target and callback have the declared ABI; neither runs during preparation.
    let hook = unsafe { RetainedHook::prepare(NonNull::new(target).unwrap(), callback as *const (), &bytes) }.unwrap();
    assert_eq!(hook.state(), HookState::Disabled);
    let mut image_base = 0;
    // SAFETY: query the retained trampoline address; the output base is a live local slot.
    assert!(unsafe { RtlLookupFunctionEntry(hook.trampoline() as u64, &raw mut image_base, std::ptr::null_mut()) }.is_null());
}

#[test]
fn concurrent_known_callers_survive_repeated_transactions_and_join_before_cleanup() {
    let _guard = SERIAL.lock().unwrap();
    let hook = prepare();
    ORIGINAL.store(original(&hook) as *const () as *mut u8, SeqCst);
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
    for _ in 0..40 {
        // SAFETY: exclusive target, explicit joined caller set, retained published code; no special
        // APC/context restoration, other patchers or outside actors can enter this private target.
        unsafe {
            if hook.state() == HookState::Enabled {
                hooks::commit(&[(&hook, false)])
            } else {
                hooks::commit(&[(&hook, true)])
            }
        }
        .unwrap();
    }
    running.store(false, SeqCst);
    for worker in callers.workers.drain(..) {
        worker.join().unwrap();
    }
    if hook.state() == HookState::Enabled {
        // Known callers have joined; unrelated test-harness threads may still finish.
        toggle(&hook, false);
    }
}

#[test]
fn copied_call_is_refused_before_a_hook_can_be_published() {
    let _guard = SERIAL.lock().unwrap();
    let target = eg_call_dispatch as *const () as *mut u8;
    // SAFETY: immutable owned assembly and matching callback ABI, never called through a refused hook.
    let bytes: [u8; 14] = unsafe { std::slice::from_raw_parts(target, 14) }.try_into().unwrap();
    // SAFETY: module-owned fixture; SERIAL excludes every writer.
    let result = unsafe { RetainedHook::prepare(NonNull::new(target).unwrap(), callback as *const (), &bytes) };
    assert!(matches!(result, Err(HookError::UnsupportedPrefix)));
    // SAFETY: same live fixture, still exclusively owned.
    assert_eq!(unsafe { std::slice::from_raw_parts(target, 14) }, bytes);
}

#[test]
fn batch_failure_restores_the_first_hook_and_reports_the_unknown_second_entry() {
    use windows_sys::Win32::System::{
        Diagnostics::Debug::FlushInstructionCache,
        Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect},
        Threading::GetCurrentProcess,
    };
    let _guard = SERIAL.lock().unwrap();
    let first = prepare();
    let address = eg_batch_dispatch as *const () as *mut u8;
    // SAFETY: immutable assembly-owned second target under SERIAL.
    let bytes: [u8; 11] = unsafe { std::slice::from_raw_parts(address, 11) }.try_into().unwrap();
    // SAFETY: owned target/callback have the same declared ABI.
    let second = unsafe { RetainedHook::prepare(NonNull::new(address).unwrap(), callback as *const (), &bytes) }.unwrap();
    // SAFETY: no caller enters this private second target. A forged byte causes
    // failure after the first hook has been written in the stock transaction.
    let error = unsafe {
        let mut old = 0;
        assert_ne!(VirtualProtect(address.cast(), 1, PAGE_EXECUTE_READWRITE, &raw mut old), 0);
        address.write_volatile(0xcc);
        let result = hooks::commit(&[(&first, true), (&second, true)]);
        address.write_volatile(bytes[0]);
        assert_ne!(FlushInstructionCache(GetCurrentProcess(), address.cast(), 1), 0);
        let mut ignored = 0;
        assert_ne!(VirtualProtect(address.cast(), 1, old, &raw mut ignored), 0);
        result.unwrap_err()
    };
    assert_eq!(first.state(), HookState::Disabled);
    assert_eq!(second.state(), HookState::Disabled);
    assert!(matches!(error,HookError::Transaction {error:detour::Error::TargetModified,recovery} if !recovery.is_empty()));
}
