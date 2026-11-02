//! The narrow lookup adapter runs against real attachment/root ownership with authored metadata and invocation.
use super::*;
use crate::game::test_fakes::{self, token};
use std::{cell::Cell, ffi::c_char};

const SOURCE: usize = 0x1000;
const ANSWER: usize = 0x2000;
const CHILD: usize = 0x3000;
const SOURCE_CLASS: usize = 0x100;
const BOOLEAN: usize = 0x200;
const PAYLOAD: usize = 0x300;
const BOOL_TYPE: usize = 0x20;
const BYTE_TYPE: usize = 0x21;
const INT_TYPE: usize = 0x22;
const OUT_TYPE: usize = 0x23;
const METHOD: usize = 0x500;
thread_local! {
    static MODE: Cell<u8> = const { Cell::new(0) };
    static BOOL: Cell<u8> = const { Cell::new(1) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
}
const SPEC: MethodSpec = MethodSpec {
    class: c"source",
    method: c"Lookup",
    instance: true,
    params: &["System.Byte", "System.Int32", "Payload&"],
    returns: "System.Boolean",
};
unsafe extern "C-unwind" fn methods(owner: Class, iterator: *mut Ptr) -> Method {
    assert_eq!(owner as usize, SOURCE_CLASS);
    // SAFETY: MetadataApi gives each iterator its own live pointer-sized slot.
    unsafe {
        if (*iterator).is_null() {
            *iterator = token(1);
            token(METHOD)
        } else {
            std::ptr::null()
        }
    }
}
unsafe extern "C-unwind" fn name(_: Method) -> *const c_char {
    c"Lookup".as_ptr()
}
unsafe extern "C-unwind" fn count(_: Method) -> u32 {
    3
}
unsafe extern "C-unwind" fn parameter(_: Method, index: u32) -> Type {
    token(match index {
        0 => BYTE_TYPE,
        1 => INT_TYPE,
        _ if MODE.get() == 4 => INT_TYPE,
        _ => OUT_TYPE,
    })
}
unsafe extern "C-unwind" fn returns(_: Method) -> Type {
    token(BOOL_TYPE)
}
unsafe extern "C-unwind" fn instance(_: Method) -> bool {
    true
}
unsafe extern "C-unwind" fn no_method(_: Method) -> bool {
    false
}
unsafe extern "C-unwind" fn flags(_: Method, _: *mut u32) -> u32 {
    0
}
unsafe extern "C-unwind" fn byref(ty: Type) -> bool {
    ty as usize == OUT_TYPE
}
unsafe extern "C-unwind" fn type_name(ty: Type) -> *mut c_char {
    match ty as usize {
        BOOL_TYPE => c"System.Boolean",
        BYTE_TYPE => c"System.Byte",
        INT_TYPE => c"System.Int32",
        _ => c"Payload&",
    }
    .as_ptr()
    .cast_mut()
}
unsafe extern "C-unwind" fn free(_: Ptr) {}
unsafe extern "C-unwind" fn type_class(ty: Type) -> Class {
    token(if ty as usize == BOOL_TYPE { BOOLEAN } else { PAYLOAD })
}
unsafe extern "C-unwind" fn value(class: *const raw::Class) -> bool {
    class as usize == BOOLEAN
}
unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
    test_fakes::rooted(object as usize);
    token(match object as usize {
        SOURCE => SOURCE_CLASS,
        ANSWER if MODE.get() != 3 => BOOLEAN,
        CHILD if MODE.get() == 5 => 0x999,
        _ => PAYLOAD,
    })
}
unsafe extern "C-unwind" fn assignable(a: Class, b: Class) -> bool {
    a == b
}
unsafe extern "C-unwind" fn class_from_name(_: *const c_void, _: *const c_char, name: *const c_char) -> Class {
    // SAFETY: MetadataApi passes its static System.Boolean lookup name.
    assert_eq!(unsafe { CStr::from_ptr(name) }, c"Boolean");
    token(BOOLEAN)
}
unsafe extern "C-unwind" fn declaring(_: Method) -> Class {
    token(SOURCE_CLASS)
}
unsafe extern "C-unwind" fn unbox(_: *mut raw::Object) -> Ptr {
    BOOL.with(|value| value.as_ptr().cast())
}
unsafe extern "C-unwind" fn invoke(method: Method, receiver: Ptr, args: *mut Ptr, exception: *mut *mut raw::Exception) -> *mut raw::Object {
    assert_eq!((method as usize, receiver as usize), (METHOD, SOURCE));
    test_fakes::rooted(SOURCE);
    CALLS.set(CALLS.get() + 1);
    // SAFETY: the complete signature fixes one Byte, one Int32 and a writable pointer-sized output slot.
    unsafe {
        assert_eq!((*args).cast::<u8>().read(), 9);
        assert_eq!((*args.add(1)).cast::<i32>().read(), -10);
        (*args.add(2)).cast::<*mut raw::Object>().write(token(CHILD));
        if MODE.get() == 2 {
            *exception = token(0x4000);
        }
    }
    token(ANSWER)
}
fn fixture() -> (RuntimeAccess, raw::InvocationApi) {
    let mut access = RuntimeAccess::test_stubs();
    access.class_methods = methods;
    access.method_name = name;
    access.method_params = count;
    access.method_param = parameter;
    access.method_return = returns;
    access.method_instance = instance;
    access.method_generic = no_method;
    access.method_inflated = no_method;
    access.method_flags = flags;
    access.type_byref = byref;
    access.type_name = type_name;
    access.runtime_free = free;
    access.type_class = type_class;
    access.class_value = value;
    access.object_class = object_class;
    access.assignable = assignable;
    access.corlib = test_fakes::corlib;
    access.class_from_name = class_from_name;
    let api = raw::InvocationApi {
        runtime_invoke: invoke,
        method_get_class: declaring,
        object_unbox: unbox,
        method_get_return_type: returns,
        method_is_generic: no_method,
        method_is_inflated: no_method,
        method_is_instance: instance,
        method_get_param_count: count,
        method_get_param: parameter,
        method_get_flags: flags,
        type_is_byref: byref,
        class_from_type: type_class,
        class_is_valuetype: value,
        class_is_assignable_from: assignable,
        object_get_class: object_class,
        get_corlib: test_fakes::corlib,
        class_from_name,
        type_get_type: TestExportStub::test_stub(),
        class_is_generic: TestExportStub::test_stub(),
        class_is_inflated: TestExportStub::test_stub(),
        object_get_virtual_method: TestExportStub::test_stub(),
        class_is_enum: TestExportStub::test_stub(),
        class_enum_basetype: TestExportStub::test_stub(),
        class_value_size: TestExportStub::test_stub(),
    };
    (access, api)
}
#[test]
fn lookup_checks_the_full_signature_preserves_scalar_widths_and_roots_or_refuses_each_result() {
    let context = test_fakes::thread();
    let attached = context.attach().unwrap();
    // SAFETY: authored object tokens are handled only by the fake exports; no game memory is touched.
    let source = unsafe { attached.root(NonNull::new(token(SOURCE)).unwrap()) }.unwrap();
    let (access, api) = fixture();
    for mode in 0..6 {
        MODE.set(mode);
        BOOL.set(u8::from(mode != 1));
        let mut byte = 9u8;
        let mut integer = -10i32;
        let before = CALLS.get();
        // SAFETY: exact declared primitive inputs and the source-owned out reference, all retained synchronously.
        let result = unsafe { access.field_lookup(&attached, api, &source, &SPEC, &mut [(&raw mut byte).cast(), (&raw mut integer).cast()]) };
        assert_eq!((byte, integer), (9, -10));
        match mode {
            0 => {
                let root = result.unwrap().unwrap();
                assert_eq!(target(&root).unwrap().as_ptr() as usize, CHILD);
                assert_eq!(test_fakes::roots(), 2);
            }
            1 => assert!(result.unwrap().is_none()),
            _ => assert!(result.is_err()),
        }
        assert_eq!(CALLS.get(), before + usize::from(mode != 4), "bad metadata cannot enter the body");
        assert_eq!(test_fakes::roots(), 1, "temporary output, return and exception roots are released");
    }
    drop(source);
    assert_eq!(test_fakes::roots(), 0);
}
