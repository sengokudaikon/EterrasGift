//! Read-only metadata discovery and the narrow main-thread status query.
//!
//! The public API resolves fields instead of carrying game-version offsets. The two small
//! layout adapters (modern x64 object/array/string headers and MethodInfo.methodPointer) are
//! explicitly part of the unsafe runtime contract, rather than inferred from a game version.
//! Export signatures follow the installed 6000.4.8f1 `il2cpp-api-functions.h`; field and method
//! contracts follow the original app's `native/runtime-discovery.h` and `native/dispatch.h`.

// Entry-point contracts cover the shared runtime assumptions. Keep local notes
// for bounds, ownership and ABI details rather than repeating those contracts.

use super::{AttachedThread, RuntimeError, raw};
use eg_trainer_wire::protocol::{Discovery, DispatcherSpec, RuntimeStatus};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr::NonNull,
};

mod attack_leech;
mod descriptions;
mod dispatcher_thread;
mod unique;
pub use attack_leech::AttackLeechWriteError;
mod catalog;
mod forge;
mod limits;
mod locale;
mod mapping;
mod write;
pub use write::ForgeWriteError;

type Ptr = *mut c_void;
type Class = *mut raw::Class;
type Method = *const raw::MethodInfo;
type Type = *const raw::Type;

/// A missing contract fails discovery as a whole; no partial layout is published.
#[derive(Debug)]
pub enum DiscoveryError {
    MissingExport(raw::MissingExport),
    Runtime(RuntimeError),
    NotReady(&'static str),
    OfflineRequired,
    Contract(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingExport(error) => error.fmt(f),
            Self::Runtime(error) => error.fmt(f),
            Self::NotReady(reason) => write!(f, "IL2CPP runtime is not ready: {reason}"),
            Self::OfflineRequired => f.write_str("An offline character with a live item manager is required"),
            Self::Contract(contract) => write!(f, "Unsupported IL2CPP runtime contract: {contract}"),
        }
    }
}
impl std::error::Error for DiscoveryError {}
impl From<RuntimeError> for DiscoveryError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

fn unsupported(contract: impl Into<String>) -> DiscoveryError {
    DiscoveryError::Contract(contract.into())
}

// Class and field descriptors belong to immutable metadata in the retained runtime.
// Only descriptor lookup is cached; object values and native liveness are always read afresh.
#[derive(Default)]
struct MetadataCache {
    fields: std::sync::Mutex<std::collections::HashMap<(usize, Vec<u8>), usize>>,
}

/// # Safety
/// The pointer is readable for one initialized T. T's representation matches the
/// metadata-validated field/adapter; its owner remains rooted or is pinned code/metadata.
unsafe fn read_value<T: Copy>(pointer: *const T) -> T {
    // SAFETY: the caller supplies the readable span and exact valid representation;
    // unaligned reads also cover generated instruction bytes and packed metadata.
    unsafe { pointer.read_unaligned() }
}
/// # Safety
/// The pointer addresses `count` initialized, aligned Ts in one readable allocation.
/// The owner remains rooted and unchanged until this synchronous owned copy completes.
unsafe fn copy_values<T: Copy>(pointer: *const T, count: usize) -> Vec<T> {
    if count == 0 {
        return Vec::new();
    }
    // SAFETY: caller validates storage type, bounded length and rooted source lifetime.
    unsafe { std::slice::from_raw_parts(pointer, count) }.to_vec()
}
/// # Safety
/// The nonnull pointer addresses a NUL-terminated runtime metadata name or an allocated
/// type name. Its storage outlives the returned borrow (before runtime_free for type names).
unsafe fn metadata_text<'a>(pointer: *const c_char) -> &'a CStr {
    // SAFETY: caller supplies the metadata string's extent and lifetime above.
    unsafe { CStr::from_ptr(pointer) }
}

/// # Safety
/// The destination is a private rooted byte array with at least source.len()
/// writable bytes. It does not overlap source and contains no managed references.
unsafe fn write_bytes(destination: *mut u8, source: &[u8]) {
    if source.is_empty() {
        return;
    }
    // SAFETY: caller validates the private byte span and excludes overlapping storage.
    unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), destination, source.len()) };
}

macro_rules! exports {
    ($($field:ident: $name:literal => $signature:ty),+ $(,)?) => {
        /// Public runtime exports. Resolution alone invokes neither managed bodies nor class initializers.
        #[derive(Clone, Copy)]
        pub struct RuntimeAccess { $($field: $signature),+, metadata: Option<&'static MetadataCache> }
        impl RuntimeAccess {
            #[cfg(test)]
            fn test_stubs() -> Self { Self { $($field: <$signature as TestExportStub>::test_stub()),+, metadata: None } }
            /// Resolve the complete metadata/status export table.
            ///
            /// # Safety
            /// Every address must be the named Unity 6000.4 public export with the declared ABI.
            /// Runtime code and immutable metadata must remain loaded for every subsequent use. GC handles
            /// use the modern pointer-valued ABI. The resolver must not substitute other functions.
            pub unsafe fn resolve(mut resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, DiscoveryError> {
                Ok(Self { $($field: {
                    let name = CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).expect("static symbol");
                    let address = resolver(name).ok_or(DiscoveryError::MissingExport(raw::MissingExport(name)))?;
                    // SAFETY: the caller establishes export identity, ABI and lifetime.
                    unsafe { std::mem::transmute::<*mut c_void, $signature>(address.as_ptr()) }
                }),+, metadata: Some(Box::leak(Box::new(MetadataCache::default()))) })
            }
        }
    };
}

// Every test stub has its field's actual ABI/signature and fails if an unexpected export is
// reached. Unlike casting one catch-all address, this does not manufacture mismatched functions.
#[cfg(test)]
trait TestExportStub {
    fn test_stub() -> Self;
}
#[cfg(test)]
macro_rules! test_export_stubs {
    ($($argument:ident),*) => {
        impl<$($argument,)* R> TestExportStub for unsafe extern "C-unwind" fn($($argument),*) -> R {
            fn test_stub() -> Self {
                unsafe extern "C-unwind" fn unexpected<$($argument,)* R>($(_: $argument),*) -> R { panic!("unexpected metadata export"); }
                unexpected::<$($argument,)* R>
            }
        }
    };
}
#[cfg(test)]
test_export_stubs!();
#[cfg(test)]
test_export_stubs!(A);
#[cfg(test)]
test_export_stubs!(A, B);
#[cfg(test)]
test_export_stubs!(A, B, C);

exports! {
    domain_get: "il2cpp_domain_get" => raw::DomainGet,
    assemblies: "il2cpp_domain_get_assemblies" => unsafe extern "C-unwind" fn(*const raw::Domain, *mut usize) -> *const Ptr,
    assembly_image: "il2cpp_assembly_get_image" => unsafe extern "C-unwind" fn(Ptr) -> Ptr,
    class_from_name: "il2cpp_class_from_name" => unsafe extern "C-unwind" fn(Ptr, *const c_char, *const c_char) -> Class,
    class_parent: "il2cpp_class_get_parent" => unsafe extern "C-unwind" fn(Class) -> Class,
    class_nested: "il2cpp_class_get_nested_types" => unsafe extern "C-unwind" fn(Class, *mut Ptr) -> Class,
    class_name: "il2cpp_class_get_name" => unsafe extern "C-unwind" fn(Class) -> *const c_char,
    class_namespace: "il2cpp_class_get_namespace" => unsafe extern "C-unwind" fn(Class) -> *const c_char,
    class_image: "il2cpp_class_get_image" => unsafe extern "C-unwind" fn(Class) -> Ptr,
    class_field: "il2cpp_class_get_field_from_name" => unsafe extern "C-unwind" fn(Class, *const c_char) -> Ptr,
    field_type: "il2cpp_field_get_type" => unsafe extern "C-unwind" fn(Ptr) -> Type,
    field_parent: "il2cpp_field_get_parent" => unsafe extern "C-unwind" fn(Ptr) -> Class,
    field_flags: "il2cpp_field_get_flags" => unsafe extern "C-unwind" fn(Ptr) -> i32,
    field_offset: "il2cpp_field_get_offset" => unsafe extern "C-unwind" fn(Ptr) -> usize,
    static_get: "il2cpp_field_static_get_value" => unsafe extern "C-unwind" fn(Ptr, Ptr),
    field_get: "il2cpp_field_get_value" => unsafe extern "C-unwind" fn(*mut raw::Object, Ptr, Ptr),
    field_get_object: "il2cpp_field_get_value_object" => unsafe extern "C-unwind" fn(Ptr, *mut raw::Object) -> *mut raw::Object,
    type_name: "il2cpp_type_get_name" => unsafe extern "C-unwind" fn(Type) -> *mut c_char,
    runtime_free: "il2cpp_free" => unsafe extern "C-unwind" fn(Ptr),
    type_class: "il2cpp_class_from_il2cpp_type" => raw::ClassFromType,
    type_code: "il2cpp_type_get_type" => raw::TypeGetType,
    type_byref: "il2cpp_type_is_byref" => raw::TypeIsByref,
    class_type: "il2cpp_class_get_type" => unsafe extern "C-unwind" fn(Class) -> Type,
    class_element: "il2cpp_class_get_element_class" => unsafe extern "C-unwind" fn(Class) -> Class,
    class_rank: "il2cpp_class_get_rank" => unsafe extern "C-unwind" fn(*const raw::Class) -> i32,
    class_value: "il2cpp_class_is_valuetype" => raw::ClassIsValueType,
    class_enum: "il2cpp_class_is_enum" => unsafe extern "C-unwind" fn(*const raw::Class) -> bool,
    enum_base: "il2cpp_class_enum_basetype" => unsafe extern "C-unwind" fn(Class) -> Type,
    class_methods: "il2cpp_class_get_methods" => unsafe extern "C-unwind" fn(Class, *mut Ptr) -> Method,
    method_name: "il2cpp_method_get_name" => unsafe extern "C-unwind" fn(Method) -> *const c_char,
    method_params: "il2cpp_method_get_param_count" => raw::MethodGetParamCount,
    method_param: "il2cpp_method_get_param" => raw::MethodGetParam,
    method_return: "il2cpp_method_get_return_type" => raw::MethodGetReturnType,
    method_instance: "il2cpp_method_is_instance" => raw::MethodIsInstance,
    method_generic: "il2cpp_method_is_generic" => raw::MethodIsGeneric,
    method_inflated: "il2cpp_method_is_inflated" => raw::MethodIsInflated,
    method_flags: "il2cpp_method_get_flags" => raw::MethodGetFlags,
    corlib: "il2cpp_get_corlib" => unsafe extern "C-unwind" fn() -> Ptr,
    object_new: "il2cpp_object_new" => unsafe extern "C-unwind" fn(Class) -> *mut raw::Object,
    array_new: "il2cpp_array_new" => unsafe extern "C-unwind" fn(Class, usize) -> *mut raw::Object,
    array_length: "il2cpp_array_length" => unsafe extern "C-unwind" fn(*mut raw::Object) -> u32,
    string_new: "il2cpp_string_new" => unsafe extern "C-unwind" fn(*const c_char) -> *mut raw::Object,
    string_length: "il2cpp_string_length" => unsafe extern "C-unwind" fn(*mut raw::Object) -> i32,
    string_chars: "il2cpp_string_chars" => unsafe extern "C-unwind" fn(*mut raw::Object) -> *mut u16,
    object_header_size: "il2cpp_object_header_size" => unsafe extern "C-unwind" fn() -> u32,
    array_header_size: "il2cpp_array_object_header_size" => unsafe extern "C-unwind" fn() -> u32,
    array_length_offset: "il2cpp_offset_of_array_length_in_array_object_header" => unsafe extern "C-unwind" fn() -> u32,
    array_bounds_offset: "il2cpp_offset_of_array_bounds_in_array_object_header" => unsafe extern "C-unwind" fn() -> u32,
    object_class: "il2cpp_object_get_class" => raw::ObjectGetClass,
    assignable: "il2cpp_class_is_assignable_from" => raw::ClassIsAssignableFrom,
}

/// Discovery data plus a typed address retained only while the caller's runtime contract holds.
pub struct DiscoveredRuntime {
    pub discovery: Discovery,
    pub dispatcher: NonNull<u8>,
}

struct Images {
    all: Vec<Ptr>,
    game: Ptr,
}

impl RuntimeAccess {
    // SAFETY for internal helpers: called only beneath discover/status's live attached-runtime contract.
    unsafe fn images(&self) -> Result<Images, DiscoveryError> {
        // SAFETY: The attached runtime retains its domain/assembly array; its bounded count precedes copying live image descriptors.
        unsafe {
            let domain = (self.domain_get)();
            if domain.is_null() {
                return Err(DiscoveryError::NotReady("domain unavailable"));
            }
            let mut count = 0;
            let assemblies = (self.assemblies)(domain, &mut count);
            if assemblies.is_null() || !(1..=4096).contains(&count) {
                return Err(unsupported("assembly table"));
            }
            let mut all = Vec::with_capacity(count);
            let mut game: Ptr = std::ptr::null_mut();
            let mut player_finder: Class = std::ptr::null_mut();
            for index in 0..count {
                let image = (self.assembly_image)(read_value(assemblies.add(index)));
                if image.is_null() {
                    return Err(unsupported("assembly image"));
                }
                let candidate = (self.class_from_name)(image, c"".as_ptr(), c"PlayerFinder".as_ptr());
                if !candidate.is_null() {
                    record_class(&mut player_finder, candidate, c"PlayerFinder")?;
                    if game.is_null() {
                        game = image;
                    }
                }
                all.push(image);
            }
            if game.is_null() {
                return Err(DiscoveryError::NotReady("PlayerFinder assembly missing"));
            }
            Ok(Images { all, game })
        }
    }

    // SAFETY: Images and class descriptors are retained immutable metadata; names and nested-class traversal are bounded and checked.
    unsafe fn class(&self, images: &Images, namespace: &CStr, name: &CStr) -> Result<Class, DiscoveryError> {
        // SAFETY: Image and class descriptors belong to retained immutable metadata; bounded lookup never invokes a game method.
        unsafe {
            let first = (self.class_from_name)(images.game, namespace.as_ptr(), name.as_ptr());
            if !first.is_null() {
                return Ok(first);
            }
            let parent = if name == c"CraftingContainers" {
                Some(c"ItemContainersManager")
            } else if name == c"Affix" {
                Some(c"AffixList")
            } else {
                None
            };
            if let Some(parent) = parent {
                let parent = (self.class_from_name)(images.game, c"".as_ptr(), parent.as_ptr());
                if !parent.is_null() {
                    let mut iterator = std::ptr::null_mut();
                    for _ in 0..4096 {
                        let nested = (self.class_nested)(parent, &mut iterator);
                        if nested.is_null() {
                            break;
                        }
                        let actual = (self.class_name)(nested);
                        if !actual.is_null() && metadata_text(actual) == name {
                            return Ok(nested);
                        }
                    }
                }
            }
            let mut found: Class = std::ptr::null_mut();
            for image in &images.all {
                let class = (self.class_from_name)(*image, namespace.as_ptr(), name.as_ptr());
                record_class(&mut found, class, name)?;
            }
            if found.is_null() {
                Err(unsupported(format!("missing class {}", name.to_string_lossy())))
            } else {
                Ok(found)
            }
        }
    }

    /// Enumerate descriptors from one live class with a shared bound.
    unsafe fn methods(&self, class: Class) -> Result<Vec<Method>, DiscoveryError> {
        let mut iterator = std::ptr::null_mut();
        let mut methods = Vec::new();
        for _ in 0..100_000 {
            // SAFETY: internal callers supply a class descriptor from these exports'
            // retained immutable metadata; iterator is a local slot managed by IL2CPP.
            let method = unsafe { (self.class_methods)(class, &mut iterator) };
            if method.is_null() {
                return Ok(methods);
            }
            methods.push(method);
        }
        Err(unsupported("method enumeration limit"))
    }

    unsafe fn field(&self, mut class: Class, name: &CStr) -> Result<Ptr, DiscoveryError> {
        let key = (class as usize, name.to_bytes().to_vec());
        if let Some(cache) = self.metadata
            && let Some(field) = cache.fields.lock().unwrap_or_else(|p| p.into_inner()).get(&key)
        {
            return Ok(*field as Ptr);
        }
        // SAFETY: class comes from this runtime's metadata; parent and field descriptors
        // remain live and immutable under the operation's retained-runtime contract.
        unsafe {
            for _ in 0..256 {
                if class.is_null() {
                    return Err(unsupported(format!("missing field {}", name.to_string_lossy())));
                }
                let field = (self.class_field)(class, name.as_ptr());
                if !field.is_null() {
                    if let Some(cache) = self.metadata {
                        cache.fields.lock().unwrap_or_else(|p| p.into_inner()).insert(key, field as usize);
                    }
                    return Ok(field);
                }
                class = (self.class_parent)(class);
            }
            Err(unsupported("class inheritance cycle/depth"))
        }
    }

    unsafe fn type_name(&self, ty: Type) -> Result<String, DiscoveryError> {
        // SAFETY: The live type descriptor returns allocated NUL-terminated text; it is copied before RuntimeFree releases that allocation.
        unsafe {
            if ty.is_null() {
                return Err(unsupported("null type"));
            }
            let name = (self.type_name)(ty);
            if name.is_null() {
                return Err(unsupported("type name unavailable"));
            }
            let result = metadata_text(name).to_str().map(str::to_owned).map_err(|_| unsupported("non-UTF8 type name"));
            (self.runtime_free)(name.cast());
            result
        }
    }

    unsafe fn static_field(&self, class: Class, name: &CStr, expected: &str) -> Result<Ptr, DiscoveryError> {
        // SAFETY: The live field descriptor is checked for static nonliteral storage and exact type before being used by a getter.
        unsafe {
            let field = self.field(class, name)?;
            // Literal and thread-static fields are not the process-wide live state this adapter
            // accepts. A metadata name/type match alone would not establish that distinction.
            if (self.field_flags)(field) & 0x50 != 0x10 || (self.field_offset)(field) == usize::MAX || self.type_name((self.field_type)(field))? != expected {
                return Err(unsupported(format!("static field {}", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    pub(super) unsafe fn zero_arg_method(
        &self,
        owner: Class,
        name: &CStr,
        returns: Class,
        instance: bool,
        api: &raw::InvocationApi,
    ) -> Result<NonNull<raw::MethodInfo>, DiscoveryError> {
        // SAFETY: The owner and return class are retained metadata; exact arity and storage are checked before binding.
        unsafe {
            let mut found = None;
            for method in self.methods(owner)? {
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("zero-argument getter name"));
                }
                if metadata_text(actual) != name {
                    continue;
                }
                let ty = (api.method_get_return_type)(method);
                let mut implementation_flags = 0;
                if found.is_some()
                    || (api.method_get_param_count)(method) != 0
                    || (api.method_is_instance)(method) != instance
                    || (api.method_get_class)(method) != owner
                    || ty.is_null()
                    || (api.type_is_byref)(ty)
                    || (api.class_from_type)(ty) != returns
                    || (api.method_get_flags)(method, &mut implementation_flags) & 0x0440 != 0
                {
                    return Err(unsupported(format!("zero-argument getter contract: {}", name.to_string_lossy())));
                }
                found = NonNull::new(method.cast_mut());
            }
            found.ok_or_else(|| unsupported(format!("zero-argument getter missing: {}", name.to_string_lossy())))
        }
    }

    unsafe fn method(&self, class: Class, name: &CStr, parameters: &[&str], instance: bool) -> Result<Method, DiscoveryError> {
        // SAFETY: The class is retained metadata; exact parameter names, instance flags and nonvirtual Void return are checked before its code is selected.
        unsafe {
            let mut found = None;
            for method in self.methods(class)? {
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("null method name"));
                }
                if metadata_text(actual) != name || (self.method_params)(method) as usize != parameters.len() {
                    continue;
                }
                let mut implementation_flags = 0;
                if found.is_some()
                    || (self.method_instance)(method) != instance
                    || (self.method_generic)(method)
                    || (self.method_inflated)(method)
                    || (self.method_flags)(method, &mut implementation_flags) & 0x2440 != 0
                    || self.type_name((self.method_return)(method))? != "System.Void"
                {
                    return Err(unsupported(format!("method contract {}", name.to_string_lossy())));
                }
                for (index, expected) in parameters.iter().enumerate() {
                    if self.type_name((self.method_param)(method, index as u32))? != *expected {
                        return Err(unsupported(format!("method parameter {}", name.to_string_lossy())));
                    }
                }
                found = Some(method);
            }
            found.ok_or_else(|| unsupported(format!("missing method {}", name.to_string_lossy())))
        }
    }

    /// Validate the legacy control discovery contract without invoking game methods.
    ///
    /// # Safety
    /// `attached` must belong to these exports' live runtime on this OS thread. Runtime metadata
    /// must be immutable and assembly loading/shutdown excluded for the operation. The runtime
    /// must use the Unity 6000.4.8 x64 nonmoving-GC layouts: MethodInfo.methodPointer at offset 0,
    /// object header 16, array header 32, string length/chars at 16/20. The module range must be
    /// the readable mapped GameAssembly image containing generated method bodies. Class metadata
    /// and the allocation APIs must be valid; public header queries and allocated adapter samples
    /// are cross-checks, not permission to dereference arbitrary or untrusted memory.
    pub unsafe fn discover(&self, attached: &AttachedThread<'_>, build_id: String, module_base: NonNull<u8>, module_size: usize) -> Result<DiscoveredRuntime, DiscoveryError> {
        // SAFETY: The caller retains the mapped module and immutable metadata; the dispatcher RVA is bounded before its instruction prefix is copied.
        unsafe {
            if build_id.len() != 64 || !build_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(unsupported("helper build ID"));
            }
            let images = self.images()?;
            self.check_allocated_adapters(attached)?;
            let dispatcher = self.method(self.class(&images, c"UnityEngine", c"UnitySynchronizationContext")?, c"ExecuteTasks", &[], false)?;
            let address = read_value(dispatcher.cast::<*mut u8>());
            let base = module_base.as_ptr() as usize;
            let rva = (address as usize)
                .checked_sub(base)
                .filter(|offset| offset.checked_add(16).is_some_and(|end| end <= module_size))
                .ok_or_else(|| unsupported("dispatcher module range"))?;
            let mut bytes = [0; 16];
            bytes.copy_from_slice(&read_value::<[u8; 16]>(address.cast()));
            Ok(DiscoveredRuntime {
                discovery: Discovery {
                    build_id,
                    dispatcher: DispatcherSpec { rva: rva as u64, bytes },
                },
                dispatcher: NonNull::new(address).ok_or_else(|| unsupported("dispatcher address"))?,
            })
        }
    }

    // SAFETY: Fresh arrays and strings are rooted; their public lengths and addresses are cross-checked before reading supported header slots.
    unsafe fn check_allocated_adapters(&self, attached: &AttachedThread<'_>) -> Result<(), DiscoveryError> {
        // SAFETY: Fresh arrays and strings stay rooted while public header APIs and bounded raw header slots are compared.
        unsafe {
            if (self.object_header_size)() != 16 || (self.array_header_size)() != 32 || (self.array_length_offset)() != 24 || (self.array_bounds_offset)() != 16 {
                return Err(unsupported("object/array header API"));
            }
            let corlib = (self.corlib)();
            if corlib.is_null() {
                return Err(unsupported("corlib image"));
            }
            let byte_class = (self.class_from_name)(corlib, c"System".as_ptr(), c"Byte".as_ptr());
            if byte_class.is_null() {
                return Err(unsupported("System.Byte"));
            }
            let bytes = NonNull::new((self.array_new)(byte_class, 2)).ok_or_else(|| unsupported("byte array allocation"))?;
            let bytes = attached.root(bytes)?;
            let text = NonNull::new((self.string_new)(c"EF".as_ptr())).ok_or_else(|| unsupported("string allocation"))?;
            let text = attached.root(text)?;
            let bytes = bytes.target().ok_or_else(|| unsupported("byte array root"))?.as_ptr();
            let text = text.target().ok_or_else(|| unsupported("string root"))?.as_ptr();
            if (self.array_length)(bytes) != 2
                || read_value(bytes.cast::<u8>().add(24).cast::<u64>()) != 2
                || (self.string_length)(text) != 2
                || read_value(text.cast::<u8>().add(16).cast::<i32>()) != 2
                || (self.string_chars)(text).cast::<u8>() != text.cast::<u8>().add(20)
            {
                return Err(unsupported("allocated array/string adapter"));
            }
            Ok(())
        }
    }

    /// Read the online flag and the current managed manager wrapper, without running game methods.
    /// The manager address is an ephemeral identity token; it grants no dereference permission.
    ///
    /// # Safety
    /// All `discover` runtime/attachment/layout conditions apply. Call only on Unity's main thread,
    /// after its original ExecuteTasks call has returned. The game's static fields/native-object
    /// lifetime must not race this query; no other thread may destroy the wrapper during rooting.
    /// No process-global initialized state is changed by this query. Static fields must already be
    /// initialized, as required by the admitted game session. No feature hooks may be active in this
    /// control-only helper (the returned active feature count is therefore exactly zero).
    pub unsafe fn status(&self, attached: &AttachedThread<'_>) -> Result<RuntimeStatus, DiscoveryError> {
        // SAFETY: The caller supplies the immutable main-thread graph span; static references are rooted and native pointer fields are validated before copying.
        unsafe {
            let images = self.images()?;
            let online_field = self.static_field(self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?, c"_isOnlinePlay", "System.Boolean")?;
            let mut online = 1_u8;
            (self.static_get)(online_field, (&mut online as *mut u8).cast());
            if online > 1 {
                return Err(unsupported("online Boolean representation"));
            }
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let field = self.static_field(self.class(&images, c"", c"PlayerFinder")?, c"localItemContainersManager", "ItemContainersManager")?;
            let mut manager = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(field, (&mut manager as *mut *mut raw::Object).cast());
            let manager_root = NonNull::new(manager).map(|pointer| attached.root(pointer)).transpose()?;
            let mut manager_address = 0;
            if let Some(root) = &manager_root {
                let manager = root.target().ok_or_else(|| unsupported("manager root target"))?;
                self.check_object(manager_class, manager)?;
                let cached = self.field(manager_class, c"m_CachedPtr")?;
                if (self.field_flags)(cached) & 0x10 != 0 || (self.field_offset)(cached) != 16 || self.type_name((self.field_type)(cached))? != "System.IntPtr" {
                    return Err(unsupported("manager native pointer"));
                }
                // The managed wrapper is strongly rooted; this reads only its pointer-sized field,
                // and never dereferences the native object or assumes a root preserves native life.
                if !read_value(manager.as_ptr().cast::<u8>().add(16).cast::<Ptr>()).is_null() {
                    manager_address = manager.as_ptr() as u64;
                }
            }
            let master_class = self.class(&images, c"", c"AffixList")?;
            let field = self.static_field(self.class(&images, c"LE.AssetManagement", c"GlobalAssets")?, c"_storage_MasterAffixesList", "AffixList")?;
            let mut master = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(field, (&mut master as *mut *mut raw::Object).cast());
            let master_root = NonNull::new(master).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = master_root {
                self.check_object(master_class, root.target().ok_or_else(|| unsupported("master root target"))?)?;
            }
            Ok(RuntimeStatus {
                online: online != 0,
                manager: manager_address,
                features_on: 0,
            })
        }
    }

    unsafe fn check_object(&self, expected: Class, object: NonNull<raw::Object>) -> Result<(), DiscoveryError> {
        // SAFETY: The rooted object and expected class descriptor remain live in this retained runtime while assignability is queried.
        unsafe {
            let actual = (self.object_class)(object.as_ptr());
            if actual.is_null() || !(self.assignable)(expected, actual) {
                return Err(unsupported("static object class"));
            }
            Ok(())
        }
    }
}

// Image::ClassFromName includes exported/forwarded types in its name table. Multiple images
// may therefore resolve to one canonical class. Only distinct class identities are ambiguous.
fn record_class(found: &mut Class, candidate: Class, name: &CStr) -> Result<(), DiscoveryError> {
    if !candidate.is_null() {
        if !found.is_null() && *found != candidate {
            return Err(unsupported(format!("ambiguous class {}", name.to_string_lossy())));
        }
        *found = candidate;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    #[test]
    fn field_descriptors_are_resolved_once_per_runtime_class_and_name() {
        thread_local! { static LOOKUPS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
        unsafe extern "C-unwind" fn lookup(_: Class, _: *const c_char) -> Ptr {
            LOOKUPS.with(|count| count.set(count.get() + 1));
            NonNull::<u8>::dangling().as_ptr().cast()
        }
        let mut access = RuntimeAccess::test_stubs();
        access.class_field = lookup;
        access.metadata = Some(Box::leak(Box::new(MetadataCache::default())));
        let (mut first, mut second) = (0_u8, 0_u8);
        let first = (&raw mut first).cast();
        let second = (&raw mut second).cast();
        for _ in 0..100 {
            // SAFETY: the fixture export treats classes as opaque keys, never dereferences
            // them, and returns the same retained descriptor for each requested name.
            assert!(!unsafe { access.field(first, c"value") }.unwrap().is_null());
        }
        // SAFETY: same fixture descriptor contract, with distinct class/name keys.
        unsafe {
            access.field(first, c"other").unwrap();
            access.field(second, c"value").unwrap();
        }
        LOOKUPS.with(|count| assert_eq!(count.get(), 3));
    }

    #[test]
    fn direct_void_method_refuses_virtual_abstract_and_native_replacements() {
        unsafe extern "C-unwind" fn methods(owner: Class, iterator: *mut Ptr) -> Method {
            // SAFETY: method() supplies a live iterator slot; owner points to test flags.
            unsafe {
                if (*iterator).is_null() {
                    *iterator = owner.cast();
                    owner.cast()
                } else {
                    std::ptr::null()
                }
            }
        }
        unsafe extern "C-unwind" fn name(_: Method) -> *const c_char {
            c"RebuildID".as_ptr()
        }
        unsafe extern "C-unwind" fn count(_: Method) -> u32 {
            0
        }
        unsafe extern "C-unwind" fn yes(_: Method) -> bool {
            true
        }
        unsafe extern "C-unwind" fn no(_: Method) -> bool {
            false
        }
        unsafe extern "C-unwind" fn flags(method: Method, implementation: *mut u32) -> u32 {
            // SAFETY: test method is a live u32; implementation is a writable caller slot.
            unsafe {
                *implementation = 0;
                method.cast::<u32>().read()
            }
        }
        unsafe extern "C-unwind" fn returns(_: Method) -> Type {
            NonNull::dangling().as_ptr()
        }
        unsafe extern "C-unwind" fn type_name(_: Type) -> *mut c_char {
            CString::new("System.Void").unwrap().into_raw()
        }
        unsafe extern "C-unwind" fn free(value: Ptr) {
            // SAFETY: allocation comes from type_name and is released once.
            unsafe {
                drop(CString::from_raw(value.cast()));
            }
        }
        let mut access = RuntimeAccess::test_stubs();
        access.class_methods = methods;
        access.method_name = name;
        access.method_params = count;
        access.method_instance = yes;
        access.method_generic = no;
        access.method_inflated = no;
        access.method_flags = flags;
        access.method_return = returns;
        access.type_name = type_name;
        access.runtime_free = free;
        for mut flags in [0_u32, 0x40, 0x400, 0x2000] {
            // SAFETY: all metadata queries are exact fake exports over this local flags token.
            let result = unsafe { access.method((&mut flags as *mut u32).cast(), c"RebuildID", &[], true) };
            assert_eq!(result.is_ok(), flags == 0);
        }
    }
    #[test]
    fn resolution_fails_before_invoking_any_missing_export() {
        // SAFETY: no address is returned, so no ABI/lifetime promises are exercised.
        let result = unsafe { RuntimeAccess::resolve(|_| None) };
        assert!(matches!(result, Err(DiscoveryError::MissingExport(raw::MissingExport(name))) if name == c"il2cpp_domain_get"));
    }
    #[test]
    fn forwarded_class_identity_is_unique_across_images() {
        let mut storage = [0_u8; 2];
        // These pointers are identity tokens only. The test and record_class never dereference them.
        let class = storage.as_mut_ptr().cast::<raw::Class>();
        let other = storage[1..].as_mut_ptr().cast::<raw::Class>();
        let mut found = std::ptr::null_mut();
        record_class(&mut found, std::ptr::null_mut(), c"Forwarded").unwrap();
        record_class(&mut found, class, c"Forwarded").unwrap();
        record_class(&mut found, class, c"Forwarded").unwrap();
        record_class(&mut found, std::ptr::null_mut(), c"Forwarded").unwrap();
        assert_eq!(found, class);
        assert!(matches!(record_class(&mut found, other, c"Forwarded"), Err(DiscoveryError::Contract(message)) if message == "ambiguous class Forwarded"));
        assert_eq!(found, class);
    }
}
