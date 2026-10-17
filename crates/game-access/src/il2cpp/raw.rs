//! Experimental declarations for the modern pointer-valued GC-handle ABI.
//!
//! These are runtime exports, not generated method bodies or Unity internal calls. No runtime
//! structure layout is assumed. Historical uint32 GC handles require a different adapter.

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;

macro_rules! opaque {
    ($($name:ident),+ $(,)?) => { $(
        #[doc = "Opaque runtime-owned type. Never construct or dereference it in Rust."]
        #[repr(C)]
        pub struct $name { _private: [u8; 0] }
    )+ };
}

opaque!(Domain, Thread, Object, MethodInfo, Exception);

pub type DomainGet = unsafe extern "C-unwind" fn() -> *mut Domain;
pub type ThreadCurrent = unsafe extern "C-unwind" fn() -> *mut Thread;
pub type ThreadAttach = unsafe extern "C-unwind" fn(*mut Domain) -> *mut Thread;
pub type ThreadDetach = unsafe extern "C-unwind" fn(*mut Thread);
pub type GcHandleNew = unsafe extern "C-unwind" fn(*mut Object, bool) -> *mut c_void;
pub type GcHandleFree = unsafe extern "C-unwind" fn(*mut c_void);
pub type GcHandleTarget = unsafe extern "C-unwind" fn(*mut c_void) -> *mut Object;
pub type RuntimeInvoke = unsafe extern "C-unwind" fn(*const MethodInfo, *mut c_void, *mut *mut c_void, *mut *mut Exception) -> *mut Object;
pub type ExecuteTasks = unsafe extern "C-unwind" fn(*const MethodInfo);

/// Only the exports needed to evaluate attachment and rooting. Loading this table calls no export.
#[derive(Clone, Copy)]
pub struct Api {
    pub domain_get: DomainGet,
    pub thread_current: ThreadCurrent,
    pub thread_attach: ThreadAttach,
    pub thread_detach: ThreadDetach,
    pub gchandle_new: GcHandleNew,
    pub gchandle_free: GcHandleFree,
    pub gchandle_get_target: GcHandleTarget,
}

/// A required export was absent. Partial tables are never published.
#[derive(Debug, PartialEq, Eq)]
pub struct MissingExport(pub &'static CStr);

impl std::fmt::Display for MissingExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing IL2CPP export: {}", self.0.to_string_lossy())
    }
}

impl std::error::Error for MissingExport {}

impl Api {
    /// Resolve a complete table without attaching a thread or invoking game code.
    ///
    /// # Safety
    /// Each returned address must identify the named function with exactly the declared ABI,
    /// including pointer-sized GC handles. Its code must remain loaded for every use of the table.
    /// This function cannot validate those properties by inspecting the address.
    pub unsafe fn resolve(mut symbol: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, MissingExport> {
        macro_rules! load {
            ($name:expr, $ty:ty) => {{
                let address = symbol($name).ok_or(MissingExport($name))?;
                // SAFETY: the caller establishes symbol identity, ABI and code lifetime.
                unsafe { std::mem::transmute::<*mut c_void, $ty>(address.as_ptr()) }
            }};
        }
        Ok(Self {
            domain_get: load!(c"il2cpp_domain_get", DomainGet),
            thread_current: load!(c"il2cpp_thread_current", ThreadCurrent),
            thread_attach: load!(c"il2cpp_thread_attach", ThreadAttach),
            thread_detach: load!(c"il2cpp_thread_detach", ThreadDetach),
            gchandle_new: load!(c"il2cpp_gchandle_new", GcHandleNew),
            gchandle_free: load!(c"il2cpp_gchandle_free", GcHandleFree),
            gchandle_get_target: load!(c"il2cpp_gchandle_get_target", GcHandleTarget),
        })
    }
}
