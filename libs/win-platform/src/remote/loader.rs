//! The system loader's entry, found here and placed in another process's address space by that process's module list.

use super::{Module, ModuleIdentity, RemoteError, Result, find_module, invalid, last};
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress,
};

fn loader_module(address: usize) -> Result<(usize, ModuleIdentity)> {
    // SAFETY: Windows locates and pins the already loaded module containing this local export.
    unsafe {
        let mut containing = std::ptr::null_mut();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            address as *const u16,
            &mut containing,
        ) == 0
        {
            return Err(last());
        }
        let mut path = vec![0u16; 32768];
        let len = GetModuleFileNameW(containing, path.as_mut_ptr(), path.len() as u32);
        if len == 0 || len as usize >= path.len() {
            return invalid("system loader containing-module path");
        }
        let path: PathBuf = OsString::from_wide(&path[..len as usize]).into();
        Ok((containing as usize, ModuleIdentity::open(&path)?))
    }
}

/// Ordinary x64 RIP-relative import thunks, including the optional REX.W prefix.
fn import_jump_slot(address: u64, code: &[u8]) -> Result<Option<u64>> {
    let prefix = if code.starts_with(&[0x48, 0xff, 0x25]) {
        3
    } else if code.starts_with(&[0xff, 0x25]) {
        2
    } else {
        return Ok(None);
    };
    let displacement = code.get(prefix..prefix + 4).ok_or(RemoteError::Invalid("system loader import thunk"))?;
    let displacement = i32::from_le_bytes(displacement.try_into().unwrap());
    address
        .checked_add((prefix + 4) as u64)
        .and_then(|next| next.checked_add_signed(i64::from(displacement)))
        .map(Some)
        .ok_or(RemoteError::Invalid("system loader import slot"))
}

/// `LoadLibraryW`'s implementation: its address in the process whose modules are `modules`, the file it lives in, held,
/// and its first instructions as this process and that file agree on them, to compare with that process's.
pub fn resolve_loader(modules: &[Module]) -> Result<(u64, ModuleIdentity, Vec<u8>)> {
    // Resolve Windows' import thunk to its declared implementation. Steam instruments the
    // kernel32 wrapper in game processes; its hook is left untouched and is never followed.
    // SAFETY: bounded local system export reads are checked against pinned module/file spans.
    unsafe {
        let kernel = GetModuleHandleW(windows_sys::core::w!("kernel32.dll"));
        if kernel.is_null() {
            return Err(last());
        }
        let function = GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()).ok_or_else(last)?;
        let mut address = function as *const () as usize;
        let (base, wrapper) = loader_module(address)?;
        let rva = u32::try_from(address.checked_sub(base).ok_or(RemoteError::Invalid("system loader RVA"))?).map_err(|_| RemoteError::Invalid("system loader RVA"))?;
        let code = wrapper.bytes(rva, 16, true)?;
        if std::slice::from_raw_parts(address as *const u8, code.len()) != code {
            return invalid("local system loader instructions differ");
        }
        if let Some(slot) = import_jump_slot(address as u64, &code)? {
            let slot_rva = u32::try_from(slot.checked_sub(base as u64).ok_or(RemoteError::Invalid("system loader import bounds"))?)
                .map_err(|_| RemoteError::Invalid("system loader import bounds"))?;
            wrapper.layout().file_offset(u64::from(slot_rva), 8, false)?;
            let implementation = std::ptr::read_unaligned(slot as *const usize);
            let kernel_base = GetModuleHandleW(windows_sys::core::w!("KernelBase.dll"));
            if kernel_base.is_null() {
                return invalid("system loader implementation module");
            }
            let declared = GetProcAddress(kernel_base, c"LoadLibraryW".as_ptr().cast()).ok_or_else(last)? as *const () as usize;
            if implementation != declared {
                return invalid("system loader import target differs from declared export");
            }
            address = implementation;
        }
        let (base, identity) = loader_module(address)?;
        let offset = u32::try_from(address.checked_sub(base).ok_or(RemoteError::Invalid("system loader RVA"))?).map_err(|_| RemoteError::Invalid("system loader RVA"))?;
        let instructions = identity.bytes(offset, 16, true)?;
        if std::slice::from_raw_parts(address as *const u8, instructions.len()) != instructions {
            return invalid("local system loader implementation instructions differ");
        }
        let remote = find_module(modules, &identity)?;
        if remote.size != identity.image_size() {
            return invalid("system loader image size");
        }
        Ok((remote.base + u64::from(offset), identity, instructions))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{inventory, same_path};
    use super::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    #[test]
    fn import_jump_slots_decode_both_thunks_and_refuse_bad_spans() {
        assert_eq!(import_jump_slot(0x1000, &[0x48, 0xff, 0x25, 0x19, 0, 0, 0]).unwrap(), Some(0x1020));
        assert_eq!(import_jump_slot(0x1000, &[0xff, 0x25, 0xea, 0xff, 0xff, 0xff]).unwrap(), Some(0xff0));
        assert_eq!(import_jump_slot(0x1000, &[0xe9, 0, 0, 0, 0]).unwrap(), None, "never follow an overlay detour");
        assert!(import_jump_slot(0x1000, &[0xff, 0x25, 0]).is_err());
        assert!(import_jump_slot(u64::MAX - 2, &[0xff, 0x25, 0, 0, 0, 0]).is_err());
        assert!(import_jump_slot(0, &[0xff, 0x25, 0x80, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn system_loader_resolves_the_implementation_before_remote_wrapper_comparison() {
        // SAFETY: read-only local OS module/export queries; no game or remote execution.
        let (pid, wrapper, declared) = unsafe {
            let kernel = GetModuleHandleW(windows_sys::core::w!("kernel32.dll"));
            let base = GetModuleHandleW(windows_sys::core::w!("KernelBase.dll"));
            (
                GetCurrentProcessId(),
                GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()).unwrap() as *const () as usize,
                GetProcAddress(base, c"LoadLibraryW".as_ptr().cast()).unwrap() as *const () as usize,
            )
        };
        let (base, identity) = loader_module(wrapper).unwrap();
        let code = identity.bytes((wrapper - base) as u32, 16, true).unwrap();
        let mut remote = inventory(pid).unwrap();
        if import_jump_slot(wrapper as u64, &code).unwrap().is_some() {
            // A target's kernel32 wrapper may have its own ASLR base or an overlay detour.
            // Only the untouched declared implementation should become the remote entry.
            remote.iter_mut().find(|module| same_path(&module.path, identity.path())).unwrap().base += 0x100000;
            let (address, implementation, instructions) = resolve_loader(&remote).unwrap();
            assert_eq!(address, declared as u64);
            assert_ne!(implementation.path(), identity.path());
            assert!(!instructions.is_empty());
        } else {
            assert_eq!(resolve_loader(&remote).unwrap().0, wrapper as u64);
        }
    }
}
