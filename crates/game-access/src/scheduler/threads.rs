//! No Rust allocations in the suspended section. Other than the checked census stub, Windows
//! wrappers are called normally; their instrumentation/lock behavior is an external assumption.
//! NT census layout is the Windows x64 SDK SYSTEM_PROCESS/
//! SYSTEM_THREAD_INFORMATION layout, validated by bounded offsets and counts before reading IDs.
use super::{HookError, failure};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_CONTROL_AMD64, GetThreadContext};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetProcessIdOfThread, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME, THREAD_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::{CLIENT_ID, SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION};

type QuerySystem = unsafe extern "system" fn(u32, *mut std::ffi::c_void, u32, *mut u32) -> i32;
const LIMIT: usize = 4096;
const PROCESS_BYTES: usize = std::mem::size_of::<SYSTEM_PROCESS_INFORMATION>();
const THREAD_BYTES: usize = std::mem::size_of::<SYSTEM_THREAD_INFORMATION>();
const PID_OFFSET: usize = std::mem::offset_of!(SYSTEM_PROCESS_INFORMATION, UniqueProcessId);
const TID_OFFSET: usize = std::mem::offset_of!(SYSTEM_THREAD_INFORMATION, ClientId) + std::mem::offset_of!(CLIENT_ID, UniqueThread);
const _: () = assert!(PROCESS_BYTES == 0x100 && THREAD_BYTES == 0x50 && PID_OFFSET == 0x50 && TID_OFFSET == 0x30);

fn thread_failure(handle: HANDLE, operation: &'static str) -> HookError {
    // Preserve the failed operation's error before the wait can change last-error state.
    let error = failure(operation);
    // SAFETY: owned SYNCHRONIZE thread handle, zero-time kernel query. A signaled thread
    // object proves termination, unlike an access-denied code or STILL_ACTIVE exit value.
    if unsafe { WaitForSingleObject(handle, 0) } == 0 {
        HookError::ThreadsChanged
    } else {
        error
    }
}

#[repr(C, align(16))]
struct AlignedContext(CONTEXT);

struct Thread {
    id: u32,
    handle: HANDLE,
    suspended: bool,
}
impl Drop for Thread {
    fn drop(&mut self) {
        // SAFETY: these handles are owned and remain open until all owned suspensions are released.
        unsafe { CloseHandle(self.handle) };
    }
}

pub(super) struct ThreadSet {
    threads: Vec<Thread>,
    system: Vec<u64>,
    ids: Vec<u32>,
    query: QuerySystem,
    process: u32,
    current: u32,
}

impl ThreadSet {
    pub(super) fn capture() -> Result<Self, HookError> {
        // SAFETY: fixed terminated module/export names; ntdll is retained by Windows until exit.
        let query: QuerySystem = unsafe {
            let module = GetModuleHandleW([110u16, 116, 100, 108, 108, 46, 100, 108, 108, 0].as_ptr());
            let function = GetProcAddress(module, c"NtQuerySystemInformation".as_ptr().cast()).ok_or_else(|| failure("system census export"))?;
            // Reject an instrumented/non-native census wrapper: this does not inspect the
            // protection/context/suspension wrappers used elsewhere. The census must not enter user heap
            // or callback machinery. Only the evaluated x64 native syscall stub is supported.
            let stub = std::slice::from_raw_parts(function as *const u8, 24);
            if stub[..4] != [0x4c, 0x8b, 0xd1, 0xb8] || stub[8..] != [0xf6, 4, 0x25, 8, 3, 0xfe, 0x7f, 1, 0x75, 3, 0x0f, 5, 0xc3, 0xcd, 0x2e, 0xc3] {
                return Err(HookError::ThreadCensus);
            }
            std::mem::transmute(function)
        };
        let mut set = Self {
            threads: Vec::with_capacity(LIMIT),
            system: vec![0; 16 * 1024 * 1024 / 8],
            ids: Vec::with_capacity(LIMIT),
            query,
            // SAFETY: local identity queries take no arguments.
            process: unsafe { GetCurrentProcessId() },
            // SAFETY: argument-free current Windows thread identity query.
            current: unsafe { GetCurrentThreadId() },
        };
        set.census()?;
        for &id in &set.ids {
            if id == set.current {
                continue;
            }
            // SAFETY: census supplied this process's TID. Failure rejects; no inaccessible thread is skipped.
            let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE, 0, id) };
            if handle.is_null() {
                let error = failure("open thread");
                // No suspension or patch has happened. A fresh native census may prove the
                // selected TID disappeared; permission failures on a surviving TID stay fatal.
                set.census()?;
                return Err(if set.ids.contains(&id) { error } else { HookError::ThreadsChanged });
            }
            let thread = Thread { id, handle, suspended: false };
            // SAFETY: owned query-capable handle. A census TID can be recycled before OpenThread;
            // reject a handle from another process before suspending anything.
            if unsafe { GetProcessIdOfThread(thread.handle) } != set.process {
                return Err(HookError::ThreadsChanged);
            }
            set.threads.push(thread);
        }
        Ok(set)
    }

    fn census(&mut self) -> Result<(), HookError> {
        let mut returned = 0u32;
        // SAFETY: preallocated writable buffer; information class 5 is SystemProcessInformation.
        let status = unsafe { (self.query)(5, self.system.as_mut_ptr().cast(), (self.system.len() * 8) as u32, &raw mut returned) };
        if status != 0 {
            return Err(HookError::NativeQuery(status));
        }
        // SAFETY: initialized u64 storage is 8-byte aligned for the native API; every byte pattern
        // is valid. The native call has completed and this immutable view lasts only through parsing.
        let buffer = unsafe { std::slice::from_raw_parts(self.system.as_ptr().cast::<u8>(), self.system.len() * 8) };
        let bytes = buffer.get(..returned as usize).ok_or(HookError::ThreadCensus)?;
        let mut offset = 0usize;
        self.ids.clear();
        loop {
            let prefix = bytes.get(offset..offset + PROCESS_BYTES).ok_or(HookError::ThreadCensus)?;
            let next = u32::from_le_bytes(prefix[..4].try_into().map_err(|_| HookError::ThreadCensus)?) as usize;
            let count = u32::from_le_bytes(prefix[4..8].try_into().map_err(|_| HookError::ThreadCensus)?) as usize;
            let pid = u64::from_le_bytes(prefix[PID_OFFSET..PID_OFFSET + 8].try_into().map_err(|_| HookError::ThreadCensus)?);
            if pid == self.process as u64 {
                if count > LIMIT {
                    return Err(HookError::ThreadLimit);
                }
                let end = offset.checked_add(PROCESS_BYTES + count * THREAD_BYTES).ok_or(HookError::ThreadCensus)?;
                if end > bytes.len() || (next != 0 && end > offset + next) {
                    return Err(HookError::ThreadCensus);
                }
                for index in 0..count {
                    let start = offset + PROCESS_BYTES + index * THREAD_BYTES + TID_OFFSET;
                    let id = u64::from_le_bytes(bytes[start..start + 8].try_into().map_err(|_| HookError::ThreadCensus)?);
                    let id = u32::try_from(id).map_err(|_| HookError::ThreadCensus)?;
                    if id == 0 || self.ids.contains(&id) {
                        return Err(HookError::ThreadCensus);
                    }
                    // Capacity was allocated before suspension and count is bounded by LIMIT.
                    self.ids.push(id);
                }
                return if self.ids.contains(&self.current) { Ok(()) } else { Err(HookError::ThreadCensus) };
            }
            if next < PROCESS_BYTES || next & 7 != 0 {
                return Err(HookError::ThreadCensus);
            }
            offset = offset.checked_add(next).ok_or(HookError::ThreadCensus)?;
        }
    }

    pub(super) fn suspend(&mut self, entry: usize, length: usize) -> Result<Suspended<'_>, HookError> {
        let suspended = Suspended { set: self };
        let validation = (|| {
            for thread in &mut suspended.set.threads {
                // SAFETY: owned handle with SUSPEND_RESUME rights, never the current control thread.
                if unsafe { SuspendThread(thread.handle) } == u32::MAX {
                    return Err(thread_failure(thread.handle, "suspend thread"));
                }
                thread.suspended = true;
                // SAFETY: correctly initialized, explicitly 16-byte-aligned context on our stack.
                let mut context = AlignedContext(unsafe { std::mem::zeroed() });
                context.0.ContextFlags = CONTEXT_CONTROL_AMD64;
                // SAFETY: valid suspended thread and correctly aligned/sized context storage.
                if unsafe { GetThreadContext(thread.handle, &raw mut context.0) } == 0 {
                    return Err(thread_failure(thread.handle, "get thread context"));
                }
                if (entry..entry + length).contains(&(context.0.Rip as usize)) {
                    return Err(HookError::ThreadInEntry);
                }
            }
            suspended.set.census()?;
            if suspended.set.ids.len() != suspended.set.threads.len() + 1 || suspended.set.threads.iter().any(|thread| !suspended.set.ids.contains(&thread.id)) {
                return Err(HookError::ThreadsChanged);
            }
            Ok(())
        })();
        if let Err(error) = validation {
            // A failed restoration matters more than a rejected patch. Drop remains a retry guard,
            // but must not be the only place validation failures attempt to release suspensions.
            suspended.resume()?;
            return Err(error);
        }
        Ok(suspended)
    }
}

pub(super) struct Suspended<'set> {
    set: &'set mut ThreadSet,
}
impl Suspended<'_> {
    fn release(&mut self) -> Result<(), HookError> {
        let mut result = Ok(());
        for thread in self.set.threads.iter_mut().rev() {
            if thread.suspended {
                // SAFETY: decrement exactly the one suspension acquired by this transaction.
                if unsafe { ResumeThread(thread.handle) } == u32::MAX {
                    let error = thread_failure(thread.handle, "resume thread");
                    if error == HookError::ThreadsChanged {
                        // There is no surviving execution/suspension to release on this proven
                        // terminated kernel object. Never apply this exception to a live handle.
                        thread.suspended = false;
                    } else {
                        result = Err(error);
                    }
                } else {
                    thread.suspended = false;
                }
            }
        }
        result
    }
    pub(super) fn resume(mut self) -> Result<(), HookError> {
        self.release()
    }
}
impl Drop for Suspended<'_> {
    fn drop(&mut self) {
        // Also runs on validation rejection/unwind; handle/Vec destruction happens after this guard.
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::SetLastError;

    #[test]
    fn only_kernel_proven_thread_exit_turns_an_access_failure_into_changed_census() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let child = std::thread::spawn(move || {
            // SAFETY: query the owned child's actual native TID.
            started_tx.send(unsafe { GetCurrentThreadId() }).unwrap();
            exit_rx.recv().unwrap();
        });
        let tid = started_rx.recv().unwrap();
        // SAFETY: owned live child TID with the actual transaction's rights.
        let handle = unsafe { OpenThread(THREAD_SYNCHRONIZE | THREAD_QUERY_LIMITED_INFORMATION | THREAD_SUSPEND_RESUME, 0, tid) };
        assert!(!handle.is_null());
        let owned = Thread {
            id: tid,
            handle,
            suspended: false,
        };
        // SAFETY: set only this test thread's last error; the native handle remains live.
        unsafe { SetLastError(5) };
        assert_eq!(
            thread_failure(owned.handle, "authored access failure"),
            HookError::Win32 {
                operation: "authored access failure",
                code: 5
            }
        );
        exit_tx.send(()).unwrap();
        child.join().unwrap();
        // SAFETY: the same retained handle now denotes the proven terminated child.
        assert_eq!(unsafe { SuspendThread(owned.handle) }, u32::MAX);
        assert_eq!(thread_failure(owned.handle, "suspend ended child"), HookError::ThreadsChanged);
    }
}
