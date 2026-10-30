//! The two private pages a gateway lives in: its code first (read-write until published, then read-execute) and its
//! data after. They are retained until process exit, or released when publishing fails.
use super::{HookError, failure};
use windows_sys::Win32::System::{
    Diagnostics::Debug::{FlushInstructionCache, IMAGE_RUNTIME_FUNCTION_ENTRY, RtlAddFunctionTable},
    Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect},
    Threading::GetCurrentProcess,
};

pub(super) const PAGE: usize = 4096;

/// Two fresh read-write pages; `operation` names the failure.
pub(super) fn allocate(operation: &'static str) -> Result<*mut u8, HookError> {
    // SAFETY: a fresh private allocation that nothing else holds.
    let allocation = unsafe { VirtualAlloc(std::ptr::null(), 2 * PAGE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }.cast::<u8>();
    if allocation.is_null() { Err(failure(operation)) } else { Ok(allocation) }
}

/// The unwind entry for `length` bytes of code whose four-byte prologue is `sub rsp, frame` (one UWOP_ALLOC_SMALL),
/// kept in the data page: the entry at 64, its unwind info at 80. No helper frame or state is on the unwind path.
/// # Safety
/// `allocation` is from `allocate`, not yet published, and its data page keeps bytes 64..88 for this.
pub(super) unsafe fn unwind(allocation: *mut u8, length: usize, frame: u8) -> *mut IMAGE_RUNTIME_FUNCTION_ENTRY {
    // SAFETY: the caller's unpublished private pages; the entry is written whole before its union is set.
    unsafe {
        let table = allocation.add(PAGE + 64).cast::<IMAGE_RUNTIME_FUNCTION_ENTRY>();
        table.write(IMAGE_RUNTIME_FUNCTION_ENTRY {
            BeginAddress: 0,
            EndAddress: length as u32,
            Anonymous: std::mem::zeroed(),
        });
        (*table).Anonymous.UnwindData = (PAGE + 80) as u32;
        // Version 1, prologue 4, one code: UWOP_ALLOC_SMALL at offset 4.
        let allocation_code = ((frame - 8) / 8) << 4 | 2;
        std::ptr::copy_nonoverlapping([1u8, 4, 1, 0, 4, allocation_code, 0, 0].as_ptr(), allocation.add(PAGE + 80), 8);
        table
    }
}

/// Makes the code page executable, flushes it and registers its unwind entry, if it has one. On failure both pages are
/// released and nothing was published; `operation` names the failure.
/// # Safety
/// `allocation` is from `allocate`, its code and data fully written, and nothing runs its code yet.
pub(super) unsafe fn publish(allocation: *mut u8, unwind: Option<*mut IMAGE_RUNTIME_FUNCTION_ENTRY>, operation: &'static str) -> Result<(), HookError> {
    // SAFETY: the caller's initialized, unpublished private pages; the error is read before the pages are released.
    unsafe {
        let mut old = 0;
        if VirtualProtect(allocation.cast(), PAGE, PAGE_EXECUTE_READ, &raw mut old) == 0
            || FlushInstructionCache(GetCurrentProcess(), allocation.cast(), PAGE) == 0
            || unwind.is_some_and(|table| !RtlAddFunctionTable(table, 1, allocation as u64))
        {
            let error = failure(operation);
            VirtualFree(allocation.cast(), 0, MEM_RELEASE);
            return Err(error);
        }
        Ok(())
    }
}
