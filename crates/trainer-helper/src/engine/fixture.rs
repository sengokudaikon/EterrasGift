//! Authored-player-only fault injection, absent from desktop helper builds.
use super::*;
use windows_sys::Win32::System::{
    Diagnostics::Debug::FlushInstructionCache,
    Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect},
    Threading::GetCurrentProcess,
};

#[unsafe(naked)]
unsafe extern "C" fn target() {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "mov eax,7", "ret", "nop", "nop", "nop", "nop", "nop");
}
unsafe extern "C" fn replacement() {}

#[unsafe(no_mangle)]
/// Fault-inject a second target after preparation, following a real dispatcher
/// change in one transaction. Called only by the owned Unity fixture on its main
/// thread, outside ExecuteTasks. Does not add any helper protocol operation.
/// # Safety
/// Uphold the authored initialized-runtime/start contract and serialize fixture calls.
pub unsafe extern "C" fn eg_helper_fixture_transaction_failure() -> u32 {
    // SAFETY: fixture build only; engine publishes a leaked hook before enabling it.
    unsafe {
        let Some(main) = FIXTURE_HOOK.load(SeqCst).as_ref() else {
            return 1;
        };
        if main.state() != HookState::Enabled {
            return 2;
        }
        let address = target as *const () as *mut u8;
        let bytes = std::slice::from_raw_parts(address, 16).to_vec();
        let Ok(second) = RetainedHook::prepare(NonNull::new(address).unwrap(), replacement as *const (), &bytes) else {
            return 3;
        };
        let mut old = 0;
        if VirtualProtect(address.cast(), 1, PAGE_EXECUTE_READWRITE, &raw mut old) == 0 {
            return 4;
        }
        address.write_volatile(0xcc);
        FlushInstructionCache(GetCurrentProcess(), address.cast(), 1);
        let mut ignored = 0;
        VirtualProtect(address.cast(), 1, old, &raw mut ignored);
        // Stock detour applies the dispatcher disable first, rejects this forged
        // second target, and then rolls the dispatcher back. Our wrapper also
        // verifies recovery and reports the intentionally unknown second entry.
        let outcome = hooks::commit(&[(main, false), (&second, true)]);
        let writable = VirtualProtect(address.cast(), 1, PAGE_EXECUTE_READWRITE, &raw mut old);
        if writable == 0 {
            return 5;
        }
        address.write_volatile(bytes[0]);
        let flushed = FlushInstructionCache(GetCurrentProcess(), address.cast(), 1);
        let restored = VirtualProtect(address.cast(), 1, old, &raw mut ignored);
        if flushed == 0 || restored == 0 {
            return 6;
        }
        // Check the pinned dependency's primary error too: an unrelated
        // suspension failure must not count as exercising partial rollback.
        if !matches!(outcome,Err(hooks::HookError::Transaction {error,recovery}) if format!("{error:?}")=="TargetModified" && !recovery.is_empty()) {
            return 7;
        }
        if main.state() != HookState::Enabled || second.state() != HookState::Disabled {
            return 8;
        }
        0
    }
}
