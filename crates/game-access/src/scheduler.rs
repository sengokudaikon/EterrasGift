//! Bounded Windows x64 entry adapter for the verified SUB RSP40 / RIP-relative CMP shape.
//!
//! This is not an arbitrary-target hook engine. Published code/tables and both modules remain
//! retained until process exit. Execution permissions, target identity and control-flow ownership
//! remain explicit unsafe obligations. No game discovery, loader or bootstrap is provided here.
mod threads;

use crate::il2cpp::raw::ExecuteTasks;
use std::ffi::c_void;
use std::ptr::NonNull;
use threads::ThreadSet;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Diagnostics::Debug::{FlushInstructionCache, IMAGE_RUNTIME_FUNCTION_ENTRY, RtlAddFunctionTable, RtlDeleteFunctionTable, RtlLookupFunctionEntry};
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessMitigationPolicy, ProcessControlFlowGuardPolicy, ProcessDEPPolicy, ProcessDynamicCodePolicy, ProcessUserShadowStackPolicy,
};

const UNWIND: [u8; 8] = [1, 4, 1, 0, 4, 0x42, 0, 0];
const TABLE_OFFSET: usize = 64;
const RELAY_OFFSET: usize = 128;
const PAGE: usize = 4096;
static TRANSACTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookError {
    UnsupportedPrefix,
    EntryCrossesPage,
    TransactionPoisoned,
    UnsupportedUnwind,
    UnsupportedPolicy,
    OutOfRange,
    AlreadyInstalled,
    NotInstalled,
    OwnershipLost,
    ThreadLimit,
    ThreadCensus,
    ThreadsChanged,
    ThreadInEntry,
    Win32 { operation: &'static str, code: u32 },
    NativeQuery(i32),
}
impl std::fmt::Display for HookError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Scheduler entry adapter: {self:?}")
    }
}
impl std::error::Error for HookError {}
fn failure(operation: &'static str) -> HookError {
    // SAFETY: thread-local Windows error query.
    HookError::Win32 {
        operation,
        // SAFETY: thread-local Windows error query.
        code: unsafe { GetLastError() },
    }
}

fn displacement(from: usize, to: usize) -> Result<[u8; 4], HookError> {
    i32::try_from(to as i128 - from as i128).map(i32::to_le_bytes).map_err(|_| HookError::OutOfRange)
}

struct Encoded {
    code: [u8; 23],
    relay: [u8; 14],
    patch: [u8; 5],
}
fn encode(target: usize, body: usize, callback: usize, original: [u8; 11]) -> Result<Encoded, HookError> {
    if original[..6] != [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d] || original[10] != 0 {
        return Err(HookError::UnsupportedPrefix);
    }
    let original_disp = i32::from_le_bytes(original[6..10].try_into().unwrap());
    let operand = usize::try_from(target as i128 + 11 + original_disp as i128).map_err(|_| HookError::OutOfRange)?;
    let mut code = [0; 23];
    code[..11].copy_from_slice(&original);
    code[6..10].copy_from_slice(&displacement(body + 11, operand)?);
    for (offset, opcode) in [(11, 0x84), (17, 0x85)] {
        code[offset..offset + 2].copy_from_slice(&[0x0f, opcode]);
        code[offset + 2..offset + 6].copy_from_slice(&displacement(body + offset + 6, target + 11)?);
    }
    let mut relay = [0; 14];
    relay[..6].copy_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
    relay[6..].copy_from_slice(&(callback as u64).to_le_bytes());
    let mut patch = [0; 5];
    patch[0] = 0xe9;
    patch[1..].copy_from_slice(&displacement(target + 5, body + RELAY_OFFSET)?);
    Ok(Encoded { code, relay, patch })
}

fn policies() -> Result<(), HookError> {
    let mut dep = [0u32; 2];
    // SAFETY: current process and exactly sized initialized policy storage.
    if unsafe { GetProcessMitigationPolicy(GetCurrentProcess(), ProcessDEPPolicy, dep.as_mut_ptr().cast(), 8) } == 0 {
        return Err(failure("DEP policy"));
    }
    if dep[0] & 1 == 0 || dep[0] & !3 != 0 {
        return Err(HookError::UnsupportedPolicy);
    }
    // Initial production support deliberately admits only the actual player/game execution profile.
    for (kind, expected) in [(ProcessDynamicCodePolicy, 0), (ProcessControlFlowGuardPolicy, 0), (ProcessUserShadowStackPolicy, 0x100)] {
        let mut flags = 0u32;
        // SAFETY: current process and correctly sized policy buffer.
        if unsafe { GetProcessMitigationPolicy(GetCurrentProcess(), kind, (&raw mut flags).cast(), 4) } == 0 {
            return Err(failure("mitigation policy"));
        }
        if flags != expected {
            return Err(HookError::UnsupportedPolicy);
        }
    }
    Ok(())
}

unsafe fn pin(address: *const u8) -> Result<(), HookError> {
    let mut module = std::ptr::null_mut();
    // SAFETY: address identifies loaded original/callback code; FROM_ADDRESS does not read a string.
    if unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, address.cast(), &raw mut module) } == 0 {
        return Err(failure("pin module"));
    }
    Ok(())
}

struct Allocation {
    page: NonNull<u8>,
    registered: bool,
}
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: this guard is used only before any function pointer/entry patch is published.
        unsafe {
            if self.registered && !RtlDeleteFunctionTable(self.page.as_ptr().add(TABLE_OFFSET).cast()) {
                // Never free storage while Windows might still retain its unwind-table pointer.
                return;
            }
            VirtualFree(self.page.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

/// A prepared, process-retained original trampoline and nearby entry relay. No automatic detach/free.
/// Access remains on the owning control thread; this object contains no managed addresses.
pub struct SchedulerHook {
    target: NonNull<u8>,
    page: NonNull<u8>,
    original: [u8; 11],
    patch: [u8; 5],
    installed: bool,
}
impl SchedulerHook {
    /// Prepare without changing original entry bytes. All code/table storage is prepared before suspension.
    ///
    /// # Safety
    /// Target is the live, caller-identified static ExecuteTasks with the exact generated native ABI,
    /// versioned metadata and expected function length. It starts with the supplied authentic original
    /// bytes and has no incoming control-flow edges into its first 11 bytes. Callback has the matching
    /// C-unwind ABI and forwards the original argument/foreign exceptions correctly. Both belong to
    /// loaded modules. Caller exclusively owns target patching and externally coordinates shutdown,
    /// special APC/context restoration, remote thread creation and other nonstandard continuations.
    /// Other patching libraries/copies must share process-wide transaction coordination; only this
    /// library instance's own transactions are serialized automatically.
    /// This adapter cannot prove method identity/body permissions from bytes or enumerate external actors.
    pub unsafe fn prepare(target: NonNull<u8>, callback: ExecuteTasks, original: [u8; 11], function_length: u32) -> Result<Self, HookError> {
        policies()?;
        // A single protection result cannot preserve independently protected adjacent pages.
        if target.as_ptr() as usize & (PAGE - 1) > PAGE - 5 {
            return Err(HookError::EntryCrossesPage);
        }
        encode(target.as_ptr() as usize, target.as_ptr() as usize, callback as usize, original)?;
        // SAFETY: caller establishes valid module-owned code and original bytes.
        unsafe {
            pin(target.as_ptr())?;
            pin(callback as *const u8)?;
            if std::slice::from_raw_parts(target.as_ptr(), 11) != original {
                return Err(HookError::OwnershipLost);
            }
            let mut base = 0;
            let entry = RtlLookupFunctionEntry(target.as_ptr() as u64, &raw mut base, std::ptr::null_mut());
            if entry.is_null() || base + (*entry).BeginAddress as u64 != target.as_ptr() as u64 || (*entry).EndAddress - (*entry).BeginAddress != function_length {
                return Err(HookError::UnsupportedUnwind);
            }
            let info = std::slice::from_raw_parts((base + (*entry).Anonymous.UnwindData as u64) as *const u8, 6);
            if info != &UNWIND[..6] {
                return Err(HookError::UnsupportedUnwind);
            }
            let aligned = target.as_ptr() as usize & !0xffff;
            for index in 1..4096usize {
                for address in [aligned.checked_add(index * 0x10000), aligned.checked_sub(index * 0x10000)].into_iter().flatten() {
                    let Ok(Encoded { code, relay, patch }) = encode(target.as_ptr() as usize, address, callback as usize, original) else {
                        continue;
                    };
                    let Some(page) = NonNull::new(VirtualAlloc(address as *const c_void, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).cast::<u8>()) else {
                        continue;
                    };
                    let mut allocation = Allocation { page, registered: false };
                    std::ptr::copy_nonoverlapping(code.as_ptr(), page.as_ptr(), code.len());
                    std::ptr::copy_nonoverlapping(UNWIND.as_ptr(), page.as_ptr().add(32), 8);
                    std::ptr::copy_nonoverlapping(relay.as_ptr(), page.as_ptr().add(RELAY_OFFSET), relay.len());
                    let table = page.as_ptr().add(TABLE_OFFSET).cast::<IMAGE_RUNTIME_FUNCTION_ENTRY>();
                    table.write(IMAGE_RUNTIME_FUNCTION_ENTRY {
                        BeginAddress: 0,
                        EndAddress: 23,
                        Anonymous: std::mem::zeroed(),
                    });
                    (*table).Anonymous.UnwindData = 32;
                    if !RtlAddFunctionTable(table, 1, page.as_ptr() as u64) {
                        return Err(failure("register unwind table"));
                    }
                    allocation.registered = true;
                    let mut old = 0;
                    if VirtualProtect(page.as_ptr().cast(), PAGE, PAGE_EXECUTE_READ, &raw mut old) == 0
                        || FlushInstructionCache(GetCurrentProcess(), page.as_ptr().cast(), PAGE) == 0
                    {
                        return Err(failure("publish executable page"));
                    }
                    // Retain even an uninstalled prepared page: original() can expose its raw entry.
                    std::mem::forget(allocation);
                    return Ok(Self {
                        target,
                        page,
                        original,
                        patch,
                        installed: false,
                    });
                }
            }
        }
        Err(HookError::OutOfRange)
    }

    pub fn original(&self) -> ExecuteTasks {
        // SAFETY: prepare establishes exact ABI and retains this registered executable page forever.
        unsafe { std::mem::transmute(self.page.as_ptr()) }
    }

    pub fn trampoline_address(&self) -> *const u8 {
        self.page.as_ptr()
    }

    /// Publish the original pointer/queue before calling; callback may execute as threads resume.
    ///
    /// # Safety
    /// Uphold prepare's exclusive-target/control-flow contract. The current control thread must
    /// not be inside this target or a callback. No runtime calls, allocation or lock acquisition may
    /// be added to the suspended section. The Windows protection, context, cache and resume wrappers
    /// must not enter instrumentation that waits for a suspended thread or a lock it holds. Only
    /// the NT census stub is inspected; the other wrappers are not checked for hooks. Thread
    /// enumeration is a rejection check, not authorization
    /// for arbitrary remote injectors/special APCs. An error after patching may leave the hook installed.
    pub unsafe fn install(&mut self) -> Result<(), HookError> {
        if self.installed {
            return Err(HookError::AlreadyInstalled);
        }
        // SAFETY: inherited target ownership contract; this object owns only its own patch.
        unsafe { self.change(true) }
    }

    /// Restore only our still-owned patch; published code/tables/modules remain retained until exit.
    ///
    /// # Safety
    /// Same installation/control-flow obligations. Disable queue admission before restoring. This
    /// operation does not drain callbacks or authorize helper/trampoline unloading. An error after
    /// writing can still mean the entry was restored; inspect installed state/error conservatively.
    pub unsafe fn restore_entry(&mut self) -> Result<(), HookError> {
        if !self.installed {
            return Err(HookError::NotInstalled);
        }
        // SAFETY: inherited exclusive-target/lifetime contract.
        unsafe { self.change(false) }
    }

    pub fn is_installed(&self) -> bool {
        self.installed
    }

    unsafe fn change(&mut self, installing: bool) -> Result<(), HookError> {
        // Serialize all instances in this library before any thread can be suspended.
        let _transaction = TRANSACTION.lock().map_err(|_| HookError::TransactionPoisoned)?;
        policies()?;
        let mut expected = self.original;
        if !installing {
            expected[..5].copy_from_slice(&self.patch);
        }
        // ThreadSet allocates/opens handles before suspending. The suspended section calls Win32
        // context/protection/cache/resume wrappers, not verified direct syscalls. An overlay/EDR
        // hook in those wrappers can deadlock on a suspended thread; install's external contract
        // excludes that instrumentation. Checking the census stub does not establish this premise.
        let mut threads = ThreadSet::capture()?;
        let transaction = threads.suspend(self.target.as_ptr() as usize, 11)?;
        // SAFETY: caller keeps this executable entry valid; every enumerated other thread is stopped.
        let result = unsafe {
            if std::slice::from_raw_parts(self.target.as_ptr(), 11) != expected {
                Err(HookError::OwnershipLost)
            } else {
                let mut old = 0;
                if VirtualProtect(self.target.as_ptr().cast(), 5, PAGE_EXECUTE_READWRITE, &raw mut old) == 0 {
                    Err(failure("entry protection"))
                } else {
                    let bytes = if installing { self.patch.as_slice() } else { &self.original[..5] };
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.target.as_ptr(), 5);
                    self.installed = installing;
                    let flushed = FlushInstructionCache(GetCurrentProcess(), self.target.as_ptr().cast(), 5);
                    let mut ignored = 0;
                    let protected = VirtualProtect(self.target.as_ptr().cast(), 5, old, &raw mut ignored);
                    if flushed == 0 || protected == 0 {
                        Err(failure("entry cache/protection restoration"))
                    } else {
                        Ok(())
                    }
                }
            }
        };
        let resumed = transaction.resume();
        resumed?;
        result
    }
}
