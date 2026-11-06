//! Pipe access is limited to the current process's user and LocalSystem, at medium integrity or above.
use std::{
    ffi::c_void,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: these allocations come only from the LocalAlloc-backed security APIs.
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) struct PipeSecurity(LocalAllocation);
impl PipeSecurity {
    fn current_user_sid() -> io::Result<String> {
        // SAFETY: query only, correctly aligned TokenUser storage, and API-owned
        // SID/descriptor allocations kept alive through their consumers.
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = OwnedHandle::from_raw_handle(token);
            let mut length = 0;
            GetTokenInformation(token.as_raw_handle(), TokenUser, std::ptr::null_mut(), 0, &mut length);
            if length < size_of::<TOKEN_USER>() as u32 {
                return Err(io::Error::last_os_error());
            }
            let mut storage = vec![0_usize; (length as usize).div_ceil(size_of::<usize>())];
            if GetTokenInformation(token.as_raw_handle(), TokenUser, storage.as_mut_ptr().cast(), length, &mut length) == 0 {
                return Err(io::Error::last_os_error());
            }
            let user = &*storage.as_ptr().cast::<TOKEN_USER>();
            let mut sid = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
                return Err(io::Error::last_os_error());
            }
            let sid_memory = LocalAllocation(sid.cast());
            let mut count = 0;
            while *sid.add(count) != 0 {
                count += 1;
            }
            let sid_text = String::from_utf16(std::slice::from_raw_parts(sid, count)).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            drop(sid_memory);
            Ok(sid_text)
        }
    }
    pub(super) fn current_user() -> io::Result<Self> {
        let sid_text = Self::current_user_sid()?;
        // Protected DACL: no inherited Everyone/anonymous/admin grants. The medium label refuses reads as well as writes
        // from a lower integrity level, so a sandboxed process of the same user cannot hold the only instance.
        let descriptor = crate::wide(format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid_text})S:(ML;;NWNR;;;ME)"));
        // SAFETY: NUL-terminated SDDL, with owned API output transferred on success.
        unsafe {
            let mut security = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(descriptor.as_ptr(), SDDL_REVISION_1, &mut security, std::ptr::null_mut()) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(LocalAllocation(security)))
        }
    }
    pub(super) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::{Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW, DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION};

    #[test]
    fn descriptor_grants_only_system_and_current_user_at_medium_or_above_with_no_inheritance() {
        let security = PipeSecurity::current_user().unwrap();
        let attributes = security.attributes();
        assert_eq!(attributes.bInheritHandle, 0);
        let mut text = std::ptr::null_mut();
        let mut length = 0;
        // SAFETY: descriptor remains owned, correctly sized API outputs and allocation.
        unsafe {
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    attributes.lpSecurityDescriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                    &mut text,
                    &mut length
                ),
                0
            );
            let _allocation = LocalAllocation(text.cast());
            let units = std::slice::from_raw_parts(text, length as usize);
            let sddl = String::from_utf16(&units[..units.iter().position(|&unit| unit == 0).unwrap()]).unwrap();
            assert_eq!(sddl, format!("D:P(A;;GA;;;SY)(A;;GA;;;{})S:(ML;;NWNR;;;ME)", PipeSecurity::current_user_sid().unwrap()));
        }
    }
}
