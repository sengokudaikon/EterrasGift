//! Opens things the way Windows does: a URL in the app registered for it (Steam's `steam://`), a folder in File Explorer.

use std::io;

/// Opens `target` through the Windows shell. The error is Windows' own reason.
#[cfg(windows)]
pub fn open(target: &str) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the strings are NUL-terminated and outlive the call; the other arguments may be null.
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    // Values above 32 mean success; anything else leaves its reason in the thread's last error.
    if result as usize > 32 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(windows))]
pub fn open(_target: &str) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Only Windows can open this."))
}
