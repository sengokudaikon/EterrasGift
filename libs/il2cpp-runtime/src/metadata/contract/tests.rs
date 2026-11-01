use super::*;
use crate::{
    metadata::Type,
    test_fakes::{corlib, token},
};
use std::{cell::Cell, ffi::c_char};

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(2) };
    static WRONG_RETURN: Cell<bool> = const { Cell::new(false) };
    static BY_REF: Cell<bool> = const { Cell::new(false) };
}
unsafe extern "C-unwind" fn methods(_: Class, iter: *mut Ptr) -> Method {
    // SAFETY: the metadata iterator owns this initialized pointer-sized slot.
    let index = unsafe { *iter as usize };
    // SAFETY: same iterator slot; each non-null token identifies immutable authored metadata.
    unsafe { *iter = token(index + 1) };
    if index < COUNT.get() { token(10 + index) } else { std::ptr::null() }
}
unsafe extern "C-unwind" fn name(_: Method) -> *const c_char {
    c"cost".as_ptr()
}
unsafe extern "C-unwind" fn count(_: Method) -> u32 {
    5
}
unsafe extern "C-unwind" fn param(method: Method, index: u32) -> Type {
    token(match index {
        0 if method as usize == 10 => 5,
        0 => 1,
        1 => 2,
        2 | 4 => 3,
        3 => 4,
        _ => unreachable!(),
    })
}
unsafe extern "C-unwind" fn returns(_: Method) -> Type {
    token(if WRONG_RETURN.get() { 6 } else { 2 })
}
unsafe extern "C-unwind" fn instance(_: Method) -> bool {
    true
}
unsafe extern "C-unwind" fn no(_: Method) -> bool {
    false
}
unsafe extern "C-unwind" fn flags(_: Method, _: *mut u32) -> u32 {
    0
}
unsafe extern "C-unwind" fn byref(ty: Type) -> bool {
    ty as usize == 1 && BY_REF.get()
}
unsafe extern "C-unwind" fn type_name(ty: Type) -> *mut c_char {
    match ty as usize {
        1 if BY_REF.get() => c"Kind&",
        1 => c"Kind",
        2 => c"System.Single",
        3 => c"System.Boolean",
        4 => c"Modifier",
        5 => c"KindId",
        6 => c"System.Int32",
        _ => unreachable!(),
    }
    .as_ptr()
    .cast_mut()
}
unsafe extern "C-unwind" fn free(_: Ptr) {}

// Same-name, same-arity overloads: KindId comes before the intended Kind overload.
#[test]
fn exact_overload_is_selected_and_ambiguity_or_wrong_return_is_refused() {
    let mut access = MetadataApi::test_stubs();
    access.class_methods = methods;
    access.method_name = name;
    access.method_params = count;
    access.method_param = param;
    access.method_return = returns;
    access.method_instance = instance;
    access.method_generic = no;
    access.method_inflated = no;
    access.method_flags = flags;
    access.type_byref = byref;
    access.type_name = type_name;
    access.runtime_free = free;
    let spec = MethodSpec {
        class: c"Owner",
        method: c"cost",
        instance: true,
        params: &["Kind", "System.Single", "System.Boolean", "Modifier", "System.Boolean"],
        returns: "System.Single",
    };
    let resolve = || {
        // SAFETY: authored immutable class/method/type tokens, accepted by the exact matching fake export signatures.
        unsafe { access.hook_method_contract(token(1), &spec) }
    };
    COUNT.set(2);
    WRONG_RETURN.set(false);
    BY_REF.set(false);
    assert_eq!(resolve().unwrap() as usize, 11);
    COUNT.set(1); // only the KindId overload remains
    assert!(resolve().is_err());
    COUNT.set(3); // two distinct exact Kind matches are ambiguous
    assert!(resolve().is_err());
    COUNT.set(2);
    WRONG_RETURN.set(true);
    assert!(resolve().is_err());
    WRONG_RETURN.set(false);
    BY_REF.set(true);
    assert!(resolve().is_err(), "a by-reference overload cannot satisfy a value-parameter contract");
    let refs = MethodSpec {
        params: &["Kind&", "System.Single", "System.Boolean", "Modifier", "System.Boolean"],
        ..spec
    };
    // SAFETY: same authored metadata, with an explicit by-reference contract.
    assert_eq!(unsafe { access.hook_method_contract(token(1), &refs) }.unwrap() as usize, 11);
    BY_REF.set(false);
    // SAFETY: same authored class/method tokens, deliberately mismatched value versus by-reference contract.
    assert!(unsafe { access.hook_method_contract(token(1), &refs) }.is_err());
}

const OWNER: usize = 0x1000;
const FIELD: usize = 0x2000;
const INT32: usize = 0x3000;
thread_local! {
    static MISSING: Cell<bool> = const { Cell::new(false) };
    static BYREF: Cell<bool> = const { Cell::new(false) };
    static CODE: Cell<i32> = const { Cell::new(raw::type_code::I4) };
    static TYPE_CLASS: Cell<usize> = const { Cell::new(INT32) };
    static DECLARING: Cell<usize> = const { Cell::new(OWNER) };
    static FLAGS: Cell<i32> = const { Cell::new(0) };
    static OFFSET: Cell<usize> = const { Cell::new(144) };
}
unsafe extern "C-unwind" fn class_from_name(_: *const std::ffi::c_void, namespace: *const c_char, name: *const c_char) -> Class {
    // SAFETY: system_class supplies static nul-terminated names.
    assert_eq!(unsafe { (CStr::from_ptr(namespace), CStr::from_ptr(name)) }, (c"System", c"Int32"));
    token(INT32)
}
unsafe extern "C-unwind" fn class_field(owner: Class, _: *const c_char) -> Ptr {
    assert_eq!(owner as usize, OWNER);
    if MISSING.get() { std::ptr::null_mut() } else { token(FIELD) }
}
unsafe extern "C-unwind" fn class_parent(_: Class) -> Class {
    std::ptr::null_mut()
}
unsafe extern "C-unwind" fn field_type(_: Ptr) -> Type {
    token(0x5000)
}
unsafe extern "C-unwind" fn type_byref(_: Type) -> bool {
    BYREF.get()
}
unsafe extern "C-unwind" fn type_code(_: Type) -> i32 {
    CODE.get()
}
unsafe extern "C-unwind" fn type_class(_: Type) -> Class {
    token(TYPE_CLASS.get())
}
unsafe extern "C-unwind" fn field_parent(_: Ptr) -> Class {
    token(DECLARING.get())
}
unsafe extern "C-unwind" fn field_flags(_: Ptr) -> i32 {
    FLAGS.get()
}
unsafe extern "C-unwind" fn field_offset(_: Ptr) -> usize {
    OFFSET.get()
}
fn reset() {
    MISSING.set(false);
    BYREF.set(false);
    CODE.set(raw::type_code::I4);
    TYPE_CLASS.set(INT32);
    DECLARING.set(OWNER);
    FLAGS.set(0);
    OFFSET.set(144);
}

#[test]
fn mutable_field_is_exactly_the_declared_writable_instance_primitive_inside_the_object() {
    let mut api = MetadataApi::test_stubs();
    api.corlib = corlib;
    api.class_from_name = class_from_name;
    api.class_field = class_field;
    api.class_parent = class_parent;
    api.field_type = field_type;
    api.type_byref = type_byref;
    api.type_code = type_code;
    api.type_class = type_class;
    api.field_parent = field_parent;
    api.field_flags = field_flags;
    api.field_offset = field_offset;
    // SAFETY: authored metadata tokens only; no object is read or written.
    let field = || unsafe { api.mutable_field(token(OWNER), c"value", c"Int32", raw::type_code::I4) };
    assert_eq!(field().unwrap().unwrap() as usize, FIELD);
    for offset in [16, 16384] {
        OFFSET.set(offset);
        assert!(field().unwrap().is_some(), "offset {offset}");
    }
    let faults: &[fn()] = &[
        || BYREF.set(true),
        || CODE.set(raw::type_code::I8),
        || TYPE_CLASS.set(INT32 + 1),
        || DECLARING.set(OWNER + 1),
        || FLAGS.set(0x10),
        || FLAGS.set(0x20),
        || FLAGS.set(0x40),
        || FLAGS.set(0x70),
        || OFFSET.set(0),
        || OFFSET.set(15),
        || OFFSET.set(16385),
        || OFFSET.set(usize::MAX),
    ];
    for (case, fault) in faults.iter().enumerate() {
        reset();
        fault();
        assert!(matches!(field(), Ok(None)), "case {case}");
    }
    reset();
    MISSING.set(true);
    assert!(field().is_err());
}

#[test]
fn code_window_stops_at_its_limit_the_module_end_or_the_next_declared_method() {
    static CODE_POINTERS: [usize; 3] = [0x1000, 0x1100, 0x0800];
    unsafe extern "C-unwind" fn declared(_: Class, iter: *mut Ptr) -> Method {
        // SAFETY: the metadata iterator owns this initialized pointer-sized slot.
        let index = unsafe { *iter as usize };
        // SAFETY: same iterator slot.
        unsafe { *iter = token(index + 1) };
        CODE_POINTERS.get(index).map_or(std::ptr::null(), |word| std::ptr::from_ref(word).cast())
    }
    let mut api = MetadataApi::test_stubs();
    api.class_methods = declared;
    // SAFETY: each authored method token points at a live word holding its code pointer; no code is read.
    let end = |limit, module_end| unsafe { api.code_end(token(OWNER), 0x1000, limit, module_end) }.unwrap();
    assert_eq!(end(0x80, 0x9000), 0x1080);
    assert_eq!(end(0x200, 0x1050), 0x1050);
    assert_eq!(end(0x200, 0x9000), 0x1100);
}
