//! A thread started in another process, and the storage it may still be using.

use super::{ProcessLease, ProcessTarget, RemoteError, Result, invalid, last, owned};
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::Arc,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::WAIT_TIMEOUT,
    System::{
        Diagnostics::Debug::WriteProcessMemory,
        Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx},
        Threading::{CreateRemoteThread, GetExitCodeThread, WaitForSingleObject},
    },
};

/// An uncertain remote thread owns its input allocation, its process lease and what its caller retained for it.
/// Dropping before proving thread exit deliberately retains those resources for process lifetime: the thread and process
/// handles and `retained` are never closed or dropped, and the input is never freed, while the process runs. Once the
/// process has ended there is nothing left to protect: everything local closes, and its memory went with it.
///
/// `C` is what the caller needs to read a reply out of the input storage: a call with one is read back once it has
/// finished. `K` is what stays alive as long as the thread might run, such as a lease on the module it runs in.
pub struct PendingCall<C, K> {
    process: Arc<ProcessLease>,
    thread: Option<OwnedHandle>,
    allocation: u64,
    length: usize,
    config: Option<C>,
    retained: Option<K>,
    complete: bool,
    consumed: bool,
}
/// A remote call that finished: the thread's exit code, and the reply if the call had something to read one with.
pub struct CallCompletion<T> {
    pub exit_code: u32,
    pub reply: Option<T>,
}
impl<C, K> PendingCall<C, K> {
    pub fn target(&self) -> &ProcessTarget {
        self.process.target()
    }
    pub fn process_exited(&self) -> bool {
        self.process.has_exited()
    }
    /// Whether the thread is proved to have ended, so its input can be released.
    pub fn exit_proven(&self) -> bool {
        self.complete
    }
    fn wait_status(&self, milliseconds: u32) -> Result<u32> {
        #[cfg(test)]
        if let Some(fake) = &self.process.fake {
            return Ok(fake.wait.load(std::sync::atomic::Ordering::SeqCst));
        }
        let thread = self.thread.as_ref().ok_or(RemoteError::Invalid("remote thread handle absent"))?;
        // SAFETY: query this operation's retained thread; never create a thread during polling.
        Ok(unsafe { WaitForSingleObject(thread.as_raw_handle(), milliseconds) })
    }
    fn exit_code(&self) -> Result<u32> {
        #[cfg(test)]
        if self.process.fake.is_some() {
            return Ok(0);
        }
        let thread = self.thread.as_ref().ok_or(RemoteError::Invalid("remote thread handle absent"))?;
        let mut code = 0;
        // SAFETY: this retained thread was already proved terminated by its wait handle.
        if unsafe { GetExitCodeThread(thread.as_raw_handle(), &mut code) } == 0 {
            return Err(last());
        }
        Ok(code)
    }
    fn finish<T, E: From<RemoteError>>(
        &mut self,
        admit: &mut dyn FnMut() -> std::result::Result<(), E>,
        decode: impl FnOnce(&C, u32, &[u8]) -> std::result::Result<T, E>,
    ) -> std::result::Result<Option<CallCompletion<T>>, E> {
        if self.consumed {
            return Err(RemoteError::Invalid("remote operation completion already consumed").into());
        }
        // A signaled thread is the ownership proof required before freeing input storage.
        self.complete = true;
        let exit_code = self.exit_code()?;
        let reply = if let Some(config) = &self.config {
            // Termination proves input can be released even if admission is now lost. Keep the
            // unconsumed result available for an explicit later admitted read, never a replay.
            admit()?;
            let bytes = self.process.read(self.allocation, self.length)?;
            self.consumed = true;
            Some(decode(config, exit_code, &bytes)?)
        } else {
            None
        };
        self.consumed = true;
        Ok(Some(CallCompletion { exit_code, reply }))
    }
    /// Waits up to `timeout` for the thread. `None` means it has not finished, and may be waited on again; the call is
    /// never made a second time. Once it has finished, a call with a reply reads it, after `admit` allows that, and passes
    /// its input storage to `decode` as it stands; admission lost at that point leaves the reply for a later wait.
    pub fn wait<T, E: From<RemoteError>>(
        &mut self,
        timeout: Duration,
        admit: &mut dyn FnMut() -> std::result::Result<(), E>,
        decode: impl FnOnce(&C, u32, &[u8]) -> std::result::Result<T, E>,
    ) -> std::result::Result<Option<CallCompletion<T>>, E> {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1).min(u32::MAX - 1);
        let wait = self.wait_status(millis)?;
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait != 0 {
            return Err(last().into());
        }
        self.finish(admit, decode)
    }
}
impl<C, K> Drop for PendingCall<C, K> {
    fn drop(&mut self) {
        if self.process.has_exited() {
            return;
        }
        if !self.complete {
            if let Some(thread) = self.thread.take() {
                std::mem::forget(thread)
            }
            if let Some(retained) = self.retained.take() {
                std::mem::forget(retained)
            }
            std::mem::forget(Arc::clone(&self.process));
            return;
        }
        // SAFETY: this one allocation is freed only after its own thread is proven terminated.
        self.process.release(self.allocation);
    }
}

/// Starts a thread in the process at `address`, on a copy of `input` (up to 64 KiB) in a private read/write allocation
/// there. A failure before the thread starts leaves no thread and frees the allocation.
///
/// # Safety
/// `address` is an `unsafe extern "system" fn(*mut c_void) -> u32` in that process, safe to call on a thread of its own
/// with a pointer to `input`, and that process is not this one.
pub unsafe fn call<C, K>(process: Arc<ProcessLease>, address: u64, input: &[u8], config: Option<C>, retained: K) -> Result<PendingCall<C, K>> {
    if input.is_empty() || input.len() > 65536 || address == 0 {
        return invalid("remote call span");
    }
    process.verify()?;
    // SAFETY: one private nonexecutable read/write allocation holds only input/configuration bytes.
    let allocation = unsafe { VirtualAllocEx(process.handle.as_raw_handle(), std::ptr::null(), input.len(), MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    if allocation.is_null() {
        return Err(last());
    }
    let mut pending = PendingCall {
        process,
        thread: None,
        allocation: allocation as usize as u64,
        length: input.len(),
        config,
        retained: Some(retained),
        complete: true,
        consumed: false,
    };
    let mut written = 0;
    // SAFETY: PendingCall owns process/allocation; the initialized input bytes live through this bounded synchronous copy.
    if unsafe { WriteProcessMemory(pending.process.handle.as_raw_handle(), allocation, input.as_ptr().cast(), input.len(), &mut written) } == 0 || written != input.len() {
        return Err(last());
    }
    // SAFETY: the caller establishes this exact remote entry ABI.
    // The input remains owned even if the thread starts before CreateRemoteThread returns.
    let entry: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(address as usize) };
    // SAFETY: validated remote entry and ABI; PendingCall retains process, allocation and input through remote completion.
    let thread = owned(unsafe {
        CreateRemoteThread(
            pending.process.handle.as_raw_handle(),
            std::ptr::null(),
            0,
            Some(entry),
            allocation,
            0,
            std::ptr::null_mut(),
        )
    })?;
    pending.thread = Some(thread);
    pending.complete = false;
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::super::lease::FakeIo;
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::SeqCst};
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::CreateEventW};

    const REPLY_SIZE: usize = 16;

    type TestCall = PendingCall<(), Arc<()>>;

    #[derive(Debug)]
    enum Failure {
        Remote,
        Rejected,
        Revoked,
    }
    impl From<RemoteError> for Failure {
        fn from(_: RemoteError) -> Self {
            Self::Remote
        }
    }
    fn event() -> OwnedHandle {
        // SAFETY: an owned local event is only a handle placeholder; no process/thread is opened.
        owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) }).unwrap()
    }
    fn pending(wait: u32, status: i32) -> (TestCall, Arc<FakeIo>) {
        let mut response = vec![0; REPLY_SIZE];
        response[12..16].copy_from_slice(&status.to_le_bytes());
        let fake = Arc::new(FakeIo {
            process_exited: AtomicBool::new(false),
            wait: AtomicU32::new(wait),
            freed: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            response,
        });
        let process = Arc::new(ProcessLease::fake(
            event(),
            ProcessTarget {
                pid: 11,
                created: 33,
                executable: "C:/fake.exe".into(),
            },
            Arc::clone(&fake),
        ));
        (
            PendingCall {
                process,
                thread: Some(event()),
                allocation: 0x1000,
                length: REPLY_SIZE,
                config: Some(()),
                retained: None,
                complete: false,
                consumed: false,
            },
            fake,
        )
    }
    fn decode(_: &(), _: u32, bytes: &[u8]) -> std::result::Result<i32, Failure> {
        let status = i32::from_le_bytes(bytes[12..16].try_into().unwrap());
        if status == 0 { Ok(status) } else { Err(Failure::Rejected) }
    }
    fn allow() -> std::result::Result<(), Failure> {
        Ok(())
    }
    /// Drops an unfinished call, then reclaims the handle and the lease clone production Drop retains on purpose, so the
    /// test itself leaks nothing. Closing the thread handle succeeds only if Drop really left it open.
    fn drop_uncertain(call: TestCall) {
        let raw_thread = call.thread.as_ref().unwrap().as_raw_handle();
        let process = Arc::as_ptr(&call.process);
        drop(call);
        // SAFETY: this authored backend never created any remote thread or allocation; these are the handle and Arc clone production Drop forgot.
        unsafe {
            assert_ne!(CloseHandle(raw_thread), 0, "an unfinished call keeps its thread handle open");
            Arc::decrement_strong_count(process);
        }
    }

    #[test]
    fn exited_process_releases_pending_owners_without_reading_or_freeing_remote_storage() {
        let (call, fake) = pending(WAIT_TIMEOUT, 0);
        let process = call.process.clone();
        fake.process_exited.store(true, SeqCst);
        drop(call);
        assert_eq!(Arc::strong_count(&process), 1);
        assert_eq!(fake.reads.load(SeqCst), 0);
        assert_eq!(fake.freed.load(SeqCst), 0);
    }
    #[test]
    fn timeout_and_failed_wait_retain_input_without_remote_reads_or_free() {
        for wait in [WAIT_TIMEOUT, u32::MAX] {
            let (mut pending, fake) = pending(wait, 0);
            let result = pending.wait(Duration::ZERO, &mut allow, decode);
            if wait == WAIT_TIMEOUT {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
            assert_eq!(fake.reads.load(SeqCst), 0);
            drop_uncertain(pending);
            assert_eq!(fake.freed.load(SeqCst), 0);
        }
    }
    #[test]
    fn an_unfinished_call_never_lets_go_of_what_it_retains() {
        let (mut pending, fake) = pending(WAIT_TIMEOUT, 0);
        let retained = Arc::new(());
        pending.retained = Some(Arc::clone(&retained));
        let process = Arc::clone(&pending.process);
        drop_uncertain(pending);
        assert_eq!(Arc::strong_count(&retained), 2, "what the call retained is forgotten, not dropped");
        assert_eq!(Arc::strong_count(&process), 1, "beside the clone drop_uncertain reclaimed");
        assert_eq!(fake.freed.load(SeqCst), 0);
        // SAFETY: reclaims the one clone Drop forgot, so the test leaks nothing.
        unsafe { Arc::decrement_strong_count(Arc::as_ptr(&retained)) };
    }
    #[test]
    fn proven_exit_allows_exactly_one_read_and_one_free_even_for_rejected_reply() {
        for status in [0, 1] {
            let (mut pending, fake) = pending(0, status);
            let result = pending.wait(Duration::ZERO, &mut allow, decode);
            if status == 0 {
                assert_eq!(result.unwrap().unwrap().reply, Some(0));
            } else {
                assert!(matches!(result, Err(Failure::Rejected)));
            }
            assert!(matches!(pending.wait(Duration::ZERO, &mut allow, decode), Err(Failure::Remote)));
            assert_eq!(fake.reads.load(SeqCst), 1);
            assert_eq!(fake.freed.load(SeqCst), 0);
            drop(pending);
            assert_eq!(fake.freed.load(SeqCst), 1);
        }
    }
    #[test]
    fn timeout_then_explicit_poll_can_finish_without_replaying_a_target_call() {
        let (mut pending, fake) = pending(WAIT_TIMEOUT, 0);
        assert!(pending.wait(Duration::from_millis(1), &mut allow, decode).unwrap().is_none());
        fake.wait.store(0, SeqCst);
        assert!(pending.wait(Duration::ZERO, &mut allow, decode).unwrap().is_some());
        drop(pending);
        assert_eq!(fake.reads.load(SeqCst), 1);
        assert_eq!(fake.freed.load(SeqCst), 1);
    }
    #[test]
    fn lost_admission_after_exit_prevents_reply_reads_and_allows_only_explicit_later_poll() {
        let (mut pending, fake) = pending(0, 0);
        assert!(matches!(pending.wait(Duration::ZERO, &mut || Err(Failure::Revoked), decode), Err(Failure::Revoked)));
        assert!(pending.exit_proven());
        assert_eq!(fake.reads.load(SeqCst), 0);
        assert_eq!(fake.freed.load(SeqCst), 0);
        assert!(pending.wait(Duration::ZERO, &mut allow, decode).unwrap().is_some());
        assert_eq!(fake.reads.load(SeqCst), 1);
        drop(pending);
        assert_eq!(fake.freed.load(SeqCst), 1);
    }
    #[test]
    fn unpublished_configuration_is_released_without_an_exit_wait() {
        let (mut pending, fake) = pending(u32::MAX, 0);
        pending.thread = None;
        pending.complete = true;
        drop(pending);
        assert_eq!(fake.reads.load(SeqCst), 0);
        assert_eq!(fake.freed.load(SeqCst), 1);
    }
}
