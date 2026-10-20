//! Explicit process identity. Capturing it queries identity only, without memory access or injection.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Windows FILETIME, 100ns since1601. PID reuse is a different identity.
    pub created: u64,
    pub executable: PathBuf,
}

#[cfg(windows)]
mod windows {
    use super::ProcessIdentity;
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
    };

    pub(crate) struct Process {
        handle: OwnedHandle,
        pid: u32,
    }
    impl Process {
        pub(crate) fn open(pid: u32) -> io::Result<Self> {
            if pid == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "process ID must be nonzero"));
            }
            // SAFETY: query/synchronize only; successful handle ownership transfers immediately.
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: uniquely owned valid process handle, closed by OwnedHandle.
            Ok(Self {
                // SAFETY: uniquely owned valid process handle, closed by OwnedHandle.
                handle: unsafe { OwnedHandle::from_raw_handle(handle) },
                pid,
            })
        }
        pub(crate) fn identity(&self) -> io::Result<ProcessIdentity> {
            // SAFETY: owned process handle has SYNCHRONIZE access; zero timeout never suspends it.
            if unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } != 0x102 {
                return Err(io::Error::new(io::ErrorKind::NotFound, "target process exited"));
            }
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            let mut path = vec![0u16; 32768];
            let mut length = path.len() as u32;
            // SAFETY: query-only owned handle and correctly sized output storage.
            if unsafe { GetProcessTimes(self.handle.as_raw_handle(), &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user) } == 0
                // SAFETY: owned query handle, correctly sized local output slots and bounded writable path buffer; the query retains no pointers.
                || unsafe { QueryFullProcessImageNameW(self.handle.as_raw_handle(), 0, path.as_mut_ptr(), &raw mut length) } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(ProcessIdentity {
                pid: self.pid,
                created: u64::from(created.dwLowDateTime) | (u64::from(created.dwHighDateTime) << 32),
                executable: std::ffi::OsString::from_wide(&path[..length as usize]).into(),
            })
        }
    }
    impl ProcessIdentity {
        pub fn capture(pid: u32) -> io::Result<Self> {
            Process::open(pid)?.identity()
        }
    }
}
#[cfg(windows)]
pub(crate) use windows::Process;
