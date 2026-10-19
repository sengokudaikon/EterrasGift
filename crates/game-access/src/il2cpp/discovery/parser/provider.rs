//! Parse with an exact rooted NumberFormatInfo; no culture capture, mutation or locale inference.
use super::{RuntimeAccess, Unavailable, scalar_text};
use crate::il2cpp::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, StrongRoot, ValueTypeContract, raw};

impl RuntimeAccess {
    // Inherits parse_display_scalar_with_provider's retained immutable runtime/metadata contract.
    unsafe fn parser_exact_provider(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, provider: &StrongRoot<'_, '_>) -> Result<(), Unavailable> {
        if !provider.belongs_to(attached) {
            return Err(Unavailable::Contract("provider belongs to another attachment"));
        }
        let target = provider.target().ok_or(Unavailable::Runtime("provider root lost target"))?;
        unsafe {
            let expected = self.parser_class(api, c"System.Globalization", c"NumberFormatInfo")?;
            if (api.object_get_class)(target.as_ptr()) != expected.as_ptr() {
                return Err(Unavailable::Contract("provider must be exact corlib NumberFormatInfo"));
            }
            Ok(())
        }
    }

    /// Parse with an exact already rooted NumberFormatInfo; never recreate it from CultureInfo.Name.
    /// Removes only the exact final unit, uses NumberStyles231 and returns only finite f64.
    /// This does not capture or validate the context used to produce the supplied display text.
    ///
    /// # Safety
    /// All tables, metadata, attachment and provider belong to the same initialized retained
    /// source-audited Unity6000.4 runtime with nonmoving GC and immutable valid metadata. Honor
    /// required engine-thread access and exclude shutdown/detach/reentrancy. The containing game
    /// operation independently owns offline/target/generation admission and the provenance of text
    /// and its provider. It must ensure this exact provider was used by that formatter when claiming
    /// a parsed mapping; this method makes no locale/current-thread-culture equivalence assertion.
    /// Keep the provider and its settings/reference graph unchanged throughout this call, including
    /// by other threads, reflection/native actors, managed reentry and profiler/GC callbacks. Roots,
    /// identity and IsReadOnly flags alone do not provide synchronization or detect restore-to-same
    /// ABA. Mutable NumberFormatInfo is allowed only under this explicit nonmutation obligation.
    /// Authorize the exact Double.TryParse/NumberFormatInfo BCL closure, class initializers,
    /// globalization tables, framework caches/locks/allocations/barriers and GC/profiler callbacks.
    /// The UTF16 factory copies synchronously without retaining native slots; scalar-out invocation
    /// retains no f64 pointer. Managed exceptions remain rooted data and are not inspected by calls.
    /// Native faults/SEH, OOM recovery, retry safety and gameplay rollback are not promised.
    pub unsafe fn parse_display_scalar_with_provider(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        factory: raw::StringFactoryApi,
        provider: &StrongRoot<'_, '_>,
        display: &str,
        unit: &str,
    ) -> Result<f64, Unavailable> {
        // Share the original exact suffix/NUL/4096UTF16/16KiBUTF8 policy without a language lookup.
        let scalar = scalar_text("", display, unit)?;
        unsafe {
            self.parser_exact_provider(attached, api, provider)?;
            let string = self.parser_class(api, c"System", c"String")?;
            let styles = self.parser_class(api, c"System.Globalization", c"NumberStyles")?;
            let number = self.parser_class(api, c"System", c"Double")?;
            let boolean = self.parser_class(api, c"System", c"Boolean")?;
            let interface = self.parser_class(api, c"System", c"IFormatProvider")?;
            let method = self.parser_method(api, number, c"TryParse", boolean, &[(string, false), (styles, false), (interface, false), (number, true)])?;
            let calls = InvocationContext::new(attached, api);
            let method = calls
                .bind_with_value_types(method, &[ValueTypeContract::I32Enum { class: styles }])
                .map_err(|_| Unavailable::Contract("TryParse checked signature"))?;
            let scalar = self.parser_string(attached, api, factory, string.as_ptr(), scalar)?;
            let mut out = Scalar::F64(0.0);
            let result = method
                .invoke(
                    None,
                    &mut [
                        Argument::Reference(Some(&scalar)),
                        Argument::EnumI32 { class: styles, value: 231 },
                        Argument::Reference(Some(provider)),
                        Argument::ByRef(&mut out),
                    ],
                )
                .map_err(|_| Unavailable::Contract("TryParse invocation/result"))?;
            match result {
                InvocationOutcome::Returned(ReturnValue::Scalar {
                    value: Scalar::Boolean(true),
                    root: _root,
                }) => match out {
                    Scalar::F64(value) if value.is_finite() => Ok(value),
                    _ => Err(Unavailable::Scalar),
                },
                InvocationOutcome::Returned(ReturnValue::Scalar {
                    value: Scalar::Boolean(false),
                    root: _root,
                })
                | InvocationOutcome::Exception(_root) => Err(Unavailable::Scalar),
                _ => Err(Unavailable::Contract("TryParse Boolean return")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il2cpp::ThreadContext;
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
        ffi::{CStr, c_char, c_void},
        ptr::NonNull,
    };
    const CORLIB: usize = 0x10;
    const STRING: usize = 0x20;
    const STYLES: usize = 0x30;
    const DOUBLE: usize = 0x40;
    const DOUBLE_OUT: usize = 0x41;
    const BOOLEAN: usize = 0x50;
    const INTERFACE: usize = 0x60;
    const NFI: usize = 0x70;
    const I32: usize = 0x80;
    const PROVIDER: usize = 0x1000;
    const TEXT: usize = 0x1100;
    const BOOL_BOX: usize = 0x1200;
    const EXCEPTION: usize = 0x1300;
    const METHOD: usize = 0x2000;
    #[derive(Default)]
    struct Fake {
        mode: u8,
        roots: BTreeMap<usize, usize>,
        lookups: usize,
        allocations: usize,
        calls: usize,
        needs_root: usize,
        rooted_results: usize,
        copied: Vec<u16>,
    }
    thread_local! {static FAKE:RefCell<Fake>=RefCell::new(Fake::default());static BOOLEAN_STORAGE:Cell<u8>=const{Cell::new(1)};}
    fn ptr<T>(n: usize) -> *mut T {
        std::ptr::without_provenance_mut(n)
    }
    fn nn<T>(n: usize) -> NonNull<T> {
        NonNull::new(ptr(n)).unwrap()
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        ptr(1)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        ptr(2)
    }
    unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
        panic!("already attached")
    }
    unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
        panic!("borrowed attachment")
    }
    unsafe extern "C-unwind" fn new_root(o: *mut raw::Object, pinned: bool) -> *mut c_void {
        assert!(!pinned);
        FAKE.with_borrow_mut(|f| {
            if matches!((f.mode, o as usize), (14, BOOL_BOX) | (15, EXCEPTION) | (16, TEXT)) {
                return std::ptr::null_mut();
            }
            *f.roots.entry(o as usize).or_default() += 1;
            if f.needs_root == o as usize {
                f.needs_root = 0;
                f.rooted_results += 1;
            }
            o.cast()
        })
    }
    unsafe extern "C-unwind" fn free(h: *mut c_void) {
        FAKE.with_borrow_mut(|f| {
            let n = f.roots.get_mut(&(h as usize)).unwrap();
            *n -= 1;
            if *n == 0 {
                f.roots.remove(&(h as usize));
            }
        })
    }
    unsafe extern "C-unwind" fn target(h: *mut c_void) -> *mut raw::Object {
        FAKE.with_borrow(|f| assert!(f.roots.contains_key(&(h as usize))));
        h.cast()
    }
    unsafe extern "C-unwind" fn corlib() -> *const c_void {
        ptr(CORLIB)
    }
    unsafe extern "C-unwind" fn class_name(_: *const c_void, ns: *const c_char, name: *const c_char) -> *mut raw::Class {
        // SAFETY: exact static NUL-terminated class names supplied by production helper.
        let ns = unsafe { CStr::from_ptr(ns) };
        let name = unsafe { CStr::from_ptr(name) };
        FAKE.with_borrow_mut(|f| {
            f.lookups += 1;
            ptr(match (ns.to_bytes(), name.to_bytes()) {
                (b"System.Globalization", b"NumberFormatInfo") => {
                    if f.mode == 2 {
                        0
                    } else {
                        NFI
                    }
                }
                (b"System.Globalization", b"NumberStyles") => STYLES,
                (b"System", b"String") => STRING,
                (b"System", b"Double") => DOUBLE,
                (b"System", b"Boolean") => BOOLEAN,
                (b"System", b"IFormatProvider") => INTERFACE,
                (b"System", b"Int32") => I32,
                _ => panic!("provider parser must not lookup CultureInfo or unrelated classes"),
            })
        })
    }
    unsafe extern "C-unwind" fn class_image(c: *mut raw::Class) -> *mut c_void {
        ptr(FAKE.with_borrow(|f| if f.mode == 3 && c as usize == NFI { CORLIB + 1 } else { CORLIB }))
    }
    unsafe extern "C-unwind" fn methods(_: *mut raw::Class, it: *mut *mut c_void) -> *const raw::MethodInfo {
        // SAFETY: parser_method supplies writable initialized iterator storage.
        let index = unsafe { *it } as usize;
        unsafe { *it = ptr(index + 1) };
        ptr(if index == 0 { METHOD } else { 0 })
    }
    unsafe extern "C-unwind" fn method_name(_: *const raw::MethodInfo) -> *const c_char {
        c"TryParse".as_ptr()
    }
    unsafe extern "C-unwind" fn owner(_: *const raw::MethodInfo) -> *mut raw::Class {
        ptr(DOUBLE)
    }
    unsafe extern "C-unwind" fn no_method(_: *const raw::MethodInfo) -> bool {
        false
    }
    unsafe extern "C-unwind" fn flags(_: *const raw::MethodInfo, implementation: *mut u32) -> u32 {
        // SAFETY: CheckedMethod supplies initialized writable implementation flags storage.
        if !implementation.is_null() {
            unsafe { *implementation = 0 }
        }
        0x10
    }
    unsafe extern "C-unwind" fn count(_: *const raw::MethodInfo) -> u32 {
        4
    }
    unsafe extern "C-unwind" fn returns(_: *const raw::MethodInfo) -> *const raw::Type {
        ptr(FAKE.with_borrow(|f| if f.mode == 5 { DOUBLE } else { BOOLEAN }))
    }
    unsafe extern "C-unwind" fn param(_: *const raw::MethodInfo, i: u32) -> *const raw::Type {
        ptr([STRING, STYLES, INTERFACE, DOUBLE_OUT][i as usize])
    }
    unsafe extern "C-unwind" fn byref(t: *const raw::Type) -> bool {
        t as usize == DOUBLE_OUT && FAKE.with_borrow(|f| f.mode != 4)
    }
    unsafe extern "C-unwind" fn type_class(t: *const raw::Type) -> *mut raw::Class {
        ptr(if t as usize == DOUBLE_OUT { DOUBLE } else { t as usize })
    }
    unsafe extern "C-unwind" fn code(t: *const raw::Type) -> i32 {
        match t as usize {
            STRING => 0x0e,
            STYLES => 0x11,
            DOUBLE | DOUBLE_OUT => 0x0d,
            BOOLEAN => 0x02,
            I32 => 0x08,
            _ => 0x12,
        }
    }
    unsafe extern "C-unwind" fn value(c: *const raw::Class) -> bool {
        matches!(c as usize, STYLES | DOUBLE | BOOLEAN | I32)
    }
    unsafe extern "C-unwind" fn generic(c: *const raw::Class) -> bool {
        c as usize == STYLES && FAKE.with_borrow(|f| f.mode == 6)
    }
    unsafe extern "C-unwind" fn inflated(_: *const raw::Class) -> bool {
        false
    }
    unsafe extern "C-unwind" fn enumeration(c: *const raw::Class) -> bool {
        c as usize == STYLES
    }
    unsafe extern "C-unwind" fn base(_: *mut raw::Class) -> *const raw::Type {
        ptr(I32)
    }
    unsafe extern "C-unwind" fn size(_: *mut raw::Class, alignment: *mut u32) -> i32 {
        // SAFETY: value-contract validation supplies initialized u32 storage.
        unsafe { *alignment = 4 };
        4
    }
    unsafe extern "C-unwind" fn assignable(expected: *mut raw::Class, actual: *mut raw::Class) -> bool {
        expected == actual || (expected as usize == INTERFACE && actual as usize == NFI && FAKE.with_borrow(|f| f.mode != 19))
    }
    unsafe extern "C-unwind" fn object_class(o: *mut raw::Object) -> *mut raw::Class {
        FAKE.with_borrow(|f| {
            assert_eq!(f.needs_root, 0, "fresh returned object must root before metadata");
            assert!(f.roots.contains_key(&(o as usize)), "object metadata requires an existing root");
            ptr(match o as usize {
                PROVIDER => {
                    if f.mode == 1 {
                        NFI + 1
                    } else {
                        NFI
                    }
                }
                TEXT => {
                    if f.mode == 18 {
                        DOUBLE
                    } else {
                        STRING
                    }
                }
                BOOL_BOX => {
                    if f.mode == 13 {
                        DOUBLE
                    } else {
                        BOOLEAN
                    }
                }
                EXCEPTION => NFI,
                _ => panic!("unknown object"),
            })
        })
    }
    unsafe extern "C-unwind" fn unbox(_: *mut raw::Object) -> *mut c_void {
        BOOLEAN_STORAGE.with(|v| v.as_ptr().cast())
    }
    unsafe extern "C-unwind" fn virtual_method(_: *mut raw::Object, _: *const raw::MethodInfo) -> *const raw::MethodInfo {
        panic!("no virtual dispatch API needed")
    }
    unsafe extern "C-unwind" fn string_new(data: *const u16, length: i32) -> *mut raw::Object {
        assert!((0..=4096).contains(&length));
        FAKE.with_borrow_mut(|f| {
            f.allocations += 1;
            // SAFETY: exact factory copies the supplied bounded UTF16 synchronously.
            f.copied = unsafe { std::slice::from_raw_parts(data, length as usize) }.to_vec();
            if f.mode == 17 { std::ptr::null_mut() } else { ptr(TEXT) }
        })
    }
    unsafe extern "C-unwind" fn invoke(_: *const raw::MethodInfo, receiver: *mut c_void, args: *mut *mut c_void, exception: *mut *mut raw::Exception) -> *mut raw::Object {
        assert!(receiver.is_null());
        FAKE.with_borrow_mut(|f| {
            f.calls += 1;
            // SAFETY: CheckedMethod builds exact four argument slots with synchronous scalar out.
            unsafe {
                assert_eq!(*args, ptr(TEXT));
                assert_eq!((*args.add(1)).cast::<i32>().read(), 231);
                assert_eq!(*args.add(2), ptr(PROVIDER));
                assert!(f.roots.contains_key(&PROVIDER) && f.roots.contains_key(&TEXT));
                (*args.add(3)).cast::<f64>().write(match f.mode {
                    10 => f64::NAN,
                    11 => f64::INFINITY,
                    _ => 12.5,
                });
                if f.mode == 12 || f.mode == 15 {
                    *exception = ptr(EXCEPTION);
                    f.needs_root = EXCEPTION;
                    return std::ptr::null_mut();
                }
            }
            BOOLEAN_STORAGE.set(u8::from(f.mode != 9));
            f.needs_root = BOOL_BOX;
            ptr(BOOL_BOX)
        })
    }
    fn api() -> raw::InvocationApi {
        raw::InvocationApi {
            runtime_invoke: invoke,
            method_get_return_type: returns,
            method_get_class: owner,
            method_is_generic: no_method,
            method_is_inflated: no_method,
            method_is_instance: no_method,
            method_get_param_count: count,
            method_get_param: param,
            method_get_flags: flags,
            type_get_type: code,
            type_is_byref: byref,
            class_from_type: type_class,
            class_is_valuetype: value,
            class_is_generic: generic,
            class_is_inflated: inflated,
            class_is_assignable_from: assignable,
            object_get_class: object_class,
            object_unbox: unbox,
            object_get_virtual_method: virtual_method,
            get_corlib: corlib,
            class_from_name: class_name,
            class_is_enum: enumeration,
            class_enum_basetype: base,
            class_value_size: size,
        }
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        FAKE.with_borrow_mut(|f| {
            assert!(f.roots.is_empty());
            *f = Fake::default()
        });
        let mut a = RuntimeAccess::test_stubs();
        a.class_image = class_image;
        a.class_methods = methods;
        a.method_name = method_name;
        // SAFETY: authored already-attached runtime on this thread, with exact pointer-valued handles.
        let ctx = unsafe {
            ThreadContext::new(raw::Api {
                domain_get: domain,
                thread_current: current,
                thread_attach: attach,
                thread_detach: detach,
                gchandle_new: new_root,
                gchandle_free: free,
                gchandle_get_target: target,
            })
        };
        (a, ctx)
    }
    fn roots() -> usize {
        FAKE.with_borrow(|f| f.roots.values().sum())
    }
    #[test]
    fn exact_provider_flows_through_checked_invocation_without_any_culture_lookup() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        // SAFETY: opaque authored provider is alive on this fake attachment until its root drops.
        let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
        // SAFETY: exact authored table/allocator/body; no metadata is read before required roots.
        assert_eq!(
            unsafe { a.parse_display_scalar_with_provider(&attached, api(), raw::StringFactoryApi { string_new_utf16: string_new }, &provider, "12,5%", "%") },
            Ok(12.5)
        );
        FAKE.with_borrow(|f| {
            assert_eq!(f.calls, 1);
            assert_eq!(f.allocations, 1);
            assert_eq!(String::from_utf16(&f.copied).unwrap(), "12,5");
            assert_eq!(f.rooted_results, 1)
        });
        assert_eq!(roots(), 1, "temporary scalar and Boolean box roots released; caller provider remains");
    }
    #[test]
    fn invalid_input_unit_and_foreign_context_fail_before_lookup_factory_or_body() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        let foreign_ctx = {
            let (_, ctx) = fixture();
            ctx
        };
        let foreign = foreign_ctx.attach().unwrap();
        // SAFETY: fake provider belongs only to the first context.
        let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
        let factory = raw::StringFactoryApi { string_new_utf16: string_new };
        for (d, u) in [("12\0", ""), ("12% ", "%"), ("12", "%")] {
            assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), factory, &provider, d, u) }.is_err());
        }
        assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), factory, &provider, &"1".repeat(4097), "") }.is_err());
        assert!(unsafe { a.parse_display_scalar_with_provider(&foreign, api(), factory, &provider, "12", "") }.is_err());
        FAKE.with_borrow(|f| {
            assert_eq!(f.lookups, 0);
            assert_eq!(f.allocations, 0);
            assert_eq!(f.calls, 0)
        });
    }
    #[test]
    fn wrong_provider_class_missing_exact_corlib_class_or_wrong_image_refuses_before_allocation() {
        for mode in [1, 2, 3] {
            let (a, ctx) = fixture();
            let attached = ctx.attach().unwrap();
            // SAFETY: one deliberately incompatible provider identity fact, held in a live fake root.
            let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
            FAKE.with_borrow_mut(|f| f.mode = mode);
            assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), raw::StringFactoryApi { string_new_utf16: string_new }, &provider, "12", "") }.is_err());
            FAKE.with_borrow(|f| {
                assert_eq!(f.allocations, 0);
                assert_eq!(f.calls, 0)
            });
            assert_eq!(roots(), 1);
        }
    }
    #[test]
    fn wrong_out_signature_return_or_enum_contract_refuses_before_factory_or_body() {
        for mode in [4, 5, 6] {
            let (a, ctx) = fixture();
            let attached = ctx.attach().unwrap();
            // SAFETY: exact fake provider; malformed method metadata is rejected without body entry.
            let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
            FAKE.with_borrow_mut(|f| f.mode = mode);
            assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), raw::StringFactoryApi { string_new_utf16: string_new }, &provider, "12", "") }.is_err());
            FAKE.with_borrow(|f| {
                assert_eq!(f.allocations, 0);
                assert_eq!(f.calls, 0)
            });
            assert_eq!(roots(), 1);
        }
    }
    #[test]
    fn false_nonfinite_exception_wrong_box_and_result_root_failure_never_expose_out_value() {
        for mode in [9, 10, 11, 12, 13, 14, 15] {
            let (a, ctx) = fixture();
            let attached = ctx.attach().unwrap();
            // SAFETY: exact fake checked body controls one invalid outcome after synchronous out write.
            let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
            FAKE.with_borrow_mut(|f| f.mode = mode);
            assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), raw::StringFactoryApi { string_new_utf16: string_new }, &provider, "12", "") }.is_err());
            FAKE.with_borrow(|f| {
                assert_eq!(f.calls, 1);
                if mode == 12 {
                    assert_eq!(f.rooted_results, 1)
                }
            });
            assert_eq!(roots(), 1);
        }
    }
    #[test]
    fn allocation_null_root_failure_and_wrong_string_keep_provider_root_and_skip_try_parse() {
        for mode in [16, 17, 18] {
            let (a, ctx) = fixture();
            let attached = ctx.attach().unwrap();
            // SAFETY: fake factory failure occurs while caller's exact provider stays rooted.
            let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
            FAKE.with_borrow_mut(|f| f.mode = mode);
            assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), raw::StringFactoryApi { string_new_utf16: string_new }, &provider, "12", "") }.is_err());
            FAKE.with_borrow(|f| assert_eq!(f.calls, 0));
            assert_eq!(roots(), 1);
        }
    }
    #[test]
    fn interface_assignability_is_still_checked_before_body() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        // SAFETY: fake exact class identity with deliberately inconsistent interface metadata.
        let provider = unsafe { attached.root(nn(PROVIDER)) }.unwrap();
        FAKE.with_borrow_mut(|f| f.mode = 19);
        assert!(unsafe { a.parse_display_scalar_with_provider(&attached, api(), raw::StringFactoryApi { string_new_utf16: string_new }, &provider, "12", "") }.is_err());
        FAKE.with_borrow(|f| {
            assert_eq!(f.allocations, 1);
            assert_eq!(f.calls, 0)
        });
        assert_eq!(roots(), 1);
    }
}
