//! Experimental declarations for the modern pointer-valued GC-handle ABI.
//!
//! These are runtime exports, not generated method bodies or Unity internal calls. No runtime
//! structure layout is assumed. Historical uint32 GC handles require a different adapter.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;

macro_rules! opaque {
    ($($name:ident),+ $(,)?) => { $(
        #[doc = "Opaque runtime-owned type. Never construct or dereference it in Rust."]
        #[repr(C)]
        pub struct $name { _private: [u8; 0] }
    )+ };
}

opaque!(Domain, Thread, Object, MethodInfo, Exception, Class, Type, FieldInfo);

pub type FieldSetValue = unsafe extern "C-unwind" fn(*mut Object, *mut FieldInfo, *mut c_void);

/// The mutation export is deliberately separate from read-only runtime access.
#[derive(Clone, Copy)]
pub struct FieldWriteApi {
    pub field_set_value: FieldSetValue,
}

impl FieldWriteApi {
    /// Resolve the exact Unity 6000.4 export without performing a write.
    ///
    /// # Safety
    /// The resolver must return this named export with its exact ABI, from the same live
    /// runtime as every object/field passed to it. Code must remain loaded for all uses.
    pub unsafe fn resolve(mut symbol: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, MissingExport> {
        let name = c"il2cpp_field_set_value";
        let address = symbol(name).ok_or(MissingExport(name))?;
        Ok(Self {
            // SAFETY: the caller establishes the named export's ABI and lifetime.
            field_set_value: unsafe { std::mem::transmute::<*mut c_void, FieldSetValue>(address.as_ptr()) },
        })
    }

    /// Store one exact Int32 instance field. This grants no general managed-memory access.
    ///
    /// # Safety
    /// The caller must be attached to this runtime on the authorized engine thread, hold a
    /// strong root for the live object, and exclude shutdown, graph mutation and reentrancy.
    /// `field` must belong to the object's validated exact class and describe a mutable,
    /// nonstatic, nonliteral, nonreadonly, nonbyref core System.Int32 (IL2CPP_TYPE_I4) field.
    /// Its metadata and storage must be valid, and this particular mutation must be authorized.
    /// A void export provides no failure/SEH recovery guarantee; the write is attempted at entry.
    pub unsafe fn set_i32(&self, object: NonNull<Object>, field: NonNull<FieldInfo>, mut value: i32) {
        // SAFETY: this per-call contract proves the exact destination and four-byte source width.
        unsafe { (self.field_set_value)(object.as_ptr(), field.as_ptr(), (&mut value as *mut i32).cast()) };
    }
}

pub type DomainGet = unsafe extern "C-unwind" fn() -> *mut Domain;
pub type ThreadCurrent = unsafe extern "C-unwind" fn() -> *mut Thread;
pub type ThreadAttach = unsafe extern "C-unwind" fn(*mut Domain) -> *mut Thread;
pub type ThreadDetach = unsafe extern "C-unwind" fn(*mut Thread);
pub type GcHandleNew = unsafe extern "C-unwind" fn(*mut Object, bool) -> *mut c_void;
pub type GcHandleFree = unsafe extern "C-unwind" fn(*mut c_void);
pub type GcHandleTarget = unsafe extern "C-unwind" fn(*mut c_void) -> *mut Object;
pub type RuntimeInvoke = unsafe extern "C-unwind" fn(*const MethodInfo, *mut c_void, *mut *mut c_void, *mut *mut Exception) -> *mut Object;
pub type ExecuteTasks = unsafe extern "C-unwind" fn(*const MethodInfo);
pub type MethodGetReturnType = unsafe extern "C-unwind" fn(*const MethodInfo) -> *const Type;
pub type MethodGetClass = unsafe extern "C-unwind" fn(*const MethodInfo) -> *mut Class;
pub type MethodIsGeneric = unsafe extern "C-unwind" fn(*const MethodInfo) -> bool;
pub type MethodIsInflated = unsafe extern "C-unwind" fn(*const MethodInfo) -> bool;
pub type MethodIsInstance = unsafe extern "C-unwind" fn(*const MethodInfo) -> bool;
pub type MethodGetParamCount = unsafe extern "C-unwind" fn(*const MethodInfo) -> u32;
pub type MethodGetParam = unsafe extern "C-unwind" fn(*const MethodInfo, u32) -> *const Type;
pub type MethodGetFlags = unsafe extern "C-unwind" fn(*const MethodInfo, *mut u32) -> u32;
pub type TypeGetType = unsafe extern "C-unwind" fn(*const Type) -> i32;
pub type TypeIsByref = unsafe extern "C-unwind" fn(*const Type) -> bool;
pub type ClassFromType = unsafe extern "C-unwind" fn(*const Type) -> *mut Class;
pub type ClassIsValueType = unsafe extern "C-unwind" fn(*const Class) -> bool;
pub type ClassIsGeneric = unsafe extern "C-unwind" fn(*const Class) -> bool;
pub type ClassIsInflated = unsafe extern "C-unwind" fn(*const Class) -> bool;
pub type ClassIsAssignableFrom = unsafe extern "C-unwind" fn(*mut Class, *mut Class) -> bool;
pub type ObjectGetClass = unsafe extern "C-unwind" fn(*mut Object) -> *mut Class;
pub type ObjectUnbox = unsafe extern "C-unwind" fn(*mut Object) -> *mut c_void;
pub type ObjectGetVirtualMethod = unsafe extern "C-unwind" fn(*mut Object, *const MethodInfo) -> *const MethodInfo;
pub type GetCorlib = unsafe extern "C-unwind" fn() -> *const c_void;
pub type ClassFromName = unsafe extern "C-unwind" fn(*const c_void, *const c_char, *const c_char) -> *mut Class;
pub type ClassIsEnum = unsafe extern "C-unwind" fn(*const Class) -> bool;
pub type ClassEnumBasetype = unsafe extern "C-unwind" fn(*mut Class) -> *const Type;
pub type ClassValueSize = unsafe extern "C-unwind" fn(*mut Class, *mut u32) -> i32;

/// Invocation/metadata exports from the Unity 6000.4 public header. Separate from attachment APIs.
#[derive(Clone, Copy)]
pub struct InvocationApi {
    pub runtime_invoke: RuntimeInvoke,
    pub method_get_return_type: MethodGetReturnType,
    pub method_get_class: MethodGetClass,
    pub method_is_generic: MethodIsGeneric,
    pub method_is_inflated: MethodIsInflated,
    pub method_is_instance: MethodIsInstance,
    pub method_get_param_count: MethodGetParamCount,
    pub method_get_param: MethodGetParam,
    pub method_get_flags: MethodGetFlags,
    pub type_get_type: TypeGetType,
    pub type_is_byref: TypeIsByref,
    pub class_from_type: ClassFromType,
    pub class_is_valuetype: ClassIsValueType,
    pub class_is_generic: ClassIsGeneric,
    pub class_is_inflated: ClassIsInflated,
    pub class_is_assignable_from: ClassIsAssignableFrom,
    pub object_get_class: ObjectGetClass,
    pub object_unbox: ObjectUnbox,
    pub object_get_virtual_method: ObjectGetVirtualMethod,
    pub get_corlib: GetCorlib,
    pub class_from_name: ClassFromName,
    pub class_is_enum: ClassIsEnum,
    pub class_enum_basetype: ClassEnumBasetype,
    pub class_value_size: ClassValueSize,
}

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

impl InvocationApi {
    /// Resolve invocation exports without calling any of them.
    ///
    /// # Safety
    /// Every address must implement the named Unity 6000.4 public runtime API with this exact ABI.
    /// Code must remain loaded throughout all uses. Resolution proves neither runtime identity
    /// nor permission/safety to execute a particular managed method.
    pub unsafe fn resolve(mut symbol: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, MissingExport> {
        macro_rules! load {
            ($name:expr, $ty:ty) => {{
                let address = symbol($name).ok_or(MissingExport($name))?;
                // SAFETY: caller establishes the named export's ABI and code lifetime.
                unsafe { std::mem::transmute::<*mut c_void, $ty>(address.as_ptr()) }
            }};
        }
        Ok(Self {
            runtime_invoke: load!(c"il2cpp_runtime_invoke", RuntimeInvoke),
            method_get_return_type: load!(c"il2cpp_method_get_return_type", MethodGetReturnType),
            method_get_class: load!(c"il2cpp_method_get_class", MethodGetClass),
            method_is_generic: load!(c"il2cpp_method_is_generic", MethodIsGeneric),
            method_is_inflated: load!(c"il2cpp_method_is_inflated", MethodIsInflated),
            method_is_instance: load!(c"il2cpp_method_is_instance", MethodIsInstance),
            method_get_param_count: load!(c"il2cpp_method_get_param_count", MethodGetParamCount),
            method_get_param: load!(c"il2cpp_method_get_param", MethodGetParam),
            method_get_flags: load!(c"il2cpp_method_get_flags", MethodGetFlags),
            type_get_type: load!(c"il2cpp_type_get_type", TypeGetType),
            type_is_byref: load!(c"il2cpp_type_is_byref", TypeIsByref),
            class_from_type: load!(c"il2cpp_class_from_type", ClassFromType),
            class_is_valuetype: load!(c"il2cpp_class_is_valuetype", ClassIsValueType),
            class_is_generic: load!(c"il2cpp_class_is_generic", ClassIsGeneric),
            class_is_inflated: load!(c"il2cpp_class_is_inflated", ClassIsInflated),
            class_is_assignable_from: load!(c"il2cpp_class_is_assignable_from", ClassIsAssignableFrom),
            object_get_class: load!(c"il2cpp_object_get_class", ObjectGetClass),
            object_unbox: load!(c"il2cpp_object_unbox", ObjectUnbox),
            object_get_virtual_method: load!(c"il2cpp_object_get_virtual_method", ObjectGetVirtualMethod),
            get_corlib: load!(c"il2cpp_get_corlib", GetCorlib),
            class_from_name: load!(c"il2cpp_class_from_name", ClassFromName),
            class_is_enum: load!(c"il2cpp_class_is_enum", ClassIsEnum),
            class_enum_basetype: load!(c"il2cpp_class_enum_basetype", ClassEnumBasetype),
            class_value_size: load!(c"il2cpp_class_value_size", ClassValueSize),
        })
    }
}

/// Exact UTF16 string allocator, resolved only for operations that authorize allocation.
#[derive(Clone, Copy)]
pub struct StringFactoryApi {
    pub string_new_utf16: unsafe extern "C-unwind" fn(*const u16, i32) -> *mut Object,
}
impl StringFactoryApi {
    /// Resolve without invoking the allocator.
    ///
    /// # Safety
    /// The address must be the exact Unity6000.4 public export with this ABI, from the same
    /// initialized runtime as subsequent calls, and remain loaded for every use.
    pub unsafe fn resolve(mut symbol: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, MissingExport> {
        let name = c"il2cpp_string_new_utf16";
        let address = symbol(name).ok_or(MissingExport(name))?;
        Ok(Self {
            // SAFETY: caller establishes export identity, ABI and code lifetime.
            string_new_utf16: unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C-unwind" fn(*const u16, i32) -> *mut Object>(address.as_ptr()) },
        })
    }
}
