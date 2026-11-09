//! Metadata lookups over the runtime's public exports: assemblies and their images, classes, fields and methods, each
//! checked against the contract its caller states, plus the two layout adapters the queries rely on (modern x64
//! object/array/string headers, and `MethodInfo.methodPointer` at offset 0). Nothing here invokes a managed method.
//! Export signatures follow Unity 6000.4.8f1's `il2cpp-api-functions.h`.
//!
//! The lookup contract, which every unsafe lookup here states as its own: the caller is attached to the live runtime
//! the table was resolved from, its metadata stays loaded and immutable for the call (assembly loading and shutdown
//! excluded), and every image, class, field, method or type passed in came from that runtime.

// Entry-point contracts cover the shared runtime assumptions. Keep local notes
// for bounds, ownership and ABI details rather than repeating those contracts.

use crate::{AttachedThread, RuntimeError, StrongRoot, raw};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr::NonNull,
};

mod collections;
mod contract;
mod methods;
mod read;

pub use contract::MethodSpec;
pub use methods::{MAX_CLASS_DEPTH, MethodError, MethodQuery, Others, Params, Returns};

pub type Ptr = *mut c_void;
pub type Class = *mut raw::Class;
pub type Method = *const raw::MethodInfo;
pub type Type = *const raw::Type;

/// Why a lookup failed. A missing contract fails the lookup as a whole; nothing partial is returned.
#[derive(Debug)]
pub enum MetadataError {
    MissingExport(raw::MissingExport),
    Runtime(RuntimeError),
    NotReady(&'static str),
    Contract(String),
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingExport(error) => error.fmt(f),
            Self::Runtime(error) => error.fmt(f),
            Self::NotReady(reason) => write!(f, "IL2CPP runtime is not ready: {reason}"),
            Self::Contract(contract) => write!(f, "Unsupported IL2CPP runtime contract: {contract}"),
        }
    }
}

impl std::error::Error for MetadataError {}
impl From<RuntimeError> for MetadataError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

/// A contract the runtime does not meet.
pub fn unsupported(contract: impl Into<String>) -> MetadataError {
    MetadataError::Contract(contract.into())
}

/// The object a root holds; a root without one is a contract failure.
pub fn target(root: &StrongRoot<'_, '_>) -> Result<NonNull<raw::Object>, MetadataError> {
    root.target().ok_or_else(|| unsupported("root target"))
}

mod sealed {
    pub trait Sealed {}
}

/// A destination type with exactly one corlib primitive class of its width. Reads check a field against that class, so
/// no metadata name can select a wider write into the destination. Closed: only this crate implements it.
pub trait Primitive: sealed::Sealed + Copy + Default {
    /// The class's name in corlib's `System` namespace.
    const NAME: &'static CStr;
}

macro_rules! primitives {
    ($($ty:ty => $name:literal),+ $(,)?) => { $(
        impl sealed::Sealed for $ty {}
        impl Primitive for $ty {
            const NAME: &'static CStr = $name;
        }
    )+ };
}
primitives!(u8 => c"Byte", u16 => c"UInt16", u32 => c"UInt32", i32 => c"Int32", f32 => c"Single", usize => c"IntPtr");

// Field descriptors belong to immutable metadata in the retained runtime. Only descriptor lookup is cached; object
// values and native liveness are always read afresh.
#[derive(Default)]
struct FieldCache {
    // Keyed by class, then name: a hit borrows the caller's name and allocates nothing.
    fields: std::sync::Mutex<std::collections::HashMap<usize, std::collections::HashMap<std::ffi::CString, usize>>>,
}

/// # Safety
/// The pointer is readable for one initialized T. T's representation matches the
/// metadata-validated field/adapter; its owner remains rooted or is pinned code/metadata.
pub unsafe fn read_value<T: Copy>(pointer: *const T) -> T {
    // SAFETY: the caller supplies the readable span and exact valid representation;
    // unaligned reads also cover generated instruction bytes and packed metadata.
    unsafe { pointer.read_unaligned() }
}
/// # Safety
/// The pointer addresses `count` initialized, aligned Ts in one readable allocation.
/// The owner remains rooted and unchanged until this synchronous owned copy completes.
pub unsafe fn copy_values<T: Copy>(pointer: *const T, count: usize) -> Vec<T> {
    if count == 0 {
        return Vec::new();
    }
    // SAFETY: caller validates storage type, bounded length and rooted source lifetime.
    unsafe { std::slice::from_raw_parts(pointer, count) }.to_vec()
}
/// # Safety
/// The nonnull pointer addresses a NUL-terminated runtime metadata name or an allocated
/// type name. Its storage outlives the returned borrow (before runtime_free for type names).
pub unsafe fn metadata_text<'a>(pointer: *const c_char) -> &'a CStr {
    // SAFETY: caller supplies the metadata string's extent and lifetime above.
    unsafe { CStr::from_ptr(pointer) }
}

/// # Safety
/// The destination is a private rooted byte array with at least source.len()
/// writable bytes. It does not overlap source and contains no managed references.
pub unsafe fn write_bytes(destination: *mut u8, source: &[u8]) {
    if source.is_empty() {
        return;
    }
    // SAFETY: caller validates the private byte span and excludes overlapping storage.
    unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), destination, source.len()) };
}

/// Bytes before a one-dimensional array's elements, in the x64 layout `check_allocated_adapters` proves.
pub const ARRAY_HEADER: usize = 32;

/// Where a one-dimensional array's elements start.
///
/// # Safety
/// `array` is a live array whose runtime has passed `check_allocated_adapters`.
#[inline]
pub unsafe fn array_data(array: *mut raw::Object) -> *mut u8 {
    // SAFETY: the caller supplies an array, whose allocation extends past its header.
    unsafe { array.cast::<u8>().add(ARRAY_HEADER) }
}

/// The furthest offset accepted for a field's storage; a deliberate support bound.
pub const MAX_FIELD_OFFSET: usize = 16384;

/// Whether an instance field's offset lies inside the object: past its 16-byte header, up to [`MAX_FIELD_OFFSET`].
pub fn instance_offset(offset: usize) -> bool {
    (16..=MAX_FIELD_OFFSET).contains(&offset)
}

macro_rules! exports {
    ($($field:ident: $name:literal => $signature:ty),+ $(,)?) => {
        /// The metadata, allocation and status exports. Resolution alone invokes neither managed bodies nor class
        /// initializers. Each field is the named export; calling one is the caller's unsafe contract.
        #[derive(Clone, Copy)]
        pub struct Exports { $(pub $field: $signature),+, cache: Option<&'static FieldCache> }
        impl Exports {
            /// Every export a stub that panics when reached, and no cache: a test sets the exports it expects.
            #[cfg(any(test, feature = "test-stubs"))]
            pub fn test_stubs() -> Self { Self { $($field: <$signature as TestExportStub>::test_stub()),+, cache: None } }
            /// Resolve the complete metadata/status export table.
            ///
            /// # Safety
            /// Every address must be the named Unity 6000.4 public export with the declared ABI.
            /// Runtime code and immutable metadata must remain loaded for every subsequent use. GC handles
            /// use the modern pointer-valued ABI. The resolver must not substitute other functions.
            pub unsafe fn resolve(mut resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, MetadataError> {
                Ok(Self { $($field: {
                    let name = CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).expect("static symbol");
                    let address = resolver(name).ok_or(MetadataError::MissingExport(raw::MissingExport(name)))?;
                    // SAFETY: the caller establishes export identity, ABI and lifetime.
                    unsafe { std::mem::transmute::<*mut c_void, $signature>(address.as_ptr()) }
                }),+, cache: Some(Box::leak(Box::new(FieldCache::default()))) })
            }
        }
    };
}

// Every test stub has its field's actual ABI/signature and fails if an unexpected export is
// reached. Unlike casting one catch-all address, this does not manufacture mismatched functions.
#[cfg(any(test, feature = "test-stubs"))]
pub trait TestExportStub {
    fn test_stub() -> Self;
}
#[cfg(any(test, feature = "test-stubs"))]
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
#[cfg(any(test, feature = "test-stubs"))]
test_export_stubs!();
#[cfg(any(test, feature = "test-stubs"))]
test_export_stubs!(A);
#[cfg(any(test, feature = "test-stubs"))]
test_export_stubs!(A, B);
#[cfg(any(test, feature = "test-stubs"))]
test_export_stubs!(A, B, C);

exports! {
    domain_get: "il2cpp_domain_get" => raw::DomainGet,
    assemblies: "il2cpp_domain_get_assemblies" => unsafe extern "C-unwind" fn(*const raw::Domain, *mut usize) -> *const Ptr,
    assembly_image: "il2cpp_assembly_get_image" => unsafe extern "C-unwind" fn(Ptr) -> Ptr,
    class_from_name: "il2cpp_class_from_name" => raw::ClassFromName,
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
    class_static_data: "il2cpp_class_get_static_field_data" => unsafe extern "C-unwind" fn(Class) -> Ptr,
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
    method_class: "il2cpp_method_get_class" => raw::MethodGetClass,
    corlib: "il2cpp_get_corlib" => raw::GetCorlib,
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

impl Exports {
    /// The invocation table for this runtime: the exports this table already holds, plus the seven only invocation
    /// uses, resolved without calling any of them.
    ///
    /// # Safety
    /// As [`Self::resolve`], with `resolver` resolving from the same runtime as this table. Resolution proves no
    /// permission or safety to execute a particular managed method.
    pub unsafe fn invocation(&self, mut resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<raw::InvocationApi, raw::MissingExport> {
        macro_rules! load {
            ($name:expr, $ty:ty) => {{
                let address = resolver($name).ok_or(raw::MissingExport($name))?;
                // SAFETY: the caller establishes the named export's ABI and code lifetime.
                unsafe { std::mem::transmute::<*mut c_void, $ty>(address.as_ptr()) }
            }};
        }
        Ok(raw::InvocationApi {
            runtime_invoke: load!(c"il2cpp_runtime_invoke", raw::RuntimeInvoke),
            class_is_generic: load!(c"il2cpp_class_is_generic", raw::ClassIsGeneric),
            class_is_inflated: load!(c"il2cpp_class_is_inflated", raw::ClassIsInflated),
            object_unbox: load!(c"il2cpp_object_unbox", raw::ObjectUnbox),
            object_get_virtual_method: load!(c"il2cpp_object_get_virtual_method", raw::ObjectGetVirtualMethod),
            class_value_size: load!(c"il2cpp_class_value_size", raw::ClassValueSize),
            method_get_return_type: self.method_return,
            method_is_generic: self.method_generic,
            method_is_inflated: self.method_inflated,
            method_is_instance: self.method_instance,
            method_get_param_count: self.method_params,
            method_get_param: self.method_param,
            method_get_flags: self.method_flags,
            method_get_class: self.method_class,
            type_get_type: self.type_code,
            type_is_byref: self.type_byref,
            class_from_type: self.type_class,
            class_is_valuetype: self.class_value,
            class_is_assignable_from: self.assignable,
            object_get_class: self.object_class,
            get_corlib: self.corlib,
            class_from_name: self.class_from_name,
            class_is_enum: self.class_enum,
            class_enum_basetype: self.enum_base,
        })
    }

    /// Every loaded assembly's image, bounded and checked; a runtime without a domain yet is not ready.
    ///
    /// # Safety
    /// The caller is attached to this export table's live runtime, with assembly loading excluded for the call.
    pub unsafe fn images(&self) -> Result<Vec<Ptr>, MetadataError> {
        // SAFETY: The attached runtime retains its domain/assembly array; its bounded count precedes copying live image descriptors.
        unsafe {
            let domain = (self.domain_get)();
            if domain.is_null() {
                return Err(MetadataError::NotReady("domain unavailable"));
            }
            let mut count = 0;
            let assemblies = (self.assemblies)(domain, &mut count);
            if assemblies.is_null() || !(1..=4096).contains(&count) {
                return Err(unsupported("assembly table"));
            }
            let mut all = Vec::with_capacity(count);
            for index in 0..count {
                let image = (self.assembly_image)(read_value(assemblies.add(index)));
                if image.is_null() {
                    return Err(unsupported("assembly image"));
                }
                all.push(image);
            }
            Ok(all)
        }
    }

    /// The one class named `namespace.name` across `images`: forwarded names that resolve to the same class count once,
    /// two distinct classes are ambiguous, and none is missing.
    ///
    /// # Safety
    /// `images` are this runtime's retained image descriptors.
    pub unsafe fn class_in(&self, images: &[Ptr], namespace: &CStr, name: &CStr) -> Result<Class, MetadataError> {
        // SAFETY: Image and class descriptors belong to retained immutable metadata; lookup never invokes a game method.
        unsafe {
            let mut found: Class = std::ptr::null_mut();
            for image in images {
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

    /// The class `name` nested directly in `parent`, if it has one.
    ///
    /// # Safety
    /// `parent` is a retained class descriptor of this runtime.
    pub unsafe fn nested_class(&self, parent: Class, name: &CStr) -> Option<Class> {
        // SAFETY: The parent and its nested descriptors are retained immutable metadata; the walk is bounded.
        unsafe {
            let mut iterator = std::ptr::null_mut();
            for _ in 0..4096 {
                let nested = (self.class_nested)(parent, &mut iterator);
                if nested.is_null() {
                    return None;
                }
                let actual = (self.class_name)(nested);
                if !actual.is_null() && metadata_text(actual) == name {
                    return Some(nested);
                }
            }
            None
        }
    }

    /// Enumerate descriptors from one live class with a shared bound, one runtime query per item.
    ///
    /// # Safety
    /// The lookup contract (module docs), for as long as the iterator is used.
    pub unsafe fn methods(&self, class: Class) -> impl Iterator<Item = Result<Method, MetadataError>> + '_ {
        let mut iterator = std::ptr::null_mut();
        let mut remaining = 100_000_u32;
        let mut finished = false;
        std::iter::from_fn(move || {
            if finished {
                return None;
            }
            if remaining == 0 {
                finished = true;
                return Some(Err(unsupported("method enumeration limit")));
            }
            remaining -= 1;
            // SAFETY: internal callers supply a class descriptor from these exports'
            // retained immutable metadata; iterator is a local slot managed by IL2CPP.
            let method = unsafe { (self.class_methods)(class, &mut iterator) };
            if method.is_null() {
                finished = true;
                None
            } else {
                Some(Ok(method))
            }
        })
    }

    /// The field `name` of `class` or its nearest ancestor that has one, cached per class and name.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn field(&self, mut class: Class, name: &CStr) -> Result<Ptr, MetadataError> {
        let owner = class as usize;
        if let Some(cache) = self.cache
            && let Some(field) = cache.fields.lock().unwrap_or_else(|p| p.into_inner()).get(&owner).and_then(|fields| fields.get(name))
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
                    if let Some(cache) = self.cache {
                        cache
                            .fields
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .entry(owner)
                            .or_default()
                            .insert(name.to_owned(), field as usize);
                    }
                    return Ok(field);
                }
                class = (self.class_parent)(class);
            }
            Err(unsupported("class inheritance cycle/depth"))
        }
    }

    /// A `System.*` class from corlib.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn system_class(&self, name: &CStr) -> Result<Class, MetadataError> {
        // SAFETY: Corlib and the named class descriptor belong to retained initialized runtime metadata.
        unsafe {
            let image = (self.corlib)();
            if image.is_null() {
                return Err(unsupported("corlib image"));
            }
            let class = (self.class_from_name)(image, c"System".as_ptr(), name.as_ptr());
            if class.is_null() {
                return Err(unsupported(format!("missing class System.{}", name.to_string_lossy())));
            }
            Ok(class)
        }
    }

    /// The class of a field's non-byref type. `label` names the caller in contract errors.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn field_class(&self, field: Ptr, label: &str) -> Result<Class, MetadataError> {
        // SAFETY: The field descriptor is retained metadata; nonnull non-byref type metadata is checked before resolving its class.
        unsafe {
            let ty = (self.field_type)(field);
            if ty.is_null() || (self.type_byref)(ty) {
                return Err(unsupported(format!("{label} field type")));
            }
            let class = (self.type_class)(ty);
            if class.is_null() {
                return Err(unsupported(format!("{label} field class")));
            }
            Ok(class)
        }
    }

    /// A nonstatic field of `owner` (or an ancestor) whose non-byref type's class is exactly `expected`.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn instance_field_in(&self, owner: Class, name: &CStr, expected: Class, label: &str) -> Result<Ptr, MetadataError> {
        // SAFETY: The owner is retained metadata; expected field class and instance storage are validated before returning the descriptor.
        unsafe {
            let field = self.field(owner, name)?;
            if (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0 || self.field_class(field, label)? != expected {
                return Err(unsupported(format!("{label} field {} type/storage", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    /// A nonstatic field of a rooted object's class (or an ancestor) whose type's class is exactly `expected`.
    ///
    /// # Safety
    /// The lookup contract (module docs), with `object` rooted in this runtime.
    pub unsafe fn instance_field(&self, object: &StrongRoot<'_, '_>, name: &CStr, expected: Class) -> Result<Ptr, MetadataError> {
        // SAFETY: The receiver is rooted; its field type, instance storage and expected class are checked before returning metadata.
        unsafe {
            let class = (self.object_class)(target(object)?.as_ptr());
            if class.is_null() {
                return Err(unsupported("object class"));
            }
            self.instance_field_in(class, name, expected, "object")
        }
    }

    /// Copy one primitive instance field of a rooted object.
    ///
    /// # Safety
    /// The lookup contract (module docs), with `object` rooted in this runtime and its graph unchanged for the copy.
    pub unsafe fn primitive<T: Primitive>(&self, object: &StrongRoot<'_, '_>, name: &CStr) -> Result<T, MetadataError> {
        // SAFETY: The receiver is rooted; exact primitive class validation fixes the stack destination's width before FieldGet.
        unsafe {
            let class = self.system_class(T::NAME)?;
            let field = self.instance_field(object, name, class)?;
            let mut value = T::default();
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut T).cast());
            Ok(value)
        }
    }

    /// Copy one Boolean instance field of a rooted object; a byte other than 0 or 1 is a contract failure.
    ///
    /// # Safety
    /// As [`Self::primitive`].
    pub unsafe fn boolean(&self, object: &StrongRoot<'_, '_>, name: &CStr) -> Result<bool, MetadataError> {
        // SAFETY: The rooted receiver's exact Boolean field is copied into one byte and validated before conversion to bool.
        unsafe {
            let class = self.system_class(c"Boolean")?;
            let field = self.instance_field(object, name, class)?;
            let mut value = 0_u8;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut u8).cast());
            match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(unsupported("Boolean representation")),
            }
        }
    }

    /// Read one reference instance field of a rooted object, declared exactly as `declared`, and root what it holds.
    /// A null field is `None`; a value that is not a `declared` is a contract failure.
    ///
    /// # Safety
    /// As [`Self::primitive`], with `attached` this runtime's attachment on this thread.
    pub unsafe fn reference<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        object: &StrongRoot<'_, '_>,
        name: &CStr,
        declared: Class,
    ) -> Result<Option<StrongRoot<'a, 'r>>, MetadataError> {
        // SAFETY: The rooted receiver's reference field is validated; the copied object is rooted before checking assignability.
        unsafe {
            if declared.is_null() || (self.class_value)(declared) {
                return Err(unsupported("reference field class"));
            }
            let field = self.instance_field(object, name, declared)?;
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut *mut raw::Object).cast());
            // The parent stays rooted, its field stays unchanged, and the child is rooted before
            // any further metadata access or allocation. Graph ownership preserves it during rooting.
            let root = NonNull::new(value).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = &root {
                self.check_object(declared, target(root)?)?;
            }
            Ok(root)
        }
    }

    /// Store a non-null reference after checking root provenance and mutable reference-field metadata. The field is the
    /// one whose type's class is exactly the value's class; no conversion.
    ///
    /// # Safety
    /// The [`raw::FieldWriteApi::set_object`] runtime, thread, lifetime and exclusive mutation obligations apply.
    /// `writes` and `attached` belong to this runtime. Metadata and the rooted graph stay stable throughout validation
    /// and store.
    pub unsafe fn set_reference(
        &self,
        writes: &raw::FieldWriteApi,
        attached: &AttachedThread<'_>,
        object: &StrongRoot<'_, '_>,
        name: &CStr,
        value: &StrongRoot<'_, '_>,
    ) -> Result<(), MetadataError> {
        // SAFETY: caller establishes the runtime and graph span. The validator rejects value types, incompatible roots
        // and immutable or noninstance fields before the store.
        unsafe {
            let field = self.reference_field(attached, object, name, value)?;
            writes.set_object(target(object)?, field, target(value)?);
            Ok(())
        }
    }

    unsafe fn reference_field(
        &self,
        attached: &AttachedThread<'_>,
        object: &StrongRoot<'_, '_>,
        name: &CStr,
        value: &StrongRoot<'_, '_>,
    ) -> Result<NonNull<raw::FieldInfo>, MetadataError> {
        // SAFETY: The receiver and value are rooted; exact mutable reference storage and declaring-class compatibility are checked before returning the field.
        unsafe {
            if !object.belongs_to(attached) || !value.belongs_to(attached) {
                return Err(unsupported("reference store runtime mismatch"));
            }
            let actual = (self.object_class)(target(value)?.as_ptr());
            if actual.is_null() || (self.class_value)(actual) {
                return Err(unsupported("reference store cannot use a boxed value"));
            }
            let field = self.instance_field(object, name, actual)?;
            let ty = (self.field_type)(field);
            if (self.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_INIT_ONLY | raw::FIELD_ATTRIBUTE_LITERAL) != 0
                || !matches!(
                    (self.type_code)(ty),
                    raw::type_code::STRING | raw::type_code::CLASS | raw::type_code::ARRAY | raw::type_code::GENERICINST | raw::type_code::OBJECT | raw::type_code::SZARRAY
                )
            {
                return Err(unsupported("reference store requires a mutable reference field"));
            }
            let owner = (self.field_parent)(field);
            let receiver = (self.object_class)(target(object)?.as_ptr());
            if owner.is_null() || receiver.is_null() || !(self.assignable)(owner, receiver) {
                return Err(unsupported("reference store declaring class"));
            }
            NonNull::new(field.cast()).ok_or_else(|| unsupported("reference field absent"))
        }
    }

    /// A type's full name, copied before the runtime's allocation is freed.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn type_name(&self, ty: Type) -> Result<String, MetadataError> {
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

    /// A static, nonliteral field of `class` whose type is named `expected`: process-wide state, not a constant or a
    /// thread-static.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn static_field(&self, class: Class, name: &CStr, expected: &str) -> Result<Ptr, MetadataError> {
        // SAFETY: The live field descriptor is checked for static nonliteral storage and exact type before being used by a getter.
        unsafe {
            let field = self.field(class, name)?;
            // Literal and thread-static fields are not the process-wide live state this adapter
            // accepts. A metadata name/type match alone would not establish that distinction.
            if (self.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL) != raw::FIELD_ATTRIBUTE_STATIC
                || (self.field_offset)(field) == usize::MAX
                || self.type_name((self.field_type)(field))? != expected
            {
                return Err(unsupported(format!("static field {}", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    /// Checks the two layout adapters the queries rely on against freshly allocated, rooted samples: the public header
    /// sizes, and an array's length and a string's length and characters where the modern x64 layout puts them.
    ///
    /// # Safety
    /// `attached` belongs to this export table's live runtime on this thread.
    pub unsafe fn check_allocated_adapters(&self, attached: &AttachedThread<'_>) -> Result<(), MetadataError> {
        // SAFETY: Fresh arrays and strings stay rooted while public header APIs and bounded raw header slots are compared.
        unsafe {
            if (self.object_header_size)() != 16 || (self.array_header_size)() != ARRAY_HEADER as u32 || (self.array_length_offset)() != 24 || (self.array_bounds_offset)() != 16 {
                return Err(unsupported("object/array header API"));
            }
            let byte_class = self.system_class(c"Byte")?;
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

    /// Whether a rooted object's class is `expected` or derives from it.
    ///
    /// # Safety
    /// The object is rooted and `expected` is a retained class descriptor of this runtime.
    pub unsafe fn check_object(&self, expected: Class, object: NonNull<raw::Object>) -> Result<(), MetadataError> {
        // SAFETY: The rooted object and expected class descriptor remain live in this retained runtime while assignability is queried.
        unsafe {
            let actual = (self.object_class)(object.as_ptr());
            if actual.is_null() || !(self.assignable)(expected, actual) {
                return Err(unsupported("static object class"));
            }
            Ok(())
        }
    }

    /// That `class` is named exactly `name` in `namespace`, and is a value type if `value` says so. `label` begins any
    /// contract failure's text.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn check_class_named(&self, class: Class, namespace: &CStr, name: &CStr, value: bool, label: &str) -> Result<(), MetadataError> {
        // SAFETY: The class descriptor and its name/namespace strings belong to retained immutable metadata.
        unsafe {
            if class.is_null() {
                return Err(unsupported(format!("{label} named class")));
            }
            let actual_name = (self.class_name)(class);
            let actual_namespace = (self.class_namespace)(class);
            if actual_name.is_null()
                || actual_namespace.is_null()
                || metadata_text(actual_name) != name
                || metadata_text(actual_namespace) != namespace
                || (self.class_value)(class) != value
            {
                return Err(unsupported(format!("{label} named class identity")));
            }
            Ok(())
        }
    }

    /// That `class` declares no static constructor, so that allocating one runs no managed code. A method with no name is
    /// refused too. `label` begins any contract failure's text.
    ///
    /// # Safety
    /// The lookup contract (module docs).
    pub unsafe fn check_no_class_initializer(&self, class: Class, label: &str) -> Result<(), MetadataError> {
        // SAFETY: The retained value class's methods are inspected without executing a managed initializer.
        unsafe {
            for method in self.methods(class) {
                let method = method?;
                let name = (self.method_name)(method);
                if name.is_null() || metadata_text(name) == c".cctor" {
                    return Err(unsupported(format!("{label} boxing class initializer")));
                }
            }
            Ok(())
        }
    }
}

// Image::ClassFromName includes exported/forwarded types in its name table. Multiple images
// may therefore resolve to one canonical class. Only distinct class identities are ambiguous.
pub fn record_class(found: &mut Class, candidate: Class, name: &CStr) -> Result<(), MetadataError> {
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
        let mut access = Exports::test_stubs();
        access.class_field = lookup;
        access.cache = Some(Box::leak(Box::new(FieldCache::default())));
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
        unsafe extern "C-unwind" fn by_value(_: Type) -> bool {
            false
        }
        unsafe extern "C-unwind" fn free(value: Ptr) {
            // SAFETY: allocation comes from type_name and is released once.
            unsafe {
                drop(CString::from_raw(value.cast()));
            }
        }
        let mut access = Exports::test_stubs();
        access.class_methods = methods;
        access.method_name = name;
        access.method_params = count;
        access.method_instance = yes;
        access.method_generic = no;
        access.method_inflated = no;
        access.method_flags = flags;
        access.method_return = returns;
        access.type_byref = by_value;
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
        let result = unsafe { Exports::resolve(|_| None) };
        assert!(matches!(result, Err(MetadataError::MissingExport(raw::MissingExport(name))) if name == c"il2cpp_domain_get"));
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
        assert!(matches!(record_class(&mut found, other, c"Forwarded"), Err(MetadataError::Contract(message)) if message == "ambiguous class Forwarded"));
        assert_eq!(found, class);
    }
}
