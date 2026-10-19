use super::*;
use crate::il2cpp::ThreadContext;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::{c_char, c_void},
};

static_assertions::assert_not_impl_any!(NumericFormatContext<'static, 'static>: Send, Sync, Clone);
const CORLIB: usize = 0x10;
const THREAD: usize = 0x20;
const CULTURE: usize = 0x30;
const NFI: usize = 0x40;
const THREAD_GET: usize = 0x100;
const CULTURE_GET: usize = 0x110;
const INFO_GET: usize = 0x120;
const OTHER: usize = 0x130;
const FIELD: usize = 0x200;
const THREAD_OBJECT: usize = 0x1000;
const CULTURE_OBJECT: usize = 0x1100;
const PROVIDER: usize = 0x1200;
const EXCEPTION: usize = 0x1300;
#[derive(Default)]
struct Fake {
    mode: u8,
    roots: BTreeMap<usize, usize>,
    needs_root: usize,
    root_attempts: usize,
    fail_root: usize,
    calls: Vec<usize>,
    exception_call: usize,
    metadata: usize,
    field_reads: usize,
    changed: Option<NumericFormatContextPart>,
    lost: usize,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
fn ptr<T>(value: usize) -> *mut T {
    std::ptr::without_provenance_mut(value)
}
fn meta() {
    FAKE.with_borrow_mut(|f| {
        assert_eq!(f.needs_root, 0, "root a fresh return before metadata");
        f.metadata += 1;
    });
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
unsafe extern "C-unwind" fn new_root(object: *mut raw::Object, pinned: bool) -> *mut c_void {
    assert!(!pinned);
    FAKE.with_borrow_mut(|f| {
        f.root_attempts += 1;
        if f.fail_root == f.root_attempts {
            return std::ptr::null_mut();
        }
        *f.roots.entry(object as usize).or_default() += 1;
        if f.needs_root == object as usize {
            f.needs_root = 0;
        }
        object.cast()
    })
}
unsafe extern "C-unwind" fn free(handle: *mut c_void) {
    FAKE.with_borrow_mut(|f| {
        let count = f.roots.get_mut(&(handle as usize)).unwrap();
        *count -= 1;
        if *count == 0 {
            f.roots.remove(&(handle as usize));
        }
    });
}
unsafe extern "C-unwind" fn target(handle: *mut c_void) -> *mut raw::Object {
    FAKE.with_borrow(|f| {
        assert!(f.roots.contains_key(&(handle as usize)));
        if f.lost == handle as usize { std::ptr::null_mut() } else { handle.cast() }
    })
}
unsafe extern "C-unwind" fn corlib() -> *const c_void {
    meta();
    ptr(FAKE.with_borrow(|f| if f.mode == 25 { 0 } else { CORLIB }))
}
unsafe extern "C-unwind" fn from_name(_: *const c_void, ns: *const c_char, name: *const c_char) -> *mut raw::Class {
    meta();
    // SAFETY: adapter supplies static terminated strings.
    let (ns, name) = unsafe { (CStr::from_ptr(ns).to_bytes(), CStr::from_ptr(name).to_bytes()) };
    ptr(match (ns, name) {
        (b"System.Threading", b"Thread") => THREAD,
        (b"System.Globalization", b"CultureInfo") => CULTURE,
        (b"System.Globalization", b"NumberFormatInfo") => FAKE.with_borrow(|f| if f.mode == 24 { 0 } else { NFI }),
        _ => panic!("no Name/IsReadOnly/GetCultureInfo or unrelated class lookup"),
    })
}
unsafe extern "C-unwind" fn image(_: *mut raw::Class) -> *mut c_void {
    meta();
    ptr(FAKE.with_borrow(|f| if f.mode == 1 { CORLIB + 1 } else { CORLIB }))
}
unsafe extern "C-unwind" fn class_value(_: *const raw::Class) -> bool {
    meta();
    FAKE.with_borrow(|f| f.mode == 21)
}
unsafe extern "C-unwind" fn class_enum(_: *const raw::Class) -> bool {
    meta();
    FAKE.with_borrow(|f| f.mode == 23)
}
unsafe extern "C-unwind" fn class_generic(_: *const raw::Class) -> bool {
    meta();
    FAKE.with_borrow(|f| f.mode == 20)
}
unsafe extern "C-unwind" fn class_inflated(_: *const raw::Class) -> bool {
    meta();
    FAKE.with_borrow(|f| f.mode == 22)
}
unsafe extern "C-unwind" fn methods(owner: *mut raw::Class, iterator: *mut *mut c_void) -> *const raw::MethodInfo {
    meta();
    // SAFETY: adapter owns initialized writable iterator.
    let index = unsafe { *iterator } as usize;
    unsafe {
        *iterator = ptr(index + 1);
    }
    ptr(FAKE.with_borrow(|f| {
        if f.mode == 16 {
            return OTHER;
        }
        if f.mode == 15 && owner as usize == THREAD && index == 1 {
            return THREAD_GET;
        }
        match (owner as usize, index) {
            (THREAD, 0) => THREAD_GET,
            (THREAD, 1) => CULTURE_GET,
            (NFI, 0) => INFO_GET,
            _ => 0,
        }
    }))
}
unsafe extern "C-unwind" fn name(method: *const raw::MethodInfo) -> *const c_char {
    meta();
    match method as usize {
        THREAD_GET => c"get_CurrentThread",
        CULTURE_GET => c"get_CurrentCulture",
        INFO_GET => c"get_CurrentInfo",
        _ => c"unrelated",
    }
    .as_ptr()
}
unsafe extern "C-unwind" fn owner(method: *const raw::MethodInfo) -> *mut raw::Class {
    meta();
    ptr(if method as usize == INFO_GET { NFI } else { THREAD })
}
unsafe extern "C-unwind" fn instance(method: *const raw::MethodInfo) -> bool {
    meta();
    method as usize == CULTURE_GET
}
unsafe extern "C-unwind" fn no_method(_: *const raw::MethodInfo) -> bool {
    meta();
    FAKE.with_borrow(|f| f.mode == 14)
}
unsafe extern "C-unwind" fn flags(method: *const raw::MethodInfo, implementation: *mut u32) -> u32 {
    meta();
    // SAFETY: checked caller supplies writable flags storage.
    unsafe {
        *implementation = 0;
    }
    FAKE.with_borrow(|f| {
        let flags = if method as usize == CULTURE_GET { 0 } else { 0x10 };
        flags
            | match f.mode {
                12 => 0x10,
                13 => 0x40,
                _ => 0,
            }
    })
}
unsafe extern "C-unwind" fn count(_: *const raw::MethodInfo) -> u32 {
    meta();
    FAKE.with_borrow(|f| u32::from(f.mode == 17))
}
unsafe extern "C-unwind" fn returns(method: *const raw::MethodInfo) -> *const raw::Type {
    meta();
    ptr(FAKE.with_borrow(|f| {
        if f.mode == 11 {
            NFI
        } else {
            match method as usize {
                THREAD_GET => THREAD,
                CULTURE_GET => CULTURE,
                INFO_GET => NFI,
                _ => panic!("unknown method"),
            }
        }
    }))
}
unsafe extern "C-unwind" fn param(_: *const raw::MethodInfo, _: u32) -> *const raw::Type {
    panic!("zero arguments only")
}
unsafe extern "C-unwind" fn byref(_: *const raw::Type) -> bool {
    meta();
    FAKE.with_borrow(|f| f.mode == 10)
}
unsafe extern "C-unwind" fn type_class(ty: *const raw::Type) -> *mut raw::Class {
    meta();
    ty.cast_mut().cast()
}
unsafe extern "C-unwind" fn type_code(_: *const raw::Type) -> i32 {
    meta();
    0x12
}
unsafe extern "C-unwind" fn assignable(expected: *mut raw::Class, actual: *mut raw::Class) -> bool {
    meta();
    expected == actual || (expected as usize == CULTURE && actual as usize == CULTURE + 1)
}
unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> *mut raw::Class {
    meta();
    FAKE.with_borrow(|f| {
        assert!(f.roots.contains_key(&(object as usize)), "metadata requires a root");
        ptr(match object as usize {
            THREAD_OBJECT => THREAD,
            n if n == THREAD_OBJECT + 1 => THREAD,
            CULTURE_OBJECT => {
                if f.mode == 2 {
                    CULTURE + 1
                } else {
                    CULTURE
                }
            }
            n if n == CULTURE_OBJECT + 1 => CULTURE,
            PROVIDER => NFI,
            n if n == PROVIDER + 1 => NFI,
            EXCEPTION => NFI,
            _ => panic!("unknown rooted object"),
        })
    })
}
unsafe extern "C-unwind" fn unbox(_: *mut raw::Object) -> *mut c_void {
    panic!("no value payloads")
}
unsafe extern "C-unwind" fn virtual_method(_: *mut raw::Object, _: *const raw::MethodInfo) -> *const raw::MethodInfo {
    panic!("no virtual resolver")
}
unsafe extern "C-unwind" fn enum_base(_: *mut raw::Class) -> *const raw::Type {
    panic!("no enum")
}
unsafe extern "C-unwind" fn value_size(_: *mut raw::Class, _: *mut u32) -> i32 {
    panic!("no values")
}
unsafe extern "C-unwind" fn field(owner: *mut raw::Class, name: *const c_char) -> *mut c_void {
    meta();
    assert_eq!(owner as usize, CULTURE);
    // SAFETY: adapter supplies static terminated field name.
    assert_eq!(unsafe { CStr::from_ptr(name) }, c"numInfo");
    ptr(FAKE.with_borrow(|f| if f.mode == 6 { 0 } else { FIELD }))
}
unsafe extern "C-unwind" fn field_parent(_: *mut c_void) -> *mut raw::Class {
    meta();
    ptr(FAKE.with_borrow(|f| if f.mode == 7 { THREAD } else { CULTURE }))
}
unsafe extern "C-unwind" fn field_type(_: *mut c_void) -> *const raw::Type {
    meta();
    ptr(FAKE.with_borrow(|f| if f.mode == 9 { CULTURE } else { NFI }))
}
unsafe extern "C-unwind" fn field_flags(_: *mut c_void) -> i32 {
    meta();
    FAKE.with_borrow(|f| if f.mode == 8 { 0x10 } else { 0 })
}
unsafe extern "C-unwind" fn field_offset(_: *mut c_void) -> usize {
    meta();
    FAKE.with_borrow(|f| if f.mode == 18 { usize::MAX } else { 0x30 })
}
unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, field: *mut c_void, dest: *mut c_void) {
    assert_eq!(field as usize, FIELD);
    FAKE.with_borrow_mut(|f| {
        assert!(f.roots.contains_key(&(object as usize)));
        f.field_reads += 1;
        let value = if f.mode == 19 {
            0
        } else if f.mode == 5 || f.changed == Some(NumericFormatContextPart::Provider) {
            PROVIDER + 1
        } else {
            PROVIDER
        };
        // SAFETY: validated reference field destination is initialized object-pointer storage.
        unsafe {
            dest.cast::<*mut raw::Object>().write(ptr(value));
        }
        f.needs_root = value;
    });
}
unsafe extern "C-unwind" fn invoke(method: *const raw::MethodInfo, receiver: *mut c_void, args: *mut *mut c_void, exception: *mut *mut raw::Exception) -> *mut raw::Object {
    assert!(args.is_null());
    FAKE.with_borrow_mut(|f| {
        assert_eq!(f.needs_root, 0);
        let method = method as usize;
        if method == CULTURE_GET {
            assert!(f.roots.contains_key(&(receiver as usize)));
        } else {
            assert!(receiver.is_null());
        }
        if method == INFO_GET {
            assert_ne!(f.mode, 2, "derived culture reached forbidden virtual closure");
        }
        f.calls.push(method);
        if f.calls.len() == f.exception_call {
            // SAFETY: CheckedMethod supplies writable initialized exception output.
            unsafe {
                *exception = ptr(EXCEPTION);
            }
            f.needs_root = EXCEPTION;
            return std::ptr::null_mut();
        }
        let result = match method {
            THREAD_GET => THREAD_OBJECT + usize::from(f.changed == Some(NumericFormatContextPart::Thread)),
            CULTURE_GET if f.mode == 3 => 0,
            CULTURE_GET => CULTURE_OBJECT + usize::from(f.changed == Some(NumericFormatContextPart::Culture)),
            INFO_GET if f.mode == 4 => 0,
            INFO_GET => PROVIDER + usize::from(f.changed == Some(NumericFormatContextPart::Provider)),
            _ => panic!("unknown body"),
        };
        f.needs_root = result;
        ptr(result)
    })
}
fn api() -> raw::InvocationApi {
    raw::InvocationApi {
        runtime_invoke: invoke,
        method_get_return_type: returns,
        method_get_class: owner,
        method_is_generic: no_method,
        method_is_inflated: no_method,
        method_is_instance: instance,
        method_get_param_count: count,
        method_get_param: param,
        method_get_flags: flags,
        type_get_type: type_code,
        type_is_byref: byref,
        class_from_type: type_class,
        class_is_valuetype: class_value,
        class_is_generic: class_generic,
        class_is_inflated: class_inflated,
        class_is_assignable_from: assignable,
        object_get_class: object_class,
        object_unbox: unbox,
        object_get_virtual_method: virtual_method,
        get_corlib: corlib,
        class_from_name: from_name,
        class_is_enum: class_enum,
        class_enum_basetype: enum_base,
        class_value_size: value_size,
    }
}
fn thread_context() -> ThreadContext {
    // SAFETY: deterministic already-attached fake ABI, handles remain live until roots drop.
    unsafe {
        ThreadContext::new(raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: attach,
            thread_detach: detach,
            gchandle_new: new_root,
            gchandle_free: free,
            gchandle_get_target: target,
        })
    }
}
fn fixture() -> (RuntimeAccess, ThreadContext) {
    FAKE.with_borrow_mut(|f| {
        assert!(f.roots.is_empty());
        *f = Fake::default();
    });
    let mut access = RuntimeAccess::test_stubs();
    access.class_image = image;
    access.class_methods = methods;
    access.method_name = name;
    access.class_field = field;
    access.field_parent = field_parent;
    access.field_type = field_type;
    access.field_flags = field_flags;
    access.field_offset = field_offset;
    access.field_get = field_get;
    (access, thread_context())
}
fn root_count() -> usize {
    FAKE.with_borrow(|f| f.roots.values().sum())
}

#[test]
fn complete_entrypoint_roots_three_exact_objects_and_rechecks_without_name_or_readonly_calls() {
    let (access, ctx) = fixture();
    let attached = ctx.attach().unwrap();
    // SAFETY: exact initialized immutable authored tables/body and exclusive synchronous context.
    let context = unsafe { access.capture_numeric_format_context(&attached, api()) }.unwrap();
    assert_eq!(context.provider().target().unwrap().as_ptr() as usize, PROVIDER);
    assert_eq!(root_count(), 3);
    FAKE.with_borrow(|f| {
        assert_eq!(f.calls, [THREAD_GET, CULTURE_GET, INFO_GET]);
        assert_eq!(f.field_reads, 1);
        assert_eq!(f.root_attempts, 4);
    });
    // SAFETY: same synchronous admitted fake context remains unchanged.
    unsafe { access.recheck_numeric_format_context(&attached, api(), &context) }.unwrap();
    assert_eq!(root_count(), 3);
    drop(context);
    assert_eq!(root_count(), 0);
}
#[test]
fn derived_culture_tripwire_and_null_getter_results_refuse_before_later_bodies() {
    for (mode, calls) in [(2, 2), (3, 2), (4, 3)] {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        FAKE.with_borrow_mut(|f| f.mode = mode);
        // SAFETY: fake returns deliberately unsupported objects under valid runtime lifetimes.
        assert!(unsafe { access.capture_numeric_format_context(&attached, api()) }.is_err());
        FAKE.with_borrow(|f| assert_eq!(f.calls.len(), calls));
        assert_eq!(root_count(), 0);
    }
}
#[test]
fn invalid_exact_class_or_getter_metadata_is_bounded_and_never_invokes_body() {
    for mode in [1, 10, 11, 12, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25] {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        FAKE.with_borrow_mut(|f| f.mode = mode);
        // SAFETY: fake metadata is address-valid and modeled contract rejection is nonmutating.
        assert!(unsafe { access.capture_numeric_format_context(&attached, api()) }.is_err(), "mode{mode}");
        FAKE.with_borrow(|f| {
            assert!(f.calls.is_empty());
            assert!(f.metadata < 10_000);
            assert_eq!(f.field_reads, 0);
        });
        assert_eq!(root_count(), 0);
    }
}
#[test]
fn field_owner_type_storage_validation_precedes_any_field_copy_and_managed_body() {
    for mode in [6, 7, 8, 9, 18] {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        FAKE.with_borrow_mut(|f| f.mode = mode);
        // SAFETY: one deliberate live metadata incompatibility.
        assert!(unsafe { access.capture_numeric_format_context(&attached, api()) }.is_err());
        FAKE.with_borrow(|f| {
            assert!(f.calls.is_empty());
            assert_eq!(f.field_reads, 0);
        });
        assert_eq!(root_count(), 0);
    }
}
#[test]
fn cached_provider_null_or_mismatch_is_not_silently_replaced() {
    for mode in [5, 19] {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        FAKE.with_borrow_mut(|f| f.mode = mode);
        // SAFETY: malformed fake observation; no actual context mutation or managed virtual body.
        assert!(matches!(
            unsafe { access.capture_numeric_format_context(&attached, api()) },
            Err(Error::Changed(NumericFormatContextPart::Provider))
        ));
        assert_eq!(root_count(), 0);
    }
}
#[test]
fn every_getter_exception_and_each_return_or_child_root_failure_releases_owned_roots() {
    for failing_call in 1..=3 {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        FAKE.with_borrow_mut(|f| f.exception_call = failing_call);
        // SAFETY: authored checked invoke supplies a rooted managed exception result.
        assert!(matches!(unsafe { access.capture_numeric_format_context(&attached, api()) }, Err(Error::GetterException(_))));
        FAKE.with_borrow(|f| assert_eq!(f.calls.len(), failing_call));
        assert_eq!(root_count(), 0);
    }
    for fail_root in 1..=4 {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        FAKE.with_borrow_mut(|f| f.fail_root = fail_root);
        // SAFETY: authored root API reports allocation failure, preserving the checked-call failure path.
        assert!(matches!(unsafe { access.capture_numeric_format_context(&attached, api()) }, Err(Error::Runtime(_))));
        assert_eq!(root_count(), 0);
    }
}
#[test]
fn foreign_attachment_and_lost_root_recheck_refuse_before_new_metadata_or_invocation() {
    let (access, ctx) = fixture();
    let attached = ctx.attach().unwrap();
    let other = thread_context();
    let foreign = other.attach().unwrap();
    // SAFETY: valid exact fake context.
    let context = unsafe { access.capture_numeric_format_context(&attached, api()) }.unwrap();
    let baseline = FAKE.with_borrow(|f| (f.metadata, f.calls.len(), f.field_reads, f.root_attempts));
    // SAFETY: deliberately foreign attachment must be rejected before metadata/body use.
    assert!(matches!(
        unsafe { access.recheck_numeric_format_context(&foreign, api(), &context) },
        Err(Error::Contract(_))
    ));
    FAKE.with_borrow_mut(|f| f.lost = THREAD_OBJECT);
    // SAFETY: valid fake handle with an unavailable target must be rejected before new work.
    assert!(matches!(
        unsafe { access.recheck_numeric_format_context(&attached, api(), &context) },
        Err(Error::Runtime(_))
    ));
    FAKE.with_borrow(|f| assert_eq!((f.metadata, f.calls.len(), f.field_reads, f.root_attempts), baseline));
    drop(context);
    assert_eq!(root_count(), 0);
}
#[test]
fn observed_identity_change_stops_recapture_before_later_getters_without_aba_claim() {
    for (part, new_calls) in [
        (NumericFormatContextPart::Thread, 1),
        (NumericFormatContextPart::Culture, 2),
        (NumericFormatContextPart::Provider, 3),
    ] {
        let (access, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        // SAFETY: valid exact fake context.
        let context = unsafe { access.capture_numeric_format_context(&attached, api()) }.unwrap();
        // Model a separately established fresh observation scope, not mid-call mutation.
        FAKE.with_borrow_mut(|f| f.changed = Some(part));
        // SAFETY: context recapture is diagnostic only under new stable authored scope.
        assert_eq!(unsafe { access.recheck_numeric_format_context(&attached, api(), &context) }, Err(Error::Changed(part)));
        FAKE.with_borrow(|f| assert_eq!(f.calls.len(), 3 + new_calls));
        assert_eq!(root_count(), 3);
        drop(context);
        assert_eq!(root_count(), 0);
    }
}
