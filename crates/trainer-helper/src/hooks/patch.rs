//! Owned interior code ranges. Resolve every site before publication; transactions compare actual bytes, suspend
//! peer threads, write/read back and recover the pre-transaction bytes on failure. No allocation while suspended.
//! A thread stopped inside a changing range refuses the transaction rather than being relocated into different code.
//!
//! Limits, part of the unsafe contract: the Toolhelp snapshot is not a full census (a thread created after it, or a
//! return address inside a changed range, is not seen), and the suspended section still calls Win32 context,
//! protection, cache and resume wrappers, which overlays or security software can instrument and which can wait on a
//! suspended thread's lock. Deadlock freedom is therefore not established.
use super::{HookState, TRANSACTION};
use std::{
    ptr::NonNull,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering::SeqCst},
    },
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE},
    System::{
        Diagnostics::{
            Debug::{CONTEXT, CONTEXT_CONTROL_AMD64, FlushInstructionCache, GetThreadContext},
            ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next},
        },
        Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect},
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetExitCodeThread, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
            THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};

#[derive(Debug)]
pub enum PatchError {
    InvalidPattern,
    Missing,
    Ambiguous,
    Overlap,
    OwnershipLost,
    ThreadInPatch,
    Win32 { operation: &'static str, code: u32 },
    Transaction { error: Box<PatchError>, recovery: Vec<PatchError> },
}
impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPattern => f.write_str("Invalid code-patch recipe"),
            Self::Missing => f.write_str("Code-patch pattern missing"),
            Self::Ambiguous => f.write_str("Code-patch pattern is ambiguous"),
            Self::Overlap => f.write_str("Code-patch ranges overlap"),
            Self::OwnershipLost => f.write_str("Code-patch bytes or protection no longer agree"),
            Self::ThreadInPatch => f.write_str("A thread is executing a changing code range; nothing was written"),
            Self::Win32 { operation, code } => write!(f, "{operation}: Windows error {code}"),
            Self::Transaction { error, recovery } => write!(f, "Code-patch transaction: {error}; recovery failures: {recovery:?}"),
        }
    }
}
impl std::error::Error for PatchError {}
fn win32(operation: &'static str) -> PatchError {
    // SAFETY: retrieves this thread's last error without formatting or allocation.
    let code = unsafe { GetLastError() };
    PatchError::Win32 { operation, code }
}

/// A bounded unique match, returning the beginning of the bytes to change. -1 is the only wildcard.
pub fn find_unique(code: &[u8], pattern: &[i16], at: usize, replacement: &[u8]) -> Result<usize, PatchError> {
    if pattern.is_empty()
        || pattern.iter().any(|b| !(-1..=255).contains(b))
        || replacement.is_empty()
        || replacement.len() > 64
        || at.checked_add(replacement.len()).is_none_or(|end| end > pattern.len())
    {
        return Err(PatchError::InvalidPattern);
    }
    let mut found = None;
    for (offset, bytes) in code.windows(pattern.len()).enumerate() {
        if pattern.iter().zip(bytes).all(|(expected, actual)| *expected == -1 || *expected == i16::from(*actual)) {
            if found.is_some() {
                return Err(PatchError::Ambiguous);
            }
            found = Some(offset + at);
        }
    }
    found.ok_or(PatchError::Missing)
}

static OWNED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
pub struct CodePatch {
    address: NonNull<u8>,
    original: Vec<u8>,
    replacement: Vec<u8>,
    pending_protection: AtomicU32,
    cache_ok: AtomicBool,
}
impl CodePatch {
    /// # Safety
    /// A retained readable executable span, exclusively owned until successful restoration. No return address may
    /// enter the interior of a changed range. The caller retains this owner after every attempted transaction.
    pub unsafe fn prepare(address: NonNull<u8>, expected: &[u8], replacement: &[u8]) -> Result<Self, PatchError> {
        if expected.is_empty() || expected.len() > 64 || expected.len() != replacement.len() || expected == replacement {
            return Err(PatchError::InvalidPattern);
        }
        let _lock = TRANSACTION.lock().map_err(|_| PatchError::OwnershipLost)?;
        // SAFETY: caller retains the complete readable code span.
        if unsafe { std::slice::from_raw_parts(address.as_ptr(), expected.len()) } != expected {
            return Err(PatchError::OwnershipLost);
        }
        let start = address.as_ptr() as usize;
        let end = start.checked_add(expected.len()).ok_or(PatchError::InvalidPattern)?;
        let mut owned = OWNED.lock().unwrap_or_else(|p| p.into_inner());
        if owned.iter().any(|(lo, hi)| start < *hi && *lo < end) {
            return Err(PatchError::Overlap);
        }
        let owner = Self {
            address,
            original: expected.to_vec(),
            replacement: replacement.to_vec(),
            pending_protection: AtomicU32::new(0),
            cache_ok: AtomicBool::new(true),
        };
        owned.push((start, end));
        Ok(owner)
    }
    pub fn range(&self) -> std::ops::Range<usize> {
        let start = self.address.as_ptr() as usize;
        start..start + self.original.len()
    }
    pub fn state(&self) -> HookState {
        let Ok(_lock) = TRANSACTION.lock() else { return HookState::Unknown };
        self.state_unlocked()
    }
    fn state_unlocked(&self) -> HookState {
        if self.pending_protection.load(SeqCst) != 0 || !self.cache_ok.load(SeqCst) {
            return HookState::Unknown;
        }
        // SAFETY: the owner retains readable process-owned code and excludes competing writers.
        let actual = unsafe { std::slice::from_raw_parts(self.address.as_ptr(), self.original.len()) };
        if actual == self.original {
            HookState::Disabled
        } else if actual == self.replacement {
            HookState::Enabled
        } else {
            HookState::Unknown
        }
    }
    unsafe fn write(&self, on: bool) -> Result<(), PatchError> {
        let bytes = if on { &self.replacement } else { &self.original };
        // SAFETY: serialized transaction; all admitted peer threads are suspended before any write.
        unsafe {
            let mut previous = 0;
            if VirtualProtect(self.address.as_ptr().cast(), bytes.len(), PAGE_EXECUTE_READWRITE, &raw mut previous) == 0 {
                return Err(win32("code patch protection"));
            }
            let restore = match self.pending_protection.compare_exchange(0, previous, SeqCst, SeqCst) {
                Ok(_) => previous,
                Err(pending) => pending,
            };
            self.cache_ok.store(false, SeqCst);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.address.as_ptr(), bytes.len());
            let flushed = FlushInstructionCache(GetCurrentProcess(), self.address.as_ptr().cast(), bytes.len());
            let flush_error = if flushed == 0 {
                Some(win32("code patch cache"))
            } else {
                self.cache_ok.store(true, SeqCst);
                None
            };
            let mut ignored = 0;
            let restored = VirtualProtect(self.address.as_ptr().cast(), bytes.len(), restore, &raw mut ignored);
            if restored != 0 {
                self.pending_protection.store(0, SeqCst);
            }
            if let Some(error) = flush_error {
                return Err(error);
            }
            if restored == 0 {
                return Err(win32("restore code patch protection"));
            }
            if self.state_unlocked() != if on { HookState::Enabled } else { HookState::Disabled } {
                return Err(PatchError::OwnershipLost);
            }
            Ok(())
        }
    }
}
impl Drop for CodePatch {
    fn drop(&mut self) {
        // Dropping never writes code. The lifecycle releases owners only before publication or after proven restore.
        let range = self.range();
        OWNED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(start, end)| *start != range.start || *end != range.end);
    }
}

#[repr(C, align(16))]
struct AlignedContext(CONTEXT);
struct Thread {
    handle: HANDLE,
    suspended: bool,
    context: AlignedContext,
}
impl Drop for Thread {
    fn drop(&mut self) {
        // SAFETY: owned handle, peers have already been resumed by Paused::drop.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
struct Paused {
    threads: Vec<Thread>,
}
impl Paused {
    fn prepare() -> Result<Self, PatchError> {
        // Allocate all records and acquire handles before suspending anyone. Snapshot races with newly created
        // threads remain an explicit supported-target limitation, as with the detour backend's best-effort census.
        // SAFETY: Toolhelp uses an initialized record; successful same-process handles are retained and closed once.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(win32("enumerate patch threads"));
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut guard = Self { threads: Vec::new() };
            let mut result = Ok(());
            let mut next = Thread32First(snapshot, &raw mut entry);
            while next != 0 {
                if entry.th32OwnerProcessID == GetCurrentProcessId() && entry.th32ThreadID != GetCurrentThreadId() {
                    let handle = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION, 0, entry.th32ThreadID);
                    if handle.is_null() {
                        if GetLastError() != ERROR_INVALID_PARAMETER {
                            result = Err(win32("open patch thread"));
                            break;
                        }
                    } else {
                        let mut context: CONTEXT = std::mem::zeroed();
                        context.ContextFlags = CONTEXT_CONTROL_AMD64;
                        guard.threads.push(Thread {
                            handle,
                            suspended: false,
                            context: AlignedContext(context),
                        });
                    }
                }
                next = Thread32Next(snapshot, &raw mut entry);
            }
            if result.is_ok() && next == 0 && GetLastError() != ERROR_NO_MORE_FILES {
                result = Err(win32("enumerate patch threads"));
            }
            CloseHandle(snapshot);
            result.map(|()| guard)
        }
    }
    fn suspend(&mut self, ranges: &[std::ops::Range<usize>]) -> Result<(), PatchError> {
        // SAFETY: owned current-process handles, current thread excluded; no heap allocation in this span.
        unsafe {
            for thread in &mut self.threads {
                if SuspendThread(thread.handle) == u32::MAX {
                    let mut code = STILL_ACTIVE as u32;
                    if GetExitCodeThread(thread.handle, &raw mut code) != 0 && code != STILL_ACTIVE as u32 {
                        continue;
                    }
                    return Err(win32("suspend patch thread"));
                }
                thread.suspended = true;
                if GetThreadContext(thread.handle, &raw mut thread.context.0) == 0 {
                    return Err(win32("read patch thread context"));
                }
                if ranges.iter().any(|range| range.contains(&(thread.context.0.Rip as usize))) {
                    return Err(PatchError::ThreadInPatch);
                }
            }
        }
        Ok(())
    }
}
impl Drop for Paused {
    fn drop(&mut self) {
        // SAFETY: match exactly our successful suspend increments before any vector/handle storage is released.
        for thread in self.threads.iter_mut().rev() {
            if thread.suspended {
                // SAFETY: exactly the suspend increment taken on this owned same-process handle.
                unsafe {
                    ResumeThread(thread.handle);
                }
                thread.suspended = false;
            }
        }
    }
}

/// # Safety
/// Every range's ownership/code contract holds. The caller is outside all changing ranges. Snapshot-excluded/new
/// threads must not execute them during the transaction. No outstanding return address enters a range's interior.
pub unsafe fn commit(changes: &[(&CodePatch, bool)]) -> Result<(), PatchError> {
    let _lock = TRANSACTION.lock().map_err(|_| PatchError::OwnershipLost)?;
    let mut work = Vec::new();
    for (index, (patch, on)) in changes.iter().enumerate() {
        let before = patch.state_unlocked();
        if before == HookState::Unknown {
            return Err(PatchError::OwnershipLost);
        }
        if changes[..index].iter().any(|(previous, _)| {
            let a = patch.range();
            let b = previous.range();
            a.start < b.end && b.start < a.end
        }) {
            return Err(PatchError::Overlap);
        }
        if (before == HookState::Enabled) != *on {
            work.push((*patch, *on, before == HookState::Enabled));
        }
    }
    if work.is_empty() {
        return Ok(());
    }
    let ranges: Vec<_> = work.iter().map(|(patch, _, _)| patch.range()).collect();
    let mut recovery = Vec::with_capacity(work.len());
    let mut paused = Paused::prepare()?;
    paused.suspend(&ranges)?;
    let mut failure = None;
    for (patch, _, before) in &work {
        if patch.state_unlocked() != if *before { HookState::Enabled } else { HookState::Disabled } {
            failure = Some(PatchError::OwnershipLost);
            break;
        }
    }
    if failure.is_none() {
        for (index, (patch, on, _)) in work.iter().enumerate() {
            // SAFETY: caller contract plus suspended peers; preparation/readback allocated nothing in this span.
            if let Err(error) = unsafe { test_write(index).and_then(|()| patch.write(*on)) } {
                failure = Some(error);
                for (patch, _, before) in work[..=index].iter().rev() {
                    // SAFETY: peers remain suspended and all pre-transaction bytes/owners are still retained.
                    if let Err(error) = unsafe { patch.write(*before) } {
                        recovery.push(error);
                    }
                }
                break;
            }
        }
    }
    drop(paused);
    match failure {
        Some(error) => Err(PatchError::Transaction { error: Box::new(error), recovery }),
        None => Ok(()),
    }
}
#[cfg(not(test))]
fn test_write(_: usize) -> Result<(), PatchError> {
    Ok(())
}
#[cfg(test)]
thread_local! {static FAIL_WRITE:std::cell::Cell<Option<usize>>=const{std::cell::Cell::new(None)};}
#[cfg(test)]
pub(crate) fn with_write_failure<T>(index: usize, operation: impl FnOnce() -> T) -> T {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_WRITE.set(self.0);
        }
    }
    let _reset = Reset(FAIL_WRITE.replace(Some(index)));
    operation()
}
#[cfg(test)]
fn test_write(index: usize) -> Result<(), PatchError> {
    if FAIL_WRITE.get() == Some(index) { Err(PatchError::OwnershipLost) } else { Ok(()) }
}

#[cfg(test)]
mod tests;
