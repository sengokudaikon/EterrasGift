//! Working on another Windows x64 process from outside it: a process held by its identity ([`ProcessLease`]), the modules
//! it has loaded ([`inventory`]) checked against files held open and hashed ([`ModuleIdentity`]), its mitigation policies
//! ([`check_mitigations`]), the system loader's entry as it would be in that process ([`resolve_loader`]), and a thread
//! started in it on bytes copied there ([`call()`]). Names no process, module or program; what to load and what a call's
//! reply means is the caller's.
//!
//! A call that has not been proved finished may still be using its input, so nothing here frees that input, and nothing
//! here retries, stops, unloads or terminates anything. [`PendingCall`] says what happens to each thing it holds.

use crate::process::{IMAGE_PATH_UNITS, IdentityError, identity};
use std::{
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    System::Threading::{
        GetProcessMitigationPolicy, IsWow64Process2, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ProcessControlFlowGuardPolicy,
        ProcessDEPPolicy, ProcessDynamicCodePolicy, ProcessSignaturePolicy, ProcessUserShadowStackPolicy,
    },
};

mod call;
mod lease;
mod loader;
mod module;

pub use call::{CallCompletion, PendingCall, call};
pub use lease::ProcessLease;
pub use loader::resolve_loader;
pub use module::{Module, ModuleIdentity, find_module, inventory, same_path};

/// Why an operation on another process was refused or failed. The reason is a fixed string a caller can match on.
#[derive(Debug)]
pub enum RemoteError {
    Invalid(&'static str),
    Io(io::Error),
}
impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "invalid remote process operation: {reason}"),
            Self::Io(error) => write!(f, "remote process I/O: {error}"),
        }
    }
}
impl std::error::Error for RemoteError {}
impl From<io::Error> for RemoteError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

type Result<T> = std::result::Result<T, RemoteError>;

fn invalid<T>(reason: &'static str) -> Result<T> {
    Err(RemoteError::Invalid(reason))
}
fn last() -> RemoteError {
    io::Error::last_os_error().into()
}
fn owned(handle: HANDLE) -> Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(last());
    }
    // SAFETY: successful Win32 creation transfers exactly one handle to this guard.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// What tells one running process from another that later has the same ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessTarget {
    pub pid: u32,
    /// When the process started, in FILETIME ticks.
    pub created: u64,
    pub executable: PathBuf,
}

fn process_identity(handle: &OwnedHandle, pid: u32) -> Result<ProcessTarget> {
    let identity = identity(handle).map_err(|error| match error {
        IdentityError::Exited => RemoteError::Invalid("target exited"),
        IdentityError::Os(error) => error.into(),
    })?;
    let units = identity.executable.encode_wide().count();
    if units == 0 || units >= IMAGE_PATH_UNITS {
        return invalid("target executable path");
    }
    Ok(ProcessTarget {
        pid,
        created: identity.created,
        executable: identity.executable.into(),
    })
}

/// Opens `target` to identify it, wait on it and query its policies, and checks it is still that process. No memory or
/// thread rights.
pub fn open_query(target: &ProcessTarget) -> Result<OwnedHandle> {
    // SAFETY: only identity/synchronization access is requested before preflight completes.
    let handle = owned(unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, target.pid) })?;
    if process_identity(&handle, target.pid)? != *target {
        return invalid("target identity changed");
    }
    Ok(handle)
}

/// Refuses a process that is not native x64, or whose mitigation policies are other than this profile: DEP on, and the
/// dynamic code, control flow guard, user shadow stack and signature policies off, but for flag 0x100 of the shadow stack
/// policy.
pub fn check_mitigations(process: &OwnedHandle) -> Result<()> {
    // SAFETY: owned query-capable process handle and correctly sized local mitigation-policy buffers.
    unsafe {
        let mut process_machine = 0;
        let mut native_machine = 0;
        if !cfg!(target_arch = "x86_64") || IsWow64Process2(process.as_raw_handle(), &mut process_machine, &mut native_machine) == 0 {
            return Err(last());
        }
        if process_machine != 0 || native_machine != 0x8664 {
            return invalid("native Windows x64 target required");
        }
        let mut dep = [0u32; 2];
        if GetProcessMitigationPolicy(process.as_raw_handle(), ProcessDEPPolicy, dep.as_mut_ptr().cast(), 8) == 0 {
            return Err(last());
        }
        if dep[0] & 1 == 0 || dep[0] & !3 != 0 {
            return invalid("unsupported DEP policy");
        }
        for (policy, expected) in [
            (ProcessDynamicCodePolicy, 0),
            (ProcessControlFlowGuardPolicy, 0),
            (ProcessUserShadowStackPolicy, 0x100),
            (ProcessSignaturePolicy, 0),
        ] {
            let mut flags = 0u32;
            if GetProcessMitigationPolicy(process.as_raw_handle(), policy, (&mut flags as *mut u32).cast(), 4) == 0 {
                return Err(last());
            }
            if flags & !expected != 0 {
                return invalid("unsupported target mitigation profile");
            }
        }
    }
    Ok(())
}

/// `path` as the system loader's argument: UTF-16 with a closing NUL, in bytes, at a length Windows accepts.
pub fn path_argument(path: &Path) -> Result<Vec<u8>> {
    let value = crate::wide(path);
    let units = &value[..value.len() - 1];
    if units.is_empty() || units.len() >= 32767 || units.contains(&0) {
        return invalid("remote path length/NUL");
    }
    Ok(value.iter().flat_map(|unit| unit.to_le_bytes()).collect())
}

/// The address of `len` bytes at `rva` in a module loaded at `base` with `size` bytes of image.
pub fn module_span(base: u64, size: u32, rva: u32, len: usize) -> Result<u64> {
    let end = u64::from(rva).checked_add(len as u64).ok_or(RemoteError::Invalid("module span overflow"))?;
    if base == 0 || end > u64::from(size) {
        return invalid("module span bounds");
    }
    base.checked_add(u64::from(rva)).ok_or(RemoteError::Invalid("module base overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_span_bounds_cannot_wrap_or_cross_the_admitted_image() {
        assert_eq!(module_span(0x1000, 0x100, 0xf0, 16).unwrap(), 0x10f0);
        for (base, size, rva, len) in [(0, 100, 0, 1), (0x1000, 0x100, 0xf0, 17), (u64::MAX, 100, 1, 1), (1, 100, u32::MAX, 1)] {
            assert!(module_span(base, size, rva, len).is_err());
        }
    }

    #[test]
    fn a_path_argument_ends_in_nul_and_refuses_what_windows_would_not_take() {
        assert_eq!(path_argument(Path::new("ab")).unwrap(), [97, 0, 98, 0, 0, 0]);
        assert!(path_argument(Path::new("")).is_err());
        assert!(path_argument(Path::new("a\0b")).is_err());
        assert!(path_argument(Path::new(&"a".repeat(32767))).is_err());
    }
}
