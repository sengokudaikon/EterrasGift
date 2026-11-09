//! The runtime's public exports, resolved once into one table. Attachment and roots ([`crate::ThreadContext`]),
//! checked invocation ([`crate::InvocationContext`]) and the lookups ([`crate::metadata`]) are views over it. The two
//! exports that store into the managed graph are resolved in the same pass but handed out apart, as
//! [`raw::FieldWriteApi`], so code given only the table cannot write. Signatures follow Unity 6000.4.8f1's
//! `il2cpp-api-functions.h`.

use crate::metadata::{Assembly, Class, Field, Image, MetadataError, Method, Ptr, Type};
use crate::raw;
use std::{
    ffi::{CStr, c_char, c_void},
    ptr::NonNull,
};

// Field descriptors belong to immutable metadata in the retained runtime. Only descriptor lookup is cached; object
// values and native liveness are always read afresh.
#[derive(Default)]
pub(crate) struct FieldCache {
    // Keyed by class, then name: a hit borrows the caller's name and allocates nothing.
    pub(crate) fields: std::sync::Mutex<std::collections::HashMap<usize, std::collections::HashMap<std::ffi::CString, usize>>>,
}

macro_rules! exports {
    (
        $($field:ident: $name:literal => $signature:ty),+ $(,)?;
        writes: $($write:ident: $write_name:literal => $write_signature:ty),+ $(,)?
    ) => {
        /// Every export this crate calls but the two that store, resolved from one runtime. Resolution alone calls none
        /// of them. Each field is the named export; calling one directly is the caller's unsafe contract.
        #[derive(Clone, Copy)]
        pub struct Exports { $(pub $field: $signature),+, pub(crate) cache: Option<&'static FieldCache> }
        impl Exports {
            /// Every export a stub that panics when reached, and no cache: a test sets the exports it expects.
            #[cfg(any(test, feature = "test-stubs"))]
            pub fn test_stubs() -> Self { Self { $($field: <$signature as TestExportStub>::test_stub()),+, cache: None } }
            /// Resolve the table, and apart from it the exports that store into the managed graph. Nothing partial is
            /// returned: the first missing export fails the whole.
            ///
            /// # Safety
            /// Every address must be the named Unity 6000.4 public export with the declared ABI, from one live
            /// runtime. Its code and immutable metadata must remain loaded for every use of either table. GC handles
            /// use the modern pointer-valued ABI. The resolver must not substitute other functions.
            pub unsafe fn resolve(mut resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<(Self, raw::FieldWriteApi), MetadataError> {
                let mut export = |name: &'static str| {
                    let name = CStr::from_bytes_with_nul(name.as_bytes()).expect("static symbol");
                    resolver(name).ok_or(MetadataError::MissingExport(raw::MissingExport(name)))
                };
                let exports = Self { $($field: {
                    let address = export(concat!($name, "\0"))?;
                    // SAFETY: the caller establishes export identity, ABI and lifetime.
                    unsafe { std::mem::transmute::<*mut c_void, $signature>(address.as_ptr()) }
                }),+, cache: Some(Box::leak(Box::new(FieldCache::default()))) };
                let writes = raw::FieldWriteApi { $($write: {
                    let address = export(concat!($write_name, "\0"))?;
                    // SAFETY: as above.
                    unsafe { std::mem::transmute::<*mut c_void, $write_signature>(address.as_ptr()) }
                }),+ };
                Ok((exports, writes))
            }
        }
    };
}

// Every test stub has its field's actual ABI/signature and fails if an unexpected export is
// reached. Unlike casting one catch-all address, this does not manufacture mismatched functions.
#[cfg(any(test, feature = "test-stubs"))]
trait TestExportStub {
    fn test_stub() -> Self;
}
#[cfg(any(test, feature = "test-stubs"))]
macro_rules! test_export_stubs {
    ($($argument:ident),*) => {
        impl<$($argument,)* R> TestExportStub for unsafe extern "C-unwind" fn($($argument),*) -> R {
            fn test_stub() -> Self {
                unsafe extern "C-unwind" fn unexpected<$($argument,)* R>($(_: $argument),*) -> R { panic!("unexpected runtime export"); }
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
#[cfg(any(test, feature = "test-stubs"))]
test_export_stubs!(A, B, C, D);

exports! {
    // Attachment and strong roots, which `ThreadContext` calls. Loading them calls none.
    domain_get: "il2cpp_domain_get" => unsafe extern "C-unwind" fn() -> *mut raw::Domain,
    thread_current: "il2cpp_thread_current" => unsafe extern "C-unwind" fn() -> *mut raw::Thread,
    thread_attach: "il2cpp_thread_attach" => unsafe extern "C-unwind" fn(*mut raw::Domain) -> *mut raw::Thread,
    thread_detach: "il2cpp_thread_detach" => unsafe extern "C-unwind" fn(*mut raw::Thread),
    gchandle_new: "il2cpp_gchandle_new" => unsafe extern "C-unwind" fn(*mut raw::Object, bool) -> Ptr,
    gchandle_free: "il2cpp_gchandle_free" => unsafe extern "C-unwind" fn(Ptr),
    gchandle_target: "il2cpp_gchandle_get_target" => unsafe extern "C-unwind" fn(Ptr) -> *mut raw::Object,
    // Metadata, allocation and status, which the lookups call. None runs a managed body or a class initializer.
    assemblies: "il2cpp_domain_get_assemblies" => unsafe extern "C-unwind" fn(*const raw::Domain, *mut usize) -> *const Assembly,
    assembly_image: "il2cpp_assembly_get_image" => unsafe extern "C-unwind" fn(Assembly) -> Image,
    class_from_name: "il2cpp_class_from_name" => unsafe extern "C-unwind" fn(Image, *const c_char, *const c_char) -> Class,
    class_parent: "il2cpp_class_get_parent" => unsafe extern "C-unwind" fn(Class) -> Class,
    class_nested: "il2cpp_class_get_nested_types" => unsafe extern "C-unwind" fn(Class, *mut Ptr) -> Class,
    class_name: "il2cpp_class_get_name" => unsafe extern "C-unwind" fn(Class) -> *const c_char,
    class_namespace: "il2cpp_class_get_namespace" => unsafe extern "C-unwind" fn(Class) -> *const c_char,
    class_image: "il2cpp_class_get_image" => unsafe extern "C-unwind" fn(Class) -> Image,
    class_field: "il2cpp_class_get_field_from_name" => unsafe extern "C-unwind" fn(Class, *const c_char) -> Field,
    field_type: "il2cpp_field_get_type" => unsafe extern "C-unwind" fn(Field) -> Type,
    field_parent: "il2cpp_field_get_parent" => unsafe extern "C-unwind" fn(Field) -> Class,
    field_flags: "il2cpp_field_get_flags" => unsafe extern "C-unwind" fn(Field) -> i32,
    field_offset: "il2cpp_field_get_offset" => unsafe extern "C-unwind" fn(Field) -> usize,
    static_get: "il2cpp_field_static_get_value" => unsafe extern "C-unwind" fn(Field, Ptr),
    field_get: "il2cpp_field_get_value" => unsafe extern "C-unwind" fn(*mut raw::Object, Field, Ptr),
    field_get_object: "il2cpp_field_get_value_object" => unsafe extern "C-unwind" fn(Field, *mut raw::Object) -> *mut raw::Object,
    type_name: "il2cpp_type_get_name" => unsafe extern "C-unwind" fn(Type) -> *mut c_char,
    runtime_free: "il2cpp_free" => unsafe extern "C-unwind" fn(Ptr),
    type_class: "il2cpp_class_from_il2cpp_type" => unsafe extern "C-unwind" fn(Type) -> Class,
    type_code: "il2cpp_type_get_type" => unsafe extern "C-unwind" fn(Type) -> i32,
    type_byref: "il2cpp_type_is_byref" => unsafe extern "C-unwind" fn(Type) -> bool,
    class_type: "il2cpp_class_get_type" => unsafe extern "C-unwind" fn(Class) -> Type,
    class_element: "il2cpp_class_get_element_class" => unsafe extern "C-unwind" fn(Class) -> Class,
    class_rank: "il2cpp_class_get_rank" => unsafe extern "C-unwind" fn(*const raw::Class) -> i32,
    class_static_data: "il2cpp_class_get_static_field_data" => unsafe extern "C-unwind" fn(Class) -> Ptr,
    class_value: "il2cpp_class_is_valuetype" => unsafe extern "C-unwind" fn(*const raw::Class) -> bool,
    class_enum: "il2cpp_class_is_enum" => unsafe extern "C-unwind" fn(*const raw::Class) -> bool,
    enum_base: "il2cpp_class_enum_basetype" => unsafe extern "C-unwind" fn(Class) -> Type,
    class_methods: "il2cpp_class_get_methods" => unsafe extern "C-unwind" fn(Class, *mut Ptr) -> Method,
    method_name: "il2cpp_method_get_name" => unsafe extern "C-unwind" fn(Method) -> *const c_char,
    method_params: "il2cpp_method_get_param_count" => unsafe extern "C-unwind" fn(Method) -> u32,
    method_param: "il2cpp_method_get_param" => unsafe extern "C-unwind" fn(Method, u32) -> Type,
    method_return: "il2cpp_method_get_return_type" => unsafe extern "C-unwind" fn(Method) -> Type,
    method_instance: "il2cpp_method_is_instance" => unsafe extern "C-unwind" fn(Method) -> bool,
    method_generic: "il2cpp_method_is_generic" => unsafe extern "C-unwind" fn(Method) -> bool,
    method_inflated: "il2cpp_method_is_inflated" => unsafe extern "C-unwind" fn(Method) -> bool,
    method_flags: "il2cpp_method_get_flags" => unsafe extern "C-unwind" fn(Method, *mut u32) -> u32,
    method_class: "il2cpp_method_get_class" => unsafe extern "C-unwind" fn(Method) -> Class,
    corlib: "il2cpp_get_corlib" => unsafe extern "C-unwind" fn() -> Image,
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
    object_class: "il2cpp_object_get_class" => unsafe extern "C-unwind" fn(*mut raw::Object) -> Class,
    assignable: "il2cpp_class_is_assignable_from" => unsafe extern "C-unwind" fn(Class, Class) -> bool,
    // What only checked invocation calls, beside the metadata above.
    runtime_invoke: "il2cpp_runtime_invoke" => unsafe extern "C-unwind" fn(Method, Ptr, *mut Ptr, *mut *mut raw::Exception) -> *mut raw::Object,
    class_generic: "il2cpp_class_is_generic" => unsafe extern "C-unwind" fn(*const raw::Class) -> bool,
    class_inflated: "il2cpp_class_is_inflated" => unsafe extern "C-unwind" fn(*const raw::Class) -> bool,
    object_unbox: "il2cpp_object_unbox" => unsafe extern "C-unwind" fn(*mut raw::Object) -> Ptr,
    virtual_method: "il2cpp_object_get_virtual_method" => unsafe extern "C-unwind" fn(*mut raw::Object, Method) -> Method,
    value_size: "il2cpp_class_value_size" => unsafe extern "C-unwind" fn(Class, *mut u32) -> i32;
    writes:
    field_set_value: "il2cpp_field_set_value" => raw::FieldSetValue,
    element_barrier: "il2cpp_gc_wbarrier_set_field" => raw::ElementBarrier,
}

impl Exports {
    /// The body `object` runs for `method`: its class's override when `method` is virtual, `method` itself when it is
    /// not, and none when the runtime has no override for it.
    ///
    /// # Safety
    /// An attached thread of this table's runtime, `object` a live object of it, and `method` a method of one of the
    /// object's classes.
    pub unsafe fn dispatched(&self, object: NonNull<raw::Object>, method: Method) -> Option<Method> {
        // SAFETY: as this function's; the flags query reads only the method's descriptor.
        unsafe {
            if (self.method_flags)(method, std::ptr::null_mut()) & raw::METHOD_ATTRIBUTE_VIRTUAL == 0 {
                return Some(method);
            }
            let body = (self.virtual_method)(object.as_ptr(), method);
            (!body.is_null()).then_some(body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_fails_before_invoking_any_missing_export() {
        // SAFETY: no address is returned, so no ABI/lifetime promises are exercised.
        let result = unsafe { Exports::resolve(|_| None) };
        assert!(matches!(result, Err(MetadataError::MissingExport(raw::MissingExport(name))) if name == c"il2cpp_domain_get"));
    }

    #[test]
    fn a_missing_store_export_fails_the_whole_table() {
        // Any address will do: resolution only transmutes it, and nothing here calls one.
        let mut anything = 0_u8;
        let address = NonNull::from(&mut anything).cast::<c_void>();
        // SAFETY: as above; no resolved export is called.
        let result = unsafe { Exports::resolve(|name| (name != c"il2cpp_gc_wbarrier_set_field").then_some(address)) };
        assert!(matches!(result, Err(MetadataError::MissingExport(raw::MissingExport(name))) if name == c"il2cpp_gc_wbarrier_set_field"));
    }
}
