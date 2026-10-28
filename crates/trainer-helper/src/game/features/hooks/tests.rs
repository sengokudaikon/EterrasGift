//! Same-name, same-arity overloads from BaseMana: AbilityID comes before the intended Ability overload.
use super::*;
use crate::game::{Type, test_fakes::token};
use std::{cell::Cell, ffi::c_char};

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(2) };
    static WRONG_RETURN: Cell<bool> = const { Cell::new(false) };
}
unsafe extern "C-unwind" fn methods(_: Class, iter: *mut Ptr) -> Method {
    // SAFETY: the metadata iterator owns this initialized pointer-sized slot.
    let index = unsafe { *iter as usize };
    // SAFETY: same iterator slot; each non-null token identifies immutable authored metadata.
    unsafe { *iter = token(index + 1) };
    if index < COUNT.get() { token(10 + index) } else { std::ptr::null() }
}
unsafe extern "C-unwind" fn name(_: Method) -> *const c_char {
    c"getManaCost".as_ptr()
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
unsafe extern "C-unwind" fn byref(_: Type) -> bool {
    false
}
unsafe extern "C-unwind" fn type_name(ty: Type) -> *mut c_char {
    match ty as usize {
        1 => c"Ability",
        2 => c"System.Single",
        3 => c"System.Boolean",
        4 => c"AbilityMutator",
        5 => c"AbilityID",
        6 => c"System.Int32",
        _ => unreachable!(),
    }
    .as_ptr()
    .cast_mut()
}
unsafe extern "C-unwind" fn free(_: Ptr) {}

#[test]
fn exact_overload_is_selected_and_ambiguity_or_wrong_return_is_refused() {
    let mut access = RuntimeAccess::test_stubs();
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
        class: c"BaseMana",
        method: c"getManaCost",
        instance: true,
        params: &["Ability", "System.Single", "System.Boolean", "AbilityMutator", "System.Boolean"],
        returns: "System.Single",
    };
    let resolve = || {
        // SAFETY: authored immutable class/method/type tokens, accepted by the exact matching fake export signatures.
        unsafe { access.hook_method_contract(token(1), &spec) }
    };
    COUNT.set(2);
    WRONG_RETURN.set(false);
    assert_eq!(resolve().unwrap() as usize, 11);
    COUNT.set(1); // only the AbilityID overload remains
    assert!(resolve().is_err());
    COUNT.set(3); // two distinct exact Ability matches are ambiguous
    assert!(resolve().is_err());
    COUNT.set(2);
    WRONG_RETURN.set(true);
    assert!(resolve().is_err());
}
