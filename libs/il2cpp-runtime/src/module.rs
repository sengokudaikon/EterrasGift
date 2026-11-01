//! Finding a loaded runtime module, its exports and its mapped size, through the Windows loader. The caller names the
//! module; nothing here keeps it loaded.

use std::{
    ffi::{CStr, c_void},
    ptr::NonNull,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// The module loaded under this file name, if it is.
///
/// # Safety
/// The returned handle takes no reference: the caller guarantees the module's lifetime for as long as it is used.
pub unsafe fn loaded(name: &str) -> Option<NonNull<u8>> {
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: read-only already-loaded module lookup over a local NUL-terminated name.
    NonNull::new(unsafe { GetModuleHandleW(name.as_ptr()) }.cast())
}

/// The address of a module's export.
///
/// # Safety
/// `module` is a retained loaded module, and the caller selects only exports it declares.
pub unsafe fn export(module: NonNull<u8>, name: &'static CStr) -> Option<NonNull<c_void>> {
    // SAFETY: caller retains the exact module and selects only declared exports.
    unsafe { GetProcAddress(module.as_ptr().cast(), name.as_ptr().cast()) }.and_then(|value| NonNull::new(value as *const () as *mut c_void))
}

/// A loaded x64 module's mapped size, from its PE headers.
///
/// # Safety
/// `module` is a live loaded Windows PE image.
pub unsafe fn image_size(module: NonNull<u8>) -> Result<usize, String> {
    // SAFETY: module is a live loaded Windows PE image; the loader validated its mapped headers.
    unsafe {
        let base = module.as_ptr();
        if std::slice::from_raw_parts(base, 2) != b"MZ" {
            return Err("invalid module header".into());
        }
        let pe = base.add(base.add(0x3c).cast::<u32>().read_unaligned() as usize);
        if std::slice::from_raw_parts(pe, 4) != b"PE\0\0" || pe.add(4).cast::<u16>().read_unaligned() != 0x8664 {
            return Err("expected x64 module".into());
        }
        Ok(pe.add(24 + 56).cast::<u32>().read_unaligned() as usize)
    }
}
