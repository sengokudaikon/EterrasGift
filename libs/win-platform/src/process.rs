//! The processes running now: a census by ID and executable file name that opens no process, and what a process says about
//! itself when asked. Asking reads nothing from the process: no handle opened here has memory rights.
//!
//! There are two ways to tell a process is running, and callers keep to the one they need. [`is_running`] waits on the
//! handle, which needs the synchronize right and so suits a handle kept to watch a process. [`started_if_running`] reads the
//! exit code, which needs the limited query right only, so it reaches more processes and holds nothing open; a process
//! that exited with code 259 (`STILL_ACTIVE`) counts as running.

use std::io;

/// Calls `visit` with every running process's ID and executable file name, as Windows gives it (UTF-16, no path).
#[cfg(windows)]
pub fn each(mut visit: impl FnMut(u32, &[u16])) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
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
    // SAFETY: the snapshot and initialized output structure stay alive throughout iteration.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    while more {
        let end = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
        visit(entry.th32ProcessID, &entry.szExeFile[..end]);
        // SAFETY: same snapshot/output lifetime as above.
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn each(_visit: impl FnMut(u32, &[u16])) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "The process census is available on Windows."))
}

/// Whether an executable file name from [`each`] is `name`, compared without case, as Windows names files, and without a
/// string per process.
pub fn is_named(executable: &[u16], name: &str) -> bool {
    char::decode_utf16(executable.iter().copied())
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .flat_map(char::to_uppercase)
        .eq(name.chars().flat_map(char::to_uppercase))
}

/// The oldest running process whose executable file name is `executable_name`: its ID and start time. Processes that exit
/// mid-query, or that can't be asked, are skipped. None too when the census fails.
#[cfg(all(windows, feature = "time"))]
pub fn oldest_named(executable_name: &str) -> Option<(u32, jiff::Timestamp)> {
    let mut oldest: Option<(u32, jiff::Timestamp)> = None;
    each(|pid, executable| {
        if is_named(executable, executable_name)
            && let Some(start) = started_if_running(pid).and_then(|ticks| crate::time::from_filetime(ticks).ok())
            && oldest.is_none_or(|(_, known)| start < known)
        {
            oldest = Some((pid, start));
        }
    })
    .ok()?;
    oldest
}

#[cfg(all(not(windows), feature = "time"))]
pub fn oldest_named(_executable_name: &str) -> Option<(u32, jiff::Timestamp)> {
    None
}

#[cfg(windows)]
pub use identity::*;

#[cfg(windows)]
mod identity {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{FILETIME, STILL_ACTIVE, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
    };

    /// The room [`identity`] gives an executable's path, in UTF-16 units, its closing NUL included.
    pub const IMAGE_PATH_UNITS: usize = 32768;

    /// What tells one process from another that later has the same ID.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Identity {
        /// When the process started, in FILETIME ticks.
        pub created: u64,
        /// Its executable's full path, as Windows reports it.
        pub executable: OsString,
    }

    #[derive(Debug)]
    pub enum IdentityError {
        /// The process has ended, though its handle is still good.
        Exited,
        /// Windows would not say.
        Os(io::Error),
    }

    /// Opens a process to ask about it and to wait on it: the limited query and synchronize rights, nothing more.
    pub fn open_for_identity(pid: u32) -> io::Result<OwnedHandle> {
        // SAFETY: query and synchronize rights only, with no pointer arguments.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful OpenProcess gives one handle, owned by this guard alone.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    /// Whether the process behind `process` still runs. The handle needs the synchronize right; a wait that fails counts as
    /// not running.
    pub fn is_running(process: impl AsHandle) -> bool {
        // SAFETY: the handle is borrowed live for the call, and a zero timeout never blocks.
        unsafe { WaitForSingleObject(process.as_handle().as_raw_handle(), 0) == WAIT_TIMEOUT }
    }

    /// When the process behind `process` started, in FILETIME ticks.
    pub fn created(process: impl AsHandle) -> io::Result<u64> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the handle is borrowed live for the call, and the four outputs are local FILETIMEs.
        if unsafe { GetProcessTimes(process.as_handle().as_raw_handle(), &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(u64::from(created.dwLowDateTime) | (u64::from(created.dwHighDateTime) << 32))
    }

    /// The identity of the process behind `process`, while it runs. Takes any handle with the query and synchronize rights.
    pub fn identity(process: impl AsHandle) -> Result<Identity, IdentityError> {
        let process = process.as_handle();
        if !is_running(process) {
            return Err(IdentityError::Exited);
        }
        let created = created(process).map_err(IdentityError::Os)?;
        let mut path = vec![0u16; IMAGE_PATH_UNITS];
        let mut length = path.len() as u32;
        // SAFETY: the handle is borrowed live for the call, `length` states the buffer's room, and the query keeps no pointer.
        if unsafe { QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &raw mut length) } == 0 {
            return Err(IdentityError::Os(io::Error::last_os_error()));
        }
        Ok(Identity {
            created,
            executable: OsString::from_wide(&path[..length as usize]),
        })
    }

    /// When `pid` started, in FILETIME ticks, or None when it has exited or can't be asked. See the module's note on telling
    /// a process runs.
    pub fn started_if_running(pid: u32) -> Option<u64> {
        // SAFETY: the limited query right only, with no pointer arguments.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: a successful OpenProcess gives one handle, owned by this guard alone.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let started = created(&handle).ok()?;
        let mut exit_code = 0u32;
        // SAFETY: `handle` is open with the right this call needs, and the output is a local u32.
        let running = unsafe { GetExitCodeProcess(handle.as_raw_handle(), &raw mut exit_code) } != 0 && exit_code == STILL_ACTIVE as u32;
        running.then_some(started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case() {
        let name: Vec<u16> = "Some Game.exe".encode_utf16().collect();
        assert!(is_named(&name, "some game.EXE"));
        assert!(!is_named(&name, "Some Game"));
    }

    #[cfg(windows)]
    #[test]
    fn this_process_is_known_by_every_question() {
        let pid = std::process::id();
        let handle = open_for_identity(pid).unwrap();
        assert!(is_running(&handle));
        let found = identity(&handle).unwrap();
        assert_eq!(found.created, created(&handle).unwrap());
        assert_eq!(found.executable, std::env::current_exe().unwrap().into_os_string());
        assert_eq!(started_if_running(pid), Some(found.created));
    }

    #[cfg(windows)]
    #[test]
    fn a_process_that_has_exited_says_so_though_its_handle_is_good() {
        let mut child = std::process::Command::new(std::env::var_os("ComSpec").unwrap()).args(["/c", "exit"]).spawn().unwrap();
        let handle = open_for_identity(child.id()).unwrap();
        child.wait().unwrap();
        assert!(!is_running(&handle));
        assert!(matches!(identity(&handle), Err(IdentityError::Exited)));
        assert!(created(&handle).is_ok(), "an ended process keeps its times");
    }

    #[cfg(all(windows, feature = "time"))]
    #[test]
    fn this_process_is_found_by_its_executable_name() {
        let executable = std::env::current_exe().unwrap();
        let name = executable.file_name().unwrap().to_string_lossy().into_owned();
        let (_, start) = oldest_named(&name).unwrap();
        assert!(start <= jiff::Timestamp::now());
        assert_eq!(oldest_named("no process is named this.exe"), None);
    }
}
