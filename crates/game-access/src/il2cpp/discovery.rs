//! Read-only metadata discovery and the narrow main-thread status query.
//!
//! The public API resolves fields instead of carrying game-version offsets. The two small
//! layout adapters (modern x64 object/array/string headers and MethodInfo.methodPointer) are
//! explicitly part of the unsafe runtime contract, rather than inferred from a game version.
//! Export signatures follow the installed 6000.4.8f1 `il2cpp-api-functions.h`; field and method
//! contracts follow the original app's `native/runtime-discovery.h` and `native/dispatch.h`.

use super::{AttachedThread, RuntimeError, raw};
use crate::protocol::{Discovery, DispatcherSpec, RuntimeStatus};
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, c_char, c_void},
    ptr::NonNull,
};

mod catalog;
mod catalog_ailment;
mod catalog_items;
mod catalog_numeric;
mod forge;
mod limits;
mod locale;

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

macro_rules! exports {
    ($($field:ident: $name:literal => $signature:ty),+ $(,)?) => {
        /// Public runtime exports. Resolution alone invokes neither managed bodies nor class initializers.
        #[derive(Clone, Copy)]
        pub struct RuntimeAccess { $($field: $signature),+ }
        impl RuntimeAccess {
            #[cfg(test)]
            fn test_stubs() -> Self { Self { $($field: <$signature as TestExportStub>::test_stub()),+ } }
            /// Resolve the complete metadata/status export table.
            ///
            /// # Safety
            /// Every address must be the named Unity 6000.4 public export with the declared ABI.
            /// Runtime code and metadata must remain loaded for every subsequent use. GC handles
            /// use the modern pointer-valued ABI. The resolver must not substitute other functions.
            pub unsafe fn resolve(mut resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, DiscoveryError> {
                Ok(Self { $($field: {
                    let name = CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).expect("static symbol");
                    let address = resolver(name).ok_or(DiscoveryError::MissingExport(raw::MissingExport(name)))?;
                    // SAFETY: the caller establishes export identity, ABI and lifetime.
                    unsafe { std::mem::transmute::<*mut c_void, $signature>(address.as_ptr()) }
                }),+ })
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
    corlib: "il2cpp_get_corlib" => unsafe extern "C-unwind" fn() -> Ptr,
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
                let image = (self.assembly_image)(assemblies.add(index).read());
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

    unsafe fn class(&self, images: &Images, namespace: &CStr, name: &CStr) -> Result<Class, DiscoveryError> {
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
                        if !actual.is_null() && CStr::from_ptr(actual) == name {
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

    unsafe fn field(&self, mut class: Class, name: &CStr) -> Result<Ptr, DiscoveryError> {
        unsafe {
            for _ in 0..256 {
                if class.is_null() {
                    return Err(unsupported(format!("missing field {}", name.to_string_lossy())));
                }
                let field = (self.class_field)(class, name.as_ptr());
                if !field.is_null() {
                    return Ok(field);
                }
                class = (self.class_parent)(class);
            }
            Err(unsupported("class inheritance cycle/depth"))
        }
    }

    unsafe fn type_name(&self, ty: Type) -> Result<String, DiscoveryError> {
        unsafe {
            if ty.is_null() {
                return Err(unsupported("null type"));
            }
            let name = (self.type_name)(ty);
            if name.is_null() {
                return Err(unsupported("type name unavailable"));
            }
            let result = CStr::from_ptr(name).to_str().map(str::to_owned).map_err(|_| unsupported("non-UTF8 type name"));
            (self.runtime_free)(name.cast());
            result
        }
    }

    unsafe fn static_field(&self, class: Class, name: &CStr, expected: &str) -> Result<Ptr, DiscoveryError> {
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

    unsafe fn method(&self, class: Class, name: &CStr, parameters: &[&str], instance: bool) -> Result<Method, DiscoveryError> {
        unsafe {
            let mut iterator = std::ptr::null_mut();
            let mut found = None;
            for _ in 0..100_000 {
                let method = (self.class_methods)(class, &mut iterator);
                if method.is_null() {
                    return found.ok_or_else(|| unsupported(format!("missing method {}", name.to_string_lossy())));
                }
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("null method name"));
                }
                if CStr::from_ptr(actual) != name || (self.method_params)(method) as usize != parameters.len() {
                    continue;
                }
                if found.is_some()
                    || (self.method_instance)(method) != instance
                    || (self.method_generic)(method)
                    || (self.method_inflated)(method)
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
            Err(unsupported("method enumeration limit"))
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
        unsafe {
            if build_id.len() != 64 || !build_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(unsupported("helper build ID"));
            }
            let images = self.images()?;
            let manager = self.class(&images, c"", c"ItemContainersManager")?;
            self.static_field(self.class(&images, c"", c"PlayerFinder")?, c"localItemContainersManager", "ItemContainersManager")?;
            self.static_field(self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?, c"_isOnlinePlay", "System.Boolean")?;
            self.static_field(self.class(&images, c"LE.AssetManagement", c"GlobalAssets")?, c"_storage_MasterAffixesList", "AffixList")?;
            let mut offsets = BTreeMap::new();
            for (key, class, name, expected) in REQUIRED_FIELDS {
                let class = CString::new(*class).expect("static class");
                let name = CString::new(*name).expect("static field");
                let field = self.field(self.class(&images, c"", &class)?, &name)?;
                let ty = (self.field_type)(field);
                let actual = self.type_name(ty)?;
                if (self.field_flags)(field) & 0x10 != 0 || !field_type_matches(&actual, expected, name == c"affixes") {
                    return Err(unsupported(format!("field {key}: {actual}")));
                }
                let class = (self.type_class)(ty);
                if !class.is_null() && (self.class_enum)(class) && self.type_name((self.enum_base)(class))? != "System.Int32" {
                    return Err(unsupported(format!("enum storage {key}")));
                }
                let offset = (self.field_offset)(field);
                if !(16..=16384).contains(&offset) {
                    return Err(unsupported(format!("field position {key}")));
                }
                offsets.insert((*key).to_owned(), offset as u64);
            }
            let item = self.class(&images, c"", c"ItemData")?;
            let list = (self.type_class)((self.field_type)(self.field(item, c"affixes")?));
            if list.is_null() {
                return Err(unsupported("affixes list class"));
            }
            for (name, offset) in [(c"_items", 16), (c"_size", 24), (c"_version", 28)] {
                if (self.field_offset)(self.field(list, name)?) != offset {
                    return Err(unsupported("List adapter"));
                }
            }
            let cached = self.field(manager, c"m_CachedPtr")?;
            if (self.field_offset)(cached) != 16 || self.type_name((self.field_type)(cached))? != "System.IntPtr" {
                return Err(unsupported("Unity object adapter"));
            }
            self.check_allocated_adapters(attached)?;
            self.method(item, c"RebuildID", &[], true)?;
            self.method(self.class(&images, c"", c"ItemDataUnpacked")?, c"AfterIDChange", &[], true)?;
            self.method(self.class(&images, c"", c"ItemContainerEntry")?, c"UpdateEntry", &[], true)?;
            self.method(
                self.class(&images, c"", c"ItemAffix")?,
                c".ctor",
                &["System.UInt16", "System.Byte", "System.Byte", "System.Byte", "SealedAffixType"],
                true,
            )?;
            let dispatcher = self.method(self.class(&images, c"UnityEngine", c"UnitySynchronizationContext")?, c"ExecuteTasks", &[], false)?;
            let address = dispatcher.cast::<*mut u8>().read();
            let base = module_base.as_ptr() as usize;
            let rva = (address as usize)
                .checked_sub(base)
                .filter(|offset| offset.checked_add(16).is_some_and(|end| end <= module_size))
                .ok_or_else(|| unsupported("dispatcher module range"))?;
            let mut bytes = [0; 16];
            std::ptr::copy_nonoverlapping(address, bytes.as_mut_ptr(), bytes.len());
            Ok(DiscoveredRuntime {
                discovery: Discovery {
                    build_id,
                    dispatcher: DispatcherSpec { rva: rva as u64, bytes },
                    offsets,
                },
                dispatcher: NonNull::new(address).ok_or_else(|| unsupported("dispatcher address"))?,
            })
        }
    }

    unsafe fn check_allocated_adapters(&self, attached: &AttachedThread<'_>) -> Result<(), DiscoveryError> {
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
                || bytes.cast::<u8>().add(24).cast::<u64>().read() != 2
                || (self.string_length)(text) != 2
                || text.cast::<u8>().add(16).cast::<i32>().read() != 2
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
                if !manager.as_ptr().cast::<u8>().add(16).cast::<Ptr>().read().is_null() {
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

fn field_type_matches(actual: &str, expected: &str, affixes: bool) -> bool {
    if expected.contains("List<") {
        actual.contains("System.Collections.Generic.List") && (!affixes || actual.contains("ItemAffix"))
    } else {
        actual == expected
            || ['/', '+', '.']
                .iter()
                .any(|separator| actual.strip_suffix(expected).is_some_and(|prefix| prefix.ends_with(*separator)))
    }
}

// Keep order aligned with protocol::REQUIRED_KEYS; field names/types come from runtime-discovery.h.
const REQUIRED_FIELDS: &[(&str, &str, &str, &str)] = &[
    ("ItemContainersManager.CraftingOffset", "ItemContainersManager", "crafting", "CraftingContainers"),
    ("CraftingContainers.MainOffset", "CraftingContainers", "main", "OneItemContainer"),
    ("OneSlotItemContainer.ContentOffset", "OneSlotItemContainer", "content", "ItemContainerEntry"),
    ("ItemContainerEntry.DataOffset", "ItemContainerEntry", "data", "ItemData"),
    ("ItemDataUnpacked.SerializationOffset", "ItemData", "id", "System.Byte[]"),
    ("ItemDataUnpacked.IndividualIdOffset", "ItemData", "individualID", "System.UInt32"),
    ("ItemDataUnpacked.ItemTypeOffset", "ItemData", "itemType", "System.Byte"),
    ("ItemDataUnpacked.SubTypeOffset", "ItemData", "subType", "System.UInt16"),
    ("ItemDataUnpacked.RarityOffset", "ItemData", "rarity", "System.Byte"),
    ("ItemDataUnpacked.UniqueIdOffset", "ItemData", "uniqueID", "System.UInt16"),
    ("ItemDataUnpacked.ImplicitRollsOffset", "ItemData", "implicitRolls", "System.Byte[]"),
    ("ItemDataUnpacked.UniqueRollsOffset", "ItemData", "uniqueRolls", "System.Byte[]"),
    ("ItemDataUnpacked.AffixesOffset", "ItemData", "affixes", "List<ItemAffix>"),
    ("ItemDataUnpacked.ClassRequirementOffset", "ItemDataUnpacked", "classReq", "ClassRequirement"),
    ("ItemDataUnpacked.FullNameOffset", "ItemDataUnpacked", "_fullName", "System.String"),
    ("ItemDataUnpacked.SealedRegularOffset", "ItemData", "hasSealedRegularAffix", "System.Boolean"),
    ("ItemDataUnpacked.SealedPrimordialOffset", "ItemData", "hasSealedPrimordialAffix", "System.Boolean"),
    ("ItemDataUnpacked.SealedCorruptionOffset", "ItemData", "hasSealedAffixFromCorruption", "System.Boolean"),
    ("ItemDataUnpacked.ForgingPotentialOffset", "ItemData", "forgingPotential", "System.Byte"),
    ("ItemDataUnpacked.ForgingPotentialTypeOffset", "ItemData", "forgingPotentialType", "ForgingPotentialType"),
    ("ItemDataUnpacked.LegendaryPotentialOffset", "ItemData", "legendaryPotential", "System.Byte"),
    ("ItemDataUnpacked.WeaversWillOffset", "ItemData", "weaversWill", "System.Byte"),
    ("ItemDataUnpacked.WeaversTouchOffset", "ItemData", "weaversTouch", "System.Byte"),
    ("ItemDataUnpacked.CorruptedOffset", "ItemData", "corrupted", "System.Boolean"),
    ("ItemDataUnpacked.RuinedOffset", "ItemData", "ruined", "System.Boolean"),
    ("ItemDataUnpacked.LevelRequirementOffset", "ItemDataUnpacked", "<LvlReq>k__BackingField", "System.Int32"),
    ("ItemAffix.IdOffset", "ItemAffix", "affixId", "System.UInt16"),
    ("ItemAffix.TierOffset", "ItemAffix", "affixTier", "System.Byte"),
    ("ItemAffix.RollOffset", "ItemAffix", "affixRoll", "System.Byte"),
    ("ItemAffix.TypeOffset", "ItemAffix", "affixType", "AffixType"),
    ("ItemAffix.SpecialAffixTypeOffset", "ItemAffix", "specialAffixType", "SpecialAffixType"),
    ("ItemAffix.NameOffset", "ItemAffix", "affixName", "System.String"),
    ("ItemAffix.SealOffset", "ItemAffix", "sealedAffixType", "SealedAffixType"),
    ("AffixList.SingleAffixesOffset", "AffixList", "singleAffixes", "SingleAffix[]"),
    ("AffixList.MultiAffixesOffset", "AffixList", "multiAffixes", "MultiAffix[]"),
    ("AffixDefinition.IdOffset", "Affix", "affixId", "System.Int32"),
    ("AffixDefinition.TiersOffset", "Affix", "tiers", "List<"),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_contract_covers_every_protocol_field_exactly_once() {
        let keys: Vec<_> = REQUIRED_FIELDS.iter().map(|entry| entry.0).collect();
        assert_eq!(keys, crate::protocol::REQUIRED_KEYS);
    }
    #[test]
    fn type_contract_accepts_nested_names_and_checks_affix_element() {
        assert!(field_type_matches("ItemContainersManager/CraftingContainers", "CraftingContainers", false));
        assert!(field_type_matches("AffixList+Affix", "Affix", false));
        assert!(!field_type_matches("FakeCraftingContainers", "CraftingContainers", false));
        assert!(field_type_matches("System.Collections.Generic.List<ItemAffix>", "List<ItemAffix>", true));
        assert!(!field_type_matches("System.Collections.Generic.List<Other>", "List<ItemAffix>", true));
        assert!(!field_type_matches("List<ItemAffix>", "List<ItemAffix>", true));
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
