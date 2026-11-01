//! A process held by a handle, and checked to be the same process before each thing asked of it.

use super::{ModuleIdentity, ProcessTarget, Result, invalid, last, owned, process_identity};
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::DuplicateHandle,
    System::{
        Diagnostics::Debug::ReadProcessMemory,
        Memory::{MEM_RELEASE, VirtualFreeEx},
        Threading::{
            GetCurrentProcess, OpenProcess, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_VM_OPERATION,
            PROCESS_VM_READ, PROCESS_VM_WRITE, WaitForSingleObject,
        },
    },
};

/// An open process, its identity, and the module files that must not change while it is held.
#[derive(Debug)]
pub struct ProcessLease {
    pub(super) handle: OwnedHandle,
    target: ProcessTarget,
    _modules: Vec<ModuleIdentity>,
    #[cfg(test)]
    pub(super) fake: Option<std::sync::Arc<FakeIo>>,
}
impl ProcessLease {
    /// Opens `target` with the rights to start a thread in it and to read and write its memory, and checks it is still that
    /// process. `modules` stay held as long as the lease does.
    pub fn open_control(target: ProcessTarget, modules: Vec<ModuleIdentity>) -> Result<Self> {
        // SAFETY: validated target PID and explicit noninheritable bootstrap rights; the returned handle is immediately owned.
        let handle = owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION
                    | PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_SYNCHRONIZE
                    | PROCESS_CREATE_THREAD
                    | PROCESS_VM_OPERATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE,
                0,
                target.pid,
            )
        })?;
        if process_identity(&handle, target.pid)? != target {
            return invalid("target changed before control access");
        }
        Ok(Self {
            handle,
            target,
            _modules: modules,
            #[cfg(test)]
            fake: None,
        })
    }
    /// A copy of this lease that can only identify, wait on and read the process: all that watching what was loaded there
    /// needs. The thread-creation and memory-write rights stay with the original and close with it.
    pub fn observer(&self) -> Result<Self> {
        #[cfg(test)]
        if self.fake.is_some() {
            return Ok(Self {
                handle: self.handle.try_clone().map_err(super::RemoteError::from)?,
                target: self.target.clone(),
                _modules: self._modules.clone(),
                fake: self.fake.clone(),
            });
        }
        let mut handle = std::ptr::null_mut();
        // SAFETY: duplicates this process's own valid handle into itself with a subset of its rights.
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle.as_raw_handle(),
                GetCurrentProcess(),
                &mut handle,
                PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_VM_READ,
                0,
                0,
            )
        };
        if duplicated == 0 {
            return Err(last());
        }
        Ok(Self {
            handle: owned(handle)?,
            target: self.target.clone(),
            _modules: self._modules.clone(),
            #[cfg(test)]
            fake: None,
        })
    }
    pub fn target(&self) -> &ProcessTarget {
        &self.target
    }
    /// Whether the process is proved to have ended. A wait that fails does not prove it.
    pub fn has_exited(&self) -> bool {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.process_exited.load(std::sync::atomic::Ordering::SeqCst);
        }
        // SAFETY: query the retained process handle; no target memory is accessed.
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == 0 }
    }
    pub fn verify(&self) -> Result<()> {
        #[cfg(test)]
        if self.fake.is_some() {
            return Ok(());
        }
        if process_identity(&self.handle, self.target.pid)? != self.target {
            invalid("target process identity changed")
        } else {
            Ok(())
        }
    }
    /// Copies up to 2 MiB out of the process, after checking it is the same one. Nothing is read through a pointer here.
    pub fn read(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.read(address, length);
        }
        self.verify()?;
        if address == 0 || length > 2 * 1024 * 1024 || address.checked_add(length as u64).is_none() {
            return invalid("remote read bound");
        }
        let mut bytes = vec![0; length];
        let mut read = 0;
        // SAFETY: no target pointer is dereferenced in this process; the OS copies a bounded span.
        if unsafe { ReadProcessMemory(self.handle.as_raw_handle(), address as usize as *const c_void, bytes.as_mut_ptr().cast(), length, &mut read) } == 0 || read != length {
            return Err(last());
        }
        Ok(bytes)
    }
    pub(super) fn release(&self, address: u64) {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            fake.freed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        // SAFETY: caller supplies an allocation belonging to this exact process after exit proof.
        unsafe {
            VirtualFreeEx(self.handle.as_raw_handle(), address as usize as *mut c_void, 0, MEM_RELEASE);
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct FakeIo {
    pub(super) process_exited: std::sync::atomic::AtomicBool,
    pub(super) wait: std::sync::atomic::AtomicU32,
    pub(super) freed: std::sync::atomic::AtomicUsize,
    pub(super) reads: std::sync::atomic::AtomicUsize,
    pub(super) response: Vec<u8>,
}
#[cfg(test)]
impl FakeIo {
    fn read(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if address != 0x1000 || length != self.response.len() {
            return invalid("fake read bounds");
        }
        Ok(self.response.clone())
    }
}
#[cfg(test)]
impl ProcessLease {
    /// A lease on no process, over a handle that is only a placeholder.
    pub(super) fn fake(handle: OwnedHandle, target: ProcessTarget, fake: std::sync::Arc<FakeIo>) -> Self {
        Self {
            handle,
            target,
            _modules: Vec::new(),
            fake: Some(fake),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;

    #[test]
    fn a_loaded_module_is_watched_through_a_lease_that_cannot_write() {
        let pid = std::process::id();
        // SAFETY: opens this test process with the control rights; the handle is owned at once.
        let handle = owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
                0,
                pid,
            )
        })
        .unwrap();
        let target = process_identity(&handle, pid).unwrap();
        let lease = ProcessLease {
            handle,
            target,
            _modules: Vec::new(),
            fake: None,
        };
        let observer = lease.observer().unwrap();
        let mut value = 7_u64;
        let address = (&raw mut value) as u64;
        assert!(!observer.has_exited());
        assert_eq!(observer.read(address, 8).unwrap(), 7_u64.to_le_bytes());
        let mut written = 0;
        // SAFETY: targets a live local; only the access check is under test, and it refuses before any copy.
        let wrote = unsafe { WriteProcessMemory(observer.handle.as_raw_handle(), address as *const c_void, [9_u8; 8].as_ptr().cast(), 8, &mut written) };
        assert_eq!(wrote, 0, "the watching lease holds no write right");
        assert_eq!(std::hint::black_box(value), 7);
    }
}
