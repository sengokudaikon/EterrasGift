//! Rooted runtime-initialization-thread scalar candidates. This grants no Unity main-thread authority.

use super::{AttachedThread, RuntimeError, StrongRoot, raw};
use std::{
    ffi::{CStr, c_char, c_void},
    num::{NonZeroI32, NonZeroU32},
    ptr::NonNull,
};

type Class = *mut raw::Class;
type Image = *const c_void;
type Field = *mut raw::FieldInfo;
type Type = *const raw::Type;

/// Windows DWORD candidate from the runtime-initialization thread; no OS-thread lifetime is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeInitializationWindowsId(NonZeroU32);
impl RuntimeInitializationWindowsId {
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

/// Managed Int32 identifier candidate, in a separate domain from Windows thread IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeInitializationManagedId(NonZeroI32);
impl RuntimeInitializationManagedId {
    pub fn get(self) -> i32 {
        self.0.get()
    }
}

/// Owned scalar observations only. Runtime initialization need not occur on Unity's engine main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeInitializationThreadCandidate {
    windows: RuntimeInitializationWindowsId,
    managed: RuntimeInitializationManagedId,
}
impl RuntimeInitializationThreadCandidate {
    pub fn windows_thread_id(self) -> RuntimeInitializationWindowsId {
        self.windows
    }
    pub fn managed_thread_id(self) -> RuntimeInitializationManagedId {
        self.managed
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeThreadReadError {
    UnsupportedPanicStrategy,
    NotReady(&'static str),
    Contract(&'static str),
    Runtime(RuntimeError),
    RootTargetUnavailable,
    WindowsIdOutOfRange(i64),
    ManagedIdOutOfRange(i32),
}
impl std::fmt::Display for RuntimeThreadReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPanicStrategy => f.write_str("Runtime thread reads require panic=unwind"),
            Self::NotReady(reason) => write!(f, "Runtime thread candidate unavailable: {reason}"),
            Self::Contract(reason) => write!(f, "Unsupported runtime thread metadata: {reason}"),
            Self::Runtime(error) => error.fmt(f),
            Self::RootTargetUnavailable => f.write_str("Runtime thread root has no target"),
            Self::WindowsIdOutOfRange(value) => write!(f, "Runtime thread Windows ID is outside positive DWORD: {value}"),
            Self::ManagedIdOutOfRange(value) => write!(f, "Runtime thread managed ID is not positive: {value}"),
        }
    }
}
impl std::error::Error for RuntimeThreadReadError {}
impl From<RuntimeError> for RuntimeThreadReadError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}
type Result<T> = std::result::Result<T, RuntimeThreadReadError>;

macro_rules! exports {
    ($($field:ident: $name:literal => $signature:ty),+ $(,)?) => {
        /// Narrow named export table, separate from attachment/rooting and game discovery.
        #[derive(Clone, Copy)]
        pub struct RuntimeThreadApi { $($field: $signature),+ }
        impl RuntimeThreadApi {
            /// Resolve addresses only; no runtime export is invoked here.
            ///
            /// # Safety
            /// Every address must be the named Unity6000.4 modern IL2CPP export with this
            /// exact ABI, including mono_thread_get_main's managed Thread return. All code
            /// must remain executable/resident and belong to the same live runtime as each
            /// later read's attachment and pointer-valued GC handles. Address identity,
            /// executable ownership and residency are external obligations, not proved here.
            pub unsafe fn resolve(mut resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> std::result::Result<Self, raw::MissingExport> {
                Ok(Self { $($field: {
                    let name = CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).expect("static export name");
                    let address = resolver(name).ok_or(raw::MissingExport(name))?;
                    // SAFETY: the caller establishes this exact named function's ABI and code lifetime.
                    unsafe { std::mem::transmute::<*mut c_void, $signature>(address.as_ptr()) }
                }),+ })
            }
        }
    };
}

exports! {
    main: "mono_thread_get_main" => unsafe extern "C-unwind" fn() -> *mut raw::Object,
    corlib: "il2cpp_get_corlib" => raw::GetCorlib,
    class_from_name: "il2cpp_class_from_name" => raw::ClassFromName,
    class_image: "il2cpp_class_get_image" => unsafe extern "C-unwind" fn(Class) -> Image,
    class_parent: "il2cpp_class_get_parent" => unsafe extern "C-unwind" fn(Class) -> Class,
    class_flags: "il2cpp_class_get_flags" => unsafe extern "C-unwind" fn(Class) -> i32,
    class_value: "il2cpp_class_is_valuetype" => raw::ClassIsValueType,
    class_enum: "il2cpp_class_is_enum" => raw::ClassIsEnum,
    class_generic: "il2cpp_class_is_generic" => raw::ClassIsGeneric,
    class_inflated: "il2cpp_class_is_inflated" => raw::ClassIsInflated,
    class_inited: "il2cpp_class_is_inited" => unsafe extern "C-unwind" fn(Class) -> bool,
    object_class: "il2cpp_object_get_class" => raw::ObjectGetClass,
    class_field: "il2cpp_class_get_field_from_name" => unsafe extern "C-unwind" fn(Class, *const c_char) -> Field,
    field_parent: "il2cpp_field_get_parent" => unsafe extern "C-unwind" fn(Field) -> Class,
    field_flags: "il2cpp_field_get_flags" => unsafe extern "C-unwind" fn(Field) -> i32,
    field_type: "il2cpp_field_get_type" => unsafe extern "C-unwind" fn(Field) -> Type,
    field_offset: "il2cpp_field_get_offset" => unsafe extern "C-unwind" fn(Field) -> usize,
    type_code: "il2cpp_type_get_type" => raw::TypeGetType,
    type_byref: "il2cpp_type_is_byref" => raw::TypeIsByref,
    type_class: "il2cpp_class_from_type" => raw::ClassFromType,
    field_get: "il2cpp_field_get_value" => unsafe extern "C-unwind" fn(*mut raw::Object, Field, *mut c_void),
}

fn target(root: &StrongRoot<'_, '_>) -> Result<NonNull<raw::Object>> {
    root.target().ok_or(RuntimeThreadReadError::RootTargetUnavailable)
}

impl RuntimeThreadApi {
    /// Copy the runtime-initialization thread's candidate Windows and managed IDs through two local roots.
    ///
    /// # Safety
    /// `attached` and this table must address the SAME live initialized modern runtime. The
    /// caller must independently establish publication of the runtime-main Thread/InternalThread
    /// graph and these fields, valid immutable metadata and Windows DWORD thread_id semantics.
    /// GC must be nonmoving; newly returned objects must stay live through immediate root creation.
    /// Exclude graph/ID mutation, runtime reinitialization/shutdown, detach, handle invalidation
    /// and reentrancy throughout this call and root cleanup. Rooting does not establish those
    /// exclusions, native-thread liveness or permission to invoke Unity APIs.
    ///
    /// Admit cold metadata/layout setup, locks/caches, framework/profiler effects and GC-handle
    /// allocation/free. Layout readiness does not prove cctor or runtime publication. No reread
    /// is a synchronization or ABA proof. Use panic=unwind and an outer compatible foreign
    /// exception boundary, including cleanup; Rust panic must remain a terminal failure and
    /// must not be mistaken for a caught C++ exception. This reader installs no panic hook,
    /// catches no unwind, and provides no SEH/native-fault recovery. A returned scalar is a
    /// candidate only: no Unity-main equivalence, OS handle/process lifetime or authority follows.
    pub unsafe fn read_candidate(&self, attached: &AttachedThread<'_>) -> Result<RuntimeInitializationThreadCandidate> {
        if cfg!(not(panic = "unwind")) {
            return Err(RuntimeThreadReadError::UnsupportedPanicStrategy);
        }
        // SAFETY: caller supplies the exact published graph and same-runtime lease; both returned
        // references are rooted immediately before any further runtime call, under nonmoving GC.
        unsafe {
            let main = NonNull::new((self.main)()).ok_or(RuntimeThreadReadError::NotReady("runtime main"))?;
            let main = attached.root(main)?;
            let core = (self.corlib)();
            if core.is_null() {
                return Err(RuntimeThreadReadError::NotReady("corlib"));
            }
            let object = self.class(core, c"System", c"Object", false)?;
            let critical = self.class(core, c"System.Runtime.ConstrainedExecution", c"CriticalFinalizerObject", false)?;
            let value = self.class(core, c"System", c"ValueType", false)?;
            let thread = self.class(core, c"System.Threading", c"Thread", false)?;
            let internal = self.class(core, c"System.Threading", c"InternalThread", false)?;
            let i64_class = self.class(core, c"System", c"Int64", true)?;
            let i32_class = self.class(core, c"System", c"Int32", true)?;
            if !(self.class_parent)(object).is_null()
                || (self.class_parent)(critical) != object
                || (self.class_parent)(value) != object
                || (self.class_parent)(thread) != critical
                || (self.class_parent)(internal) != critical
                || (self.class_parent)(i64_class) != value
                || (self.class_parent)(i32_class) != value
            {
                return Err(RuntimeThreadReadError::Contract("core class parent"));
            }
            for class in [thread, internal, i64_class, i32_class] {
                // Exact significant shape: sequential, sealed, noninterface and nonabstract.
                if (self.class_flags)(class) & 0x1b8 != 0x108 {
                    return Err(RuntimeThreadReadError::Contract("sequential sealed class shape"));
                }
            }
            if (self.object_class)(target(&main)?.as_ptr()) != thread {
                return Err(RuntimeThreadReadError::Contract("exact Thread object"));
            }
            let child_field = self.field(thread, c"internal_thread", internal, 0x12, 1)?;
            let id_field = self.field(internal, c"thread_id", i64_class, 0x0a, 3)?;
            let managed_field = self.field(internal, c"managed_id", i32_class, 0x08, 3)?;
            let mut child = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target(&main)?.as_ptr(), child_field, std::ptr::from_mut(&mut child).cast());
            let child = NonNull::new(child).ok_or(RuntimeThreadReadError::NotReady("internal thread"))?;
            let child = attached.root(child)?;
            if (self.object_class)(target(&child)?.as_ptr()) != internal {
                return Err(RuntimeThreadReadError::Contract("exact InternalThread object"));
            }
            let mut windows = 0_i64;
            let mut managed = 0_i32;
            (self.field_get)(target(&child)?.as_ptr(), id_field, std::ptr::from_mut(&mut windows).cast());
            (self.field_get)(target(&child)?.as_ptr(), managed_field, std::ptr::from_mut(&mut managed).cast());
            let windows = u32::try_from(windows)
                .ok()
                .and_then(NonZeroU32::new)
                .ok_or(RuntimeThreadReadError::WindowsIdOutOfRange(windows))?;
            let managed = NonZeroI32::new(managed)
                .filter(|value| value.get() > 0)
                .ok_or(RuntimeThreadReadError::ManagedIdOutOfRange(managed))?;
            Ok(RuntimeInitializationThreadCandidate {
                windows: RuntimeInitializationWindowsId(windows),
                managed: RuntimeInitializationManagedId(managed),
            })
        }
    }

    unsafe fn class(&self, image: Image, namespace: &CStr, name: &CStr, value: bool) -> Result<Class> {
        // SAFETY: called only beneath read_candidate's immutable live metadata contract.
        unsafe {
            let class = (self.class_from_name)(image, namespace.as_ptr(), name.as_ptr());
            if class.is_null()
                || (self.class_image)(class) != image
                || (self.class_value)(class) != value
                || (self.class_enum)(class)
                || (self.class_generic)(class)
                || (self.class_inflated)(class)
            {
                return Err(RuntimeThreadReadError::Contract("exact closed core class"));
            }
            Ok(class)
        }
    }

    unsafe fn field(&self, owner: Class, name: &CStr, class: Class, code: i32, flags: i32) -> Result<Field> {
        // SAFETY: caller validated the exact owner/type classes under read_candidate's contract.
        unsafe {
            let field = (self.class_field)(owner, name.as_ptr());
            if field.is_null() {
                return Err(RuntimeThreadReadError::Contract("named instance field"));
            }
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) != flags
                || (self.type_byref)(ty)
                || (self.type_code)(ty) != code
                || (self.type_class)(ty) != class
            {
                return Err(RuntimeThreadReadError::Contract("exact instance field storage/type"));
            }
            // The export may prepare metadata layout. The value is never used for address arithmetic.
            let offset = (self.field_offset)(field);
            if offset > isize::MAX as usize || !(self.class_inited)(owner) {
                return Err(RuntimeThreadReadError::Contract("instance field layout readiness/offset"));
            }
            Ok(field)
        }
    }
}
