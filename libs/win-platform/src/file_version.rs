//! A file's fixed version resource.

use std::{ffi::c_void, io, path::Path};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW};

/// Why a file has no version to read. The caller says it in its own words.
#[derive(Debug)]
pub enum FileVersionError {
    /// Windows could not read the resource, or the file has none.
    Unreadable(io::Error),
    /// The resource is larger than any real one (over 16 MiB).
    TooLarge,
    /// The resource has no fixed part.
    NoFixedVersion,
    /// Windows pointed at something outside the resource.
    InvalidPointer,
    /// The fixed part is too short, or runs past the resource.
    InvalidResource,
    /// The fixed part's signature or structure version is wrong.
    InvalidFixedVersion,
}

/// The file version in `path`'s fixed version resource, most significant part first: 6000.4.8.1 is `[6000, 4, 8, 1]`.
pub fn read(path: &Path) -> Result<[u32; 4], FileVersionError> {
    let path = crate::wide(path);
    let mut ignored = 0;
    // SAFETY: nul-terminated local path and writable DWORD output.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &raw mut ignored) };
    if size == 0 {
        return Err(FileVersionError::Unreadable(io::Error::last_os_error()));
    }
    if size > 16 * 1024 * 1024 {
        return Err(FileVersionError::TooLarge);
    }
    let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
    // SAFETY: buffer is aligned and at least size writable bytes; path remains live.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) } == 0 {
        return Err(FileVersionError::Unreadable(io::Error::last_os_error()));
    }
    let mut value: *mut c_void = std::ptr::null_mut();
    let mut length = 0;
    let root = [b'\\' as u16, 0];
    // SAFETY: successfully loaded version data; root query returns a borrowed pointer.
    if unsafe { VerQueryValueW(buffer.as_ptr().cast(), root.as_ptr(), &raw mut value, &raw mut length) } == 0 {
        return Err(FileVersionError::NoFixedVersion);
    }
    let offset = value.addr().checked_sub(buffer.as_ptr().addr()).ok_or(FileVersionError::InvalidPointer)?;
    if length < size_of::<VS_FIXEDFILEINFO>() as u32 || offset.checked_add(length as usize).is_none_or(|end| end > size as usize) {
        return Err(FileVersionError::InvalidResource);
    }
    // SAFETY: checked initialized region inside buffer, enough bytes for this plain-data structure.
    let info = unsafe { value.cast::<VS_FIXEDFILEINFO>().read_unaligned() };
    from_fixed(&info)
}

fn from_fixed(info: &VS_FIXEDFILEINFO) -> Result<[u32; 4], FileVersionError> {
    if info.dwSignature != 0xfeef04bd || info.dwStrucVersion != 0x00010000 {
        return Err(FileVersionError::InvalidFixedVersion);
    }
    Ok([
        info.dwFileVersionMS >> 16,
        info.dwFileVersionMS & 0xffff,
        info.dwFileVersionLS >> 16,
        info.dwFileVersionLS & 0xffff,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_part_is_checked_before_it_is_believed() {
        // SAFETY: integer-only Windows structure; remaining fields are irrelevant to the version.
        let mut info: VS_FIXEDFILEINFO = unsafe { std::mem::zeroed() };
        info.dwSignature = 0xfeef04bd;
        info.dwStrucVersion = 0x00010000;
        info.dwFileVersionMS = 6000 << 16 | 4;
        info.dwFileVersionLS = 8 << 16 | 1;
        assert_eq!(from_fixed(&info).unwrap(), [6000, 4, 8, 1]);
        info.dwSignature = 0;
        assert!(matches!(from_fixed(&info), Err(FileVersionError::InvalidFixedVersion)));
        info.dwSignature = 0xfeef04bd;
        info.dwStrucVersion = 0;
        assert!(matches!(from_fixed(&info), Err(FileVersionError::InvalidFixedVersion)));
    }

    #[test]
    fn a_missing_or_versionless_file_has_no_version() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("some.dll");
        assert!(matches!(read(&path), Err(FileVersionError::Unreadable(_))));
        std::fs::write(&path, b"not a PE version resource").unwrap();
        assert!(matches!(read(&path), Err(FileVersionError::Unreadable(_))));
    }

    #[test]
    fn a_windows_file_has_a_version() {
        let path = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/kernel32.dll");
        assert_ne!(read(&path).unwrap(), [0; 4]);
    }
}
