//! The processes running now, by ID and executable file name: a read-only census that opens no process.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case() {
        let name: Vec<u16> = "Last Epoch.exe".encode_utf16().collect();
        assert!(is_named(&name, "last epoch.EXE"));
        assert!(!is_named(&name, "Last Epoch"));
    }
}
