//! Folders Windows knows by name.

use std::path::PathBuf;

/// LocalLow as Windows knows it (it can be redirected apart from Local AppData); beside Local AppData when Windows can't say.
pub fn local_low() -> PathBuf {
    known_local_low().unwrap_or_else(|| {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
        local.parent().map_or_else(|| local.clone(), |parent| parent.to_path_buf()).join("LocalLow")
    })
}

#[cfg(windows)]
fn known_local_low() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppDataLow, SHGetKnownFolderPath};
    let mut path = std::ptr::null_mut();
    // SAFETY: the folder id is a static GUID; Windows allocates `path`, which is freed below whatever the result.
    let result = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppDataLow, 0, std::ptr::null_mut(), &mut path) };
    let found = (result == 0 && !path.is_null()).then(|| {
        // SAFETY: on success `path` is a NUL-terminated wide string.
        let length = (0..).take_while(|&index| unsafe { *path.add(index) } != 0).count();
        // SAFETY: `length` units were just read from it.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path, length) })
    });
    // SAFETY: `path` came from SHGetKnownFolderPath (or is null, which CoTaskMemFree accepts).
    unsafe { CoTaskMemFree(path.cast()) };
    found.filter(|text| !text.is_empty()).map(PathBuf::from)
}

#[cfg(not(windows))]
fn known_local_low() -> Option<PathBuf> {
    None
}
