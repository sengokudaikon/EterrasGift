//! Explicit process identity. Capturing it queries identity only, without memory access or injection.
#[cfg(windows)]
mod windows {
    use eg_trainer_wire::process::ProcessIdentity;
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
    /// Every running process: its ID and executable file name. A read-only census; no process is opened.
    pub fn census() -> io::Result<Vec<(u32, String)>> {
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, System::Diagnostics::ToolHelp::*};
        // SAFETY: read-only process census; ownership transfers to OwnedHandle.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful snapshot handle is owned exactly once.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: the Win32 plain-data structure requires zero initialization and dwSize.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Vec::new();
        // SAFETY: the snapshot and initialized output structure stay alive throughout iteration.
        let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
        while more {
            let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            found.push((entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..end])));
            // SAFETY: same snapshot/output lifetime as above.
            more = unsafe { Process32NextW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
        }
        Ok(found)
    }

    pub fn capture(pid: u32) -> io::Result<ProcessIdentity> {
        Process::open(pid)?.identity()
    }
}
#[cfg(windows)]
pub(crate) use windows::Process;

#[cfg(windows)]
pub use windows::{capture, census};
