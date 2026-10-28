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

/// Opens a file or folder the way Windows does, or shows it selected in File Explorer (`reveal`); `create` makes the
/// folder first.
#[cfg(windows)]
pub fn open_path(path: &std::path::Path, reveal: bool, create: bool) -> io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, process::CommandExt};
    if create {
        std::fs::create_dir_all(path)?;
    }
    if reveal {
        std::process::Command::new("explorer.exe").arg("/select,").arg(path).creation_flags(0x0800_0000).spawn()?;
        return Ok(());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let open: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // SAFETY: all UTF-16 strings are terminated and live until the call returns; ShellExecute opens the user's selected file.
    let result = unsafe { windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
    if result as isize <= 32 {
        Err(io::Error::other(format!("Could not open {} (Windows error {}).", path.display(), result as isize)))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn open_path(_path: &std::path::Path, _reveal: bool, _create: bool) -> io::Result<()> {
    Err(io::Error::other("Opening files is available on Windows."))
}
