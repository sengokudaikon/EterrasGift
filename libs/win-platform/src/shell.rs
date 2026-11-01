//! Opens things the way Windows does: a URL in the app registered for it (Steam's `steam://`), a folder in File Explorer.

use std::io;

/// Opens `target` through the Windows shell. The error is Windows' own reason.
#[cfg(windows)]
pub fn open(target: impl AsRef<std::ffi::OsStr>) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = crate::wide("open");
    let file = crate::wide(target);
    // SAFETY: the strings are NUL-terminated and outlive the call; the other arguments may be null.
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    // Values above 32 mean success; anything else leaves its reason in the thread's last error.
    if result as usize > 32 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(windows))]
pub fn open(_target: impl AsRef<std::ffi::OsStr>) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Only Windows can open this."))
}

/// Opens a file or folder the way Windows does, or shows it selected in File Explorer (`reveal`); `create` makes the
/// folder first.
#[cfg(windows)]
pub fn open_path(path: &std::path::Path, reveal: bool, create: bool) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    if create {
        std::fs::create_dir_all(path)?;
    }
    if reveal {
        std::process::Command::new("explorer.exe").arg("/select,").arg(path).creation_flags(0x0800_0000).spawn()?;
        return Ok(());
    }
    open(path)
}

#[cfg(not(windows))]
pub fn open_path(_path: &std::path::Path, _reveal: bool, _create: bool) -> io::Result<()> {
    Err(io::Error::other("Opening files is available on Windows."))
}
