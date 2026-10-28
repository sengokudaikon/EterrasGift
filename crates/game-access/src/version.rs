//! Supported ABI check before any remote work. Hashes separately bind these disk bytes
//! to the loaded module; a matching version is not proof of graph/lifetime assumptions.
use std::{ffi::c_void, io, os::windows::ffi::OsStrExt, path::Path};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW};

/// The executable's version resource is Unity's, the engine the supported game ships.
pub fn check_unity(path: &Path) -> io::Result<()> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut ignored = 0;
    // SAFETY: nul-terminated local path and writable DWORD output.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &raw mut ignored) };
    if size == 0 {
        return Err(io::Error::other(format!("Cannot read UnityPlayer version: {}", io::Error::last_os_error())));
    }
    if size > 16 * 1024 * 1024 {
        return Err(io::Error::other("UnityPlayer version resource is too large"));
    }
    let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
    // SAFETY: buffer is aligned and at least size writable bytes; path remains live.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) } == 0 {
        return Err(io::Error::other(format!("Cannot read UnityPlayer version: {}", io::Error::last_os_error())));
    }
    let mut value: *mut c_void = std::ptr::null_mut();
    let mut length = 0;
    let root = [b'\\' as u16, 0];
    // SAFETY: successfully loaded version data; root query returns a borrowed pointer.
    if unsafe { VerQueryValueW(buffer.as_ptr().cast(), root.as_ptr(), &raw mut value, &raw mut length) } == 0 {
        return Err(io::Error::other("UnityPlayer has no fixed file version"));
    }
    let offset = value
        .addr()
        .checked_sub(buffer.as_ptr().addr())
        .ok_or_else(|| io::Error::other("Invalid UnityPlayer version pointer"))?;
    if length < size_of::<VS_FIXEDFILEINFO>() as u32 || offset.checked_add(length as usize).is_none_or(|end| end > size as usize) {
        return Err(io::Error::other("Invalid UnityPlayer version resource"));
    }
    // SAFETY: checked initialized region inside buffer, enough bytes for this plain-data structure.
    let info = unsafe { value.cast::<VS_FIXEDFILEINFO>().read_unaligned() };
    check_fixed(&info)
}

fn check_fixed(info: &VS_FIXEDFILEINFO) -> io::Result<()> {
    if info.dwSignature != 0xfeef04bd || info.dwStrucVersion != 0x00010000 {
        return Err(io::Error::other("Invalid UnityPlayer fixed file version"));
    }
    let version = [
        info.dwFileVersionMS >> 16,
        info.dwFileVersionMS & 0xffff,
        info.dwFileVersionLS >> 16,
        info.dwFileVersionLS & 0xffff,
    ];
    if version[..2] != [6000, 4] {
        return Err(io::Error::other(format!(
            "Unsupported UnityPlayer {}.{}.{}.{}; the trainer supports Unity 6000.4. No helper was loaded.",
            version[0], version[1], version[2], version[3]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_supported_engine_family_passes() {
        // SAFETY: integer-only Windows structure; remaining fields are irrelevant to version selection.
        let mut info: VS_FIXEDFILEINFO = unsafe { std::mem::zeroed() };
        info.dwSignature = 0xfeef04bd;
        info.dwStrucVersion = 0x00010000;
        info.dwFileVersionLS = 8 << 16 | 1;
        for (major, minor, supported) in [(6000, 4, true), (6000, 3, false), (6000, 5, false), (2022, 4, false)] {
            info.dwFileVersionMS = major << 16 | minor;
            assert_eq!(check_fixed(&info).is_ok(), supported);
        }
        info.dwFileVersionMS = 6000 << 16 | 4;
        info.dwSignature = 0;
        assert!(check_fixed(&info).is_err());
        info.dwSignature = 0xfeef04bd;
        info.dwStrucVersion = 0;
        assert!(check_fixed(&info).is_err());
    }

    #[test]
    fn missing_or_versionless_files_fail_closed() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("UnityPlayer.dll");
        assert!(check_unity(&path).is_err());
        std::fs::write(&path, b"not a PE version resource").unwrap();
        assert!(check_unity(&path).is_err());
    }

    #[test]
    fn windows_version_resource_is_read_but_refused_as_an_unsupported_engine() {
        let path = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/kernel32.dll");
        let error = check_unity(&path).unwrap_err();
        assert!(error.to_string().contains("Unsupported UnityPlayer"), "{error}");
    }
}
