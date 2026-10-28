//! Fake metadata/runtime tests for rejection, storage and rooting; not Unity compatibility evidence.
use eg_il2cpp_runtime::raw::{self, Api, InvocationApi};
use eg_il2cpp_runtime::{Argument, CheckedMethod, InvocationContext, InvocationOutcome, InvokeError, ReturnValue, RuntimeError, Scalar, ThreadContext, ValueTypeContract};
use static_assertions::assert_not_impl_any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{NonNull, null_mut};

assert_not_impl_any!(InvocationContext<'static, 'static>: Send, Sync);
assert_not_impl_any!(CheckedMethod<'static, 'static, 'static>: Send, Sync);
assert_not_impl_any!(InvocationOutcome<'static, 'static>: Send, Sync);

const OWNER: usize = 0x100;
const DERIVED: usize = 0x101;
const OTHER: usize = 0x102;
const STRUCT: usize = 0x103;
const ARRAY: usize = 0x104;
const ELEMENT: usize = 0x105;
const ENUM: usize = 0x106;
const OTHER_ENUM: usize = 0x107;
const ENUM_BASE: usize = 0x108;
const CORLIB: usize = 0x109;

fn pointer<T>(id: usize) -> *mut T {
    std::ptr::without_provenance_mut(id)
}

#[derive(Clone, Copy)]
struct Type {
    code: i32,
    by_ref: bool,
    class: usize,
}
#[derive(Clone, Copy, Default)]
struct Class {
    parent: usize,
    value: bool,
    generic: bool,
    inflated: bool,
    enumeration: bool,
    enum_base: usize,
    size: i32,
    alignment: u32,
}
#[derive(Clone, Copy, Default)]
enum Action {
    #[default]
    Nothing,
    BooleanThree,
    ThrowAfterWrite,
    ExpectObject(usize),
    ExpectValueReceiver(usize),
    ExpectEnumPair(i32, i32),
}
#[derive(Clone)]
struct Method {
    class: usize,
    instance: bool,
    generic: bool,
    inflated: bool,
    flags: u32,
    parameters: Vec<usize>,
    returns: usize,
    action: Action,
    result: usize,
    exception: usize,
}
struct Object {
    class: usize,
    payload: Box<u64>,
}
#[derive(Default)]
struct Fake {
    classes: BTreeMap<usize, Class>,
    types: BTreeMap<usize, Type>,
    methods: BTreeMap<usize, Method>,
    objects: BTreeMap<usize, Object>,
    handles: BTreeMap<usize, usize>,
    next: usize,
    invocations: usize,
    fail_root: bool,
    result_needs_root: usize,
    unboxes: usize,
    corlib: usize,
    int32_class: usize,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }

unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
    pointer(1)
}
unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
    pointer(2)
}
unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
    panic!("engine attachment already exists")
}
unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
    panic!("must not detach borrowed engine attachment")
}
unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> *mut c_void {
    assert!(!pinned);
    FAKE.with_borrow_mut(|f| {
        assert!(f.objects.contains_key(&object.addr()));
        if f.fail_root {
            return null_mut();
        }
        f.next += 1;
        f.handles.insert(f.next, object.addr());
        pointer(f.next)
    })
}
unsafe extern "C-unwind" fn free(handle: *mut c_void) {
    FAKE.with_borrow_mut(|f| assert!(f.handles.remove(&handle.addr()).is_some()));
}
unsafe extern "C-unwind" fn target(handle: *mut c_void) -> *mut raw::Object {
    FAKE.with_borrow(|f| pointer(f.handles[&handle.addr()]))
}
unsafe extern "C-unwind" fn method_return(method: *const raw::MethodInfo) -> *const raw::Type {
    FAKE.with_borrow(|f| pointer(f.methods[&method.addr()].returns))
}
unsafe extern "C-unwind" fn method_class(method: *const raw::MethodInfo) -> *mut raw::Class {
    FAKE.with_borrow(|f| pointer(f.methods[&method.addr()].class))
}
macro_rules! method_boolean {
    ($name:ident, $field:ident) => {
        unsafe extern "C-unwind" fn $name(method: *const raw::MethodInfo) -> bool {
            FAKE.with_borrow(|f| f.methods[&method.addr()].$field)
        }
    };
}
method_boolean!(method_generic, generic);
method_boolean!(method_inflated, inflated);
method_boolean!(method_instance, instance);
unsafe extern "C-unwind" fn method_flags(method: *const raw::MethodInfo, implementation: *mut u32) -> u32 {
    // SAFETY: the wrapper provides initialized writable u32 storage.
    unsafe { *implementation = 0 };
    FAKE.with_borrow(|f| f.methods[&method.addr()].flags)
}
unsafe extern "C-unwind" fn parameter_count(method: *const raw::MethodInfo) -> u32 {
    FAKE.with_borrow(|f| f.methods[&method.addr()].parameters.len() as u32)
}
unsafe extern "C-unwind" fn parameter(method: *const raw::MethodInfo, index: u32) -> *const raw::Type {
    FAKE.with_borrow(|f| pointer(f.methods[&method.addr()].parameters[index as usize]))
}
unsafe extern "C-unwind" fn type_code(value: *const raw::Type) -> i32 {
    FAKE.with_borrow(|f| f.types[&value.addr()].code)
}
unsafe extern "C-unwind" fn type_by_ref(value: *const raw::Type) -> bool {
    FAKE.with_borrow(|f| f.types[&value.addr()].by_ref)
}
unsafe extern "C-unwind" fn class_from_type(value: *const raw::Type) -> *mut raw::Class {
    FAKE.with_borrow(|f| pointer(f.types[&value.addr()].class))
}
macro_rules! class_boolean {
    ($name:ident, $field:ident) => {
        unsafe extern "C-unwind" fn $name(class: *const raw::Class) -> bool {
            FAKE.with_borrow(|f| f.classes[&class.addr()].$field)
        }
    };
}
class_boolean!(class_value, value);
class_boolean!(class_generic, generic);
class_boolean!(class_inflated, inflated);
class_boolean!(class_enum, enumeration);
unsafe extern "C-unwind" fn get_corlib() -> *const c_void {
    FAKE.with_borrow(|f| pointer(f.corlib))
}
unsafe extern "C-unwind" fn class_from_name(image: *const c_void, namespace: *const c_char, name: *const c_char) -> *mut raw::Class {
    // SAFETY: the checked enum validator supplies static NUL-terminated corlib names.
    let (namespace, name) = unsafe { (CStr::from_ptr(namespace), CStr::from_ptr(name)) };
    assert_eq!(namespace, c"System");
    assert_eq!(name, c"Int32");
    FAKE.with_borrow(|f| {
        assert_eq!(image.addr(), f.corlib);
        pointer(f.int32_class)
    })
}
unsafe extern "C-unwind" fn enum_base(class: *mut raw::Class) -> *const raw::Type {
    FAKE.with_borrow(|f| pointer(f.classes[&class.addr()].enum_base))
}
unsafe extern "C-unwind" fn value_size(class: *mut raw::Class, alignment: *mut u32) -> i32 {
    FAKE.with_borrow(|f| {
        let class = f.classes[&class.addr()];
        // SAFETY: the invocation validator supplies one writable alignment result.
        unsafe { alignment.write(class.alignment) };
        class.size
    })
}
unsafe extern "C-unwind" fn assignable(expected: *mut raw::Class, actual: *mut raw::Class) -> bool {
    FAKE.with_borrow(|f| actual == expected || f.classes[&actual.addr()].parent == expected.addr())
}
unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> *mut raw::Class {
    FAKE.with_borrow(|f| {
        if f.result_needs_root == object.addr() {
            assert!(f.handles.values().any(|&value| value == object.addr()), "root result before metadata query");
        }
        pointer(f.objects[&object.addr()].class)
    })
}
unsafe extern "C-unwind" fn unbox(object: *mut raw::Object) -> *mut c_void {
    FAKE.with_borrow_mut(|f| {
        f.unboxes += 1;
        std::ptr::from_mut(f.objects.get_mut(&object.addr()).unwrap().payload.as_mut()).cast()
    })
}
unsafe extern "C-unwind" fn virtual_method(_: *mut raw::Object, _: *const raw::MethodInfo) -> *const raw::MethodInfo {
    panic!("direct invocation must not silently resolve a virtual override")
}
unsafe extern "C-unwind" fn invoke(method: *const raw::MethodInfo, receiver: *mut c_void, arguments: *mut *mut c_void, exception: *mut *mut raw::Exception) -> *mut raw::Object {
    FAKE.with_borrow_mut(|f| {
        let method = f.methods[&method.addr()].clone();
        f.invocations += 1;
        // SAFETY: these actions correspond to the fake method's checked signature and only access
        // its provided receiver/argument storage. They intentionally exercise byte-valued bool.
        unsafe {
            *exception = pointer(method.exception);
            match method.action {
                Action::Nothing => {}
                Action::BooleanThree => {
                    assert_eq!(*(*arguments).cast::<u8>(), 1);
                    *(*arguments).cast::<u8>() = 3;
                }
                Action::ThrowAfterWrite => {
                    *(*arguments).cast::<i32>() = -91;
                }
                Action::ExpectObject(expected) => {
                    assert_eq!((*arguments).addr(), expected);
                }
                Action::ExpectValueReceiver(expected) => {
                    assert_eq!(receiver.addr(), expected);
                }
                Action::ExpectEnumPair(first, second) => {
                    assert_eq!((*arguments).cast::<i32>().read(), first);
                    assert_eq!((*arguments.add(1)).cast::<i32>().read(), second);
                }
            }
        }
        f.result_needs_root = if method.exception != 0 { method.exception } else { method.result };
        pointer(method.result)
    })
}

fn api() -> Api {
    Api {
        domain_get: domain,
        thread_current: current,
        thread_attach: attach,
        thread_detach: detach,
        gchandle_new: root,
        gchandle_free: free,
        gchandle_get_target: target,
    }
}
fn invocation_api() -> InvocationApi {
    InvocationApi {
        runtime_invoke: invoke,
        method_get_return_type: method_return,
        method_get_class: method_class,
        method_is_generic: method_generic,
        method_is_inflated: method_inflated,
        method_is_instance: method_instance,
        method_get_param_count: parameter_count,
        method_get_param: parameter,
        method_get_flags: method_flags,
        type_get_type: type_code,
        type_is_byref: type_by_ref,
        class_from_type,
        class_is_valuetype: class_value,
        class_is_generic: class_generic,
        class_is_inflated: class_inflated,
        class_is_assignable_from: assignable,
        object_get_class: object_class,
        object_unbox: unbox,
        object_get_virtual_method: virtual_method,
        get_corlib,
        class_from_name,
        class_is_enum: class_enum,
        class_enum_basetype: enum_base,
        class_value_size: value_size,
    }
}
fn context() -> ThreadContext {
    FAKE.with_borrow_mut(|f| {
        assert!(f.handles.is_empty());
        *f = Fake {
            next: 0x1000,
            corlib: CORLIB,
            int32_class: 8,
            ..Fake::default()
        };
        for id in [OWNER, OTHER, ARRAY, ELEMENT] {
            f.classes.insert(id, Class::default());
        }
        f.classes.insert(
            DERIVED,
            Class {
                parent: OWNER,
                ..Class::default()
            },
        );
        f.classes.insert(
            STRUCT,
            Class {
                value: true,
                size: 8,
                alignment: 8,
                ..Class::default()
            },
        );
        for id in [ENUM, OTHER_ENUM] {
            f.classes.insert(
                id,
                Class {
                    value: true,
                    enumeration: true,
                    enum_base: ENUM_BASE,
                    size: 4,
                    alignment: 4,
                    ..Class::default()
                },
            );
        }
        f.types.insert(ENUM_BASE, Type { code: 8, by_ref: false, class: 8 });
        for code in [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 0x18, 0x19] {
            f.classes.insert(code, Class { value: true, ..Class::default() });
        }
        f.classes.get_mut(&8).unwrap().size = 4;
        f.classes.get_mut(&8).unwrap().alignment = 4;
    });
    // SAFETY: thread-local fake engine attachment and correctly typed exports remain valid throughout the test.
    unsafe { ThreadContext::new(api()) }
}
fn method(params: &[(i32, bool)], returns: (i32, bool)) -> NonNull<raw::MethodInfo> {
    FAKE.with_borrow_mut(|f| {
        let mut register = |(code, by_ref)| {
            f.next += 1;
            let class = match code {
                0x12 | 0x1c | 0x0e => OWNER,
                0x14 | 0x1d => ARRAY,
                0x11 | 0x55 => STRUCT,
                _ => code as usize,
            };
            f.types.insert(f.next, Type { code, by_ref, class });
            f.next
        };
        let parameters = params.iter().copied().map(&mut register).collect();
        let returns = register(returns);
        f.next += 1;
        f.methods.insert(
            f.next,
            Method {
                class: OWNER,
                instance: false,
                generic: false,
                inflated: false,
                flags: 0,
                parameters,
                returns,
                action: Action::Nothing,
                result: 0,
                exception: 0,
            },
        );
        NonNull::new(pointer(f.next)).unwrap()
    })
}
fn change_method(method: NonNull<raw::MethodInfo>, change: impl FnOnce(&mut Method)) {
    FAKE.with_borrow_mut(|f| change(f.methods.get_mut(&method.as_ptr().addr()).unwrap()));
}
fn object(class: usize, bits: u64) -> NonNull<raw::Object> {
    FAKE.with_borrow_mut(|f| {
        f.next += 1;
        f.objects.insert(f.next, Object { class, payload: Box::new(bits) });
        NonNull::new(pointer(f.next)).unwrap()
    })
}
fn calls() -> usize {
    FAKE.with_borrow(|f| f.invocations)
}
fn handles() -> usize {
    FAKE.with_borrow(|f| f.handles.len())
}

#[test]
fn closed_list_calls_check_receiver_argument_and_runtime_origin_before_invocation() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    let metadata = method(&[(0x12, false)], (1, false));
    FAKE.with_borrow_mut(|f| {
        f.classes.get_mut(&OWNER).unwrap().inflated = true;
        let parameter = f.methods[&metadata.as_ptr().addr()].parameters[0];
        f.types.get_mut(&parameter).unwrap().class = ELEMENT;
    });
    let value = object(ELEMENT, 0);
    change_method(metadata, |m| {
        m.instance = true;
        m.inflated = true;
        m.action = Action::ExpectObject(value.as_ptr().addr());
    });
    // SAFETY: exact fake closed class and registered reference-only method body.
    unsafe {
        let context = InvocationContext::new(&attached, invocation_api());
        assert!(matches!(context.bind(metadata), Err(InvokeError::GenericMethod)));
        let method = context.bind_with_closed_references(metadata, &[NonNull::new(pointer(OWNER)).unwrap()]).unwrap();
        let receiver = attached.root(object(OWNER, 0)).unwrap();
        let wrong = attached.root(object(OTHER, 0)).unwrap();
        let value = attached.root(value).unwrap();
        assert!(matches!(
            method.invoke(Some(&wrong), &mut [Argument::Reference(Some(&value))]),
            Err(InvokeError::ObjectTypeMismatch)
        ));
        assert!(matches!(
            method.invoke(Some(&receiver), &mut [Argument::Reference(Some(&wrong))]),
            Err(InvokeError::ObjectTypeMismatch)
        ));
        let other_runtime = ThreadContext::new(api());
        let other_attached = other_runtime.attach().unwrap();
        let foreign = other_attached.root(object(ELEMENT, 0)).unwrap();
        assert!(matches!(
            method.invoke(Some(&receiver), &mut [Argument::Reference(Some(&foreign))]),
            Err(InvokeError::ContextMismatch)
        ));
        assert!(matches!(
            method.invoke(Some(&receiver), &mut [Argument::Scalar(Scalar::I32(0))]),
            Err(InvokeError::ArgumentType { index: 0 })
        ));
        assert_eq!(calls(), 0);
        assert!(matches!(
            method.invoke(Some(&receiver), &mut [Argument::Reference(Some(&value))]).unwrap(),
            InvocationOutcome::Returned(ReturnValue::Void)
        ));
        assert_eq!(calls(), 1);
    }
    assert_eq!(handles(), 0);
}

#[test]
fn closed_reference_returns_are_rooted_and_checked_against_the_exact_allowlist() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    FAKE.with_borrow_mut(|f| f.classes.get_mut(&OTHER).unwrap().inflated = true);
    let good = method(&[], (0x15, false));
    let bad = method(&[], (0x15, false));
    let result = object(OTHER, 0);
    let wrong = object(ELEMENT, 0);
    for (metadata, result) in [(good, result), (bad, wrong)] {
        change_method(metadata, |m| m.result = result.as_ptr().addr());
        FAKE.with_borrow_mut(|f| {
            let returns = f.methods[&metadata.as_ptr().addr()].returns;
            f.types.get_mut(&returns).unwrap().class = OTHER;
        });
    }
    // SAFETY: fake closed reference return identities and no-op bodies; no payload layouts.
    unsafe {
        let context = InvocationContext::new(&attached, invocation_api());
        assert!(matches!(context.bind(good), Err(InvokeError::UnsupportedType { .. })));
        assert!(matches!(context.bind_with_closed_references(good, &[]), Err(InvokeError::UnsupportedType { .. })));
        let allowed = [NonNull::new(pointer(OTHER)).unwrap()];
        let good = context.bind_with_closed_references(good, &allowed).unwrap();
        let outcome = good.invoke(None, &mut []).unwrap();
        assert!(matches!(outcome, InvocationOutcome::Returned(ReturnValue::Reference(Some(_)))));
        assert_eq!(handles(), 1);
        drop(outcome);
        assert_eq!(handles(), 0);
        let bad = context.bind_with_closed_references(bad, &allowed).unwrap();
        assert!(matches!(bad.invoke(None, &mut []), Err(InvokeError::ObjectTypeMismatch)));
        assert_eq!(handles(), 0);
        assert_eq!(calls(), 2);
    }
}

#[test]
fn closed_reference_opt_in_rejects_open_generics_values_and_reference_byrefs() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    FAKE.with_borrow_mut(|f| {
        f.classes.get_mut(&OTHER).unwrap().generic = true;
        f.classes.get_mut(&STRUCT).unwrap().inflated = true;
        f.classes.get_mut(&OWNER).unwrap().inflated = true;
    });
    let metadata = method(&[], (1, false));
    let byref = method(&[(0x15, true)], (1, false));
    FAKE.with_borrow_mut(|f| {
        let parameter = f.methods[&byref.as_ptr().addr()].parameters[0];
        f.types.get_mut(&parameter).unwrap().class = OWNER;
    });
    // SAFETY: live fake metadata and queried-only no-op bodies.
    unsafe {
        let context = InvocationContext::new(&attached, invocation_api());
        for class in [OTHER, STRUCT, ELEMENT] {
            assert!(matches!(
                context.bind_with_closed_references(metadata, &[NonNull::new(pointer(class)).unwrap()]),
                Err(InvokeError::InvalidClosedReference)
            ));
        }
        assert!(matches!(
            context.bind_with_closed_references(byref, &[NonNull::new(pointer(OWNER)).unwrap()]),
            Err(InvokeError::UnsupportedType { .. })
        ));
    }
    assert_eq!(calls(), 0);
}

#[test]
fn rejects_unsupported_metadata_before_invocation() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: same thread-local fake runtime; fixture metadata/bodies obey the bounded contract.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    for (code, by_ref) in [
        (0x11, false),
        (0x55, false),
        (0x0f, false),
        (0x1b, false),
        (0x15, false),
        (0x13, false),
        (0x1e, false),
        (0x12, true),
        (0x1c, true),
        (0x1d, true),
    ] {
        let method = method(&[(code, by_ref)], (1, false));
        // SAFETY: live fake method metadata, valid fixture body.
        assert!(matches!(unsafe { context.bind(method) }, Err(InvokeError::UnsupportedType { .. })));
    }
    for returns in [(8, true), (0x12, true), (0x11, false), (0x0f, false)] {
        let method = method(&[], returns);
        // SAFETY: live fake method metadata, valid fixture body.
        assert!(matches!(unsafe { context.bind(method) }, Err(InvokeError::UnsupportedType { .. })));
    }
    for field in 0..3 {
        let method = method(&[], (1, false));
        change_method(method, |m| match field {
            0 => m.generic = true,
            1 => m.inflated = true,
            _ => m.flags = 0x0400,
        });
        // SAFETY: live fake metadata, queried only; rejected methods are never invoked.
        assert!(matches!(unsafe { context.bind(method) }, Err(InvokeError::GenericMethod | InvokeError::AbstractMethod)));
    }
    FAKE.with_borrow_mut(|f| f.classes.get_mut(&OWNER).unwrap().inflated = true);
    // SAFETY: live fake method metadata.
    assert!(matches!(unsafe { context.bind(method(&[], (1, false))) }, Err(InvokeError::GenericDeclaringType)));
    assert_eq!(calls(), 0);
}

#[test]
fn rejects_wrong_width_arity_byref_receiver_and_context_before_calling() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: same valid thread-local fake runtime.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    // SAFETY: live fake method with no-op body safe for all accepted arguments.
    let method = unsafe { context.bind(method(&[(8, false)], (1, false))) }.unwrap();
    assert!(matches!(method.invoke(None, &mut []), Err(InvokeError::ArgumentCount { .. })));
    assert!(matches!(
        method.invoke(None, &mut [Argument::Scalar(Scalar::U32(1))]),
        Err(InvokeError::ArgumentType { index: 0 })
    ));
    let mut value = Scalar::I32(1);
    assert!(matches!(
        method.invoke(None, &mut [Argument::ByRef(&mut value)]),
        Err(InvokeError::ArgumentType { index: 0 })
    ));
    // SAFETY: registered live fake object.
    let root = unsafe { attached.root(object(OWNER, 0)) }.unwrap();
    assert!(matches!(
        method.invoke(Some(&root), &mut [Argument::Scalar(Scalar::I32(1))]),
        Err(InvokeError::UnexpectedReceiver)
    ));
    // SAFETY: independent context borrows the same permanent fake engine attachment and cannot detach it.
    let other_runtime = unsafe { ThreadContext::new(api()) };
    let other_attached = other_runtime.attach().unwrap();
    // SAFETY: registered live fake object; intentionally rooted through a different context.
    let foreign_root = unsafe { other_attached.root(object(OWNER, 0)) }.unwrap();
    let reference_method = self::method(&[(0x12, false)], (1, false));
    // SAFETY: live fake reference method.
    let reference_method = unsafe { context.bind(reference_method) }.unwrap();
    assert!(matches!(
        reference_method.invoke(None, &mut [Argument::Reference(Some(&foreign_root))]),
        Err(InvokeError::ContextMismatch)
    ));
    assert_eq!(calls(), 0);
}

#[test]
fn byref_boolean_uses_byte_storage_and_normalizes_noncanonical_foreign_value() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: valid fake context and method, whose body writes a byte to Boolean storage.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[(2, true)], (2, false));
    let result = object(2, 3);
    change_method(metadata, |m| {
        m.action = Action::BooleanThree;
        m.result = result.as_ptr().addr();
    });
    // SAFETY: the body writes only the authorized Boolean byte and returns a valid fresh fake box.
    let method = unsafe { context.bind(metadata) }.unwrap();
    let mut value = Scalar::Boolean(true);
    let result = method.invoke(None, &mut [Argument::ByRef(&mut value)]).unwrap();
    assert_eq!(value, Scalar::Boolean(true));
    assert!(matches!(result, InvocationOutcome::Returned(ReturnValue::Scalar { value: Scalar::Boolean(true), .. })));
    assert_eq!(handles(), 1);
    drop(result);
    assert_eq!(handles(), 0);
}

#[test]
fn managed_exception_is_rooted_and_scalar_writes_are_copied_back_even_when_rooting_fails() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: valid thread-local runtime.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[(8, true)], (1, false));
    let exception = object(OWNER, 0);
    change_method(metadata, |m| {
        m.action = Action::ThrowAfterWrite;
        m.exception = exception.as_ptr().addr();
    });
    // SAFETY: fake body writes a checked i32 then returns a registered exception.
    let method = unsafe { context.bind(metadata) }.unwrap();
    let mut value = Scalar::I32(1);
    let outcome = method.invoke(None, &mut [Argument::ByRef(&mut value)]).unwrap();
    assert_eq!(value, Scalar::I32(-91));
    match outcome {
        InvocationOutcome::Exception(root) => assert_eq!(root.target(), Some(exception)),
        _ => panic!("expected rooted managed exception"),
    }
    assert_eq!(handles(), 0);
    FAKE.with_borrow_mut(|f| f.fail_root = true);
    value = Scalar::I32(22);
    assert!(matches!(
        method.invoke(None, &mut [Argument::ByRef(&mut value)]),
        Err(InvokeError::Runtime(RuntimeError::RootFailed))
    ));
    assert_eq!(value, Scalar::I32(-91));
    assert_eq!(handles(), 0);
}

#[test]
fn returned_primitive_box_is_rooted_before_validation_and_wrong_class_is_rejected() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: valid thread-local runtime.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[], (8, false));
    let wrong = object(9, 42);
    change_method(metadata, |m| m.result = wrong.as_ptr().addr());
    // SAFETY: fake returns a live object; intentional class mismatch tests fail-closed validation.
    let method = unsafe { context.bind(metadata) }.unwrap();
    assert!(matches!(method.invoke(None, &mut []), Err(InvokeError::ObjectTypeMismatch)));
    assert_eq!(handles(), 0, "rejected result root must be released");
    let correct = object(8, 42);
    change_method(metadata, |m| m.result = correct.as_ptr().addr());
    FAKE.with_borrow_mut(|f| f.fail_root = true);
    assert!(matches!(method.invoke(None, &mut []), Err(InvokeError::Runtime(RuntimeError::RootFailed))));
    assert_eq!(handles(), 0);
}

#[test]
fn reference_arguments_are_objects_or_null_and_arrays_use_array_class_not_element_class() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: valid thread-local runtime.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[(0x12, false)], (1, false));
    let child = object(DERIVED, 0);
    // SAFETY: registered live fake object.
    let child = unsafe { attached.root(child) }.unwrap();
    let child_address = child.target().unwrap().as_ptr().addr();
    change_method(metadata, |m| m.action = Action::ExpectObject(child_address));
    // SAFETY: body reads only the argument object address and is valid for the accepted reference shapes.
    let method = unsafe { context.bind(metadata) }.unwrap();
    assert!(matches!(
        method.invoke(None, &mut [Argument::Reference(Some(&child))]).unwrap(),
        InvocationOutcome::Returned(ReturnValue::Void)
    ));
    change_method(metadata, |m| m.action = Action::ExpectObject(0));
    assert!(method.invoke(None, &mut [Argument::Reference(None)]).is_ok());
    // SAFETY: registered live fake array/element objects.
    let array = unsafe { attached.root(object(ARRAY, 0)) }.unwrap();
    // SAFETY: registered live fake element object.
    let element = unsafe { attached.root(object(ELEMENT, 0)) }.unwrap();
    // SAFETY: no-op method accepts arrays, not individual elements.
    let array_method = unsafe { context.bind(self::method(&[(0x1d, false)], (1, false))) }.unwrap();
    assert!(array_method.invoke(None, &mut [Argument::Reference(Some(&array))]).is_ok());
    assert!(matches!(
        array_method.invoke(None, &mut [Argument::Reference(Some(&element))]),
        Err(InvokeError::ObjectTypeMismatch)
    ));
    assert_eq!(calls(), 3);
}

#[test]
fn instance_receivers_check_assignability_and_exact_value_box_before_unboxing() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: valid thread-local runtime.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[], (1, false));
    change_method(metadata, |m| m.instance = true);
    // SAFETY: no-op instance method, valid for all compatible receivers.
    let method = unsafe { context.bind(metadata) }.unwrap();
    assert!(matches!(method.invoke(None, &mut []), Err(InvokeError::ReceiverRequired)));
    // SAFETY: registered live fake derived object.
    let derived = unsafe { attached.root(object(DERIVED, 0)) }.unwrap();
    assert!(method.invoke(Some(&derived), &mut []).is_ok());
    // SAFETY: registered live fake unrelated object.
    let other = unsafe { attached.root(object(OTHER, 0)) }.unwrap();
    assert!(matches!(method.invoke(Some(&other), &mut []), Err(InvokeError::ObjectTypeMismatch)));
    let value_object = object(STRUCT, 7);
    // SAFETY: live fake value box; unbox returns its owned payload allocation.
    let payload = unsafe { unbox(value_object.as_ptr()) }.addr();
    // SAFETY: registered live fake value box.
    let value_root = unsafe { attached.root(value_object) }.unwrap();
    let metadata = self::method(&[], (1, false));
    change_method(metadata, |m| {
        m.class = STRUCT;
        m.instance = true;
        m.action = Action::ExpectValueReceiver(payload);
    });
    // SAFETY: fixture body only checks the authorized exact box payload address.
    let method = unsafe { context.bind(metadata) }.unwrap();
    assert!(method.invoke(Some(&value_root), &mut []).is_ok());
    assert!(matches!(method.invoke(Some(&derived), &mut []), Err(InvokeError::ObjectTypeMismatch)));
    assert_eq!(calls(), 2);
}

fn boxed_contract() -> ValueTypeContract {
    ValueTypeContract::BoxedReturn {
        class: NonNull::new(pointer(STRUCT)).unwrap(),
        size: 8,
        alignment: 8,
    }
}

fn enum_contract(class: usize) -> ValueTypeContract {
    ValueTypeContract::I32Enum {
        class: NonNull::new(pointer(class)).unwrap(),
    }
}

fn enum_method(parameters: &[usize], returns: Option<usize>) -> NonNull<raw::MethodInfo> {
    let metadata = method(&vec![(0x11, false); parameters.len()], if returns.is_some() { (0x11, false) } else { (1, false) });
    FAKE.with_borrow_mut(|f| {
        let signature = f.methods[&metadata.as_ptr().addr()].clone();
        for (parameter, class) in signature.parameters.iter().zip(parameters) {
            f.types.get_mut(parameter).unwrap().class = *class;
        }
        if let Some(class) = returns {
            f.types.get_mut(&signature.returns).unwrap().class = class;
        }
    });
    metadata
}

#[test]
fn opaque_boxed_return_is_rooted_without_copying_payload_then_usable_as_exact_receiver() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: valid fake runtime; opaque box payloads remain in fake managed storage.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let text = object(OWNER, 0);
    let boxed = object(STRUCT, text.as_ptr().addr() as u64);
    let payload = FAKE.with_borrow(|f| std::ptr::from_ref(f.objects[&boxed.as_ptr().addr()].payload.as_ref()).addr());
    let metadata = method(&[], (0x11, false));
    change_method(metadata, |method| method.result = boxed.as_ptr().addr());
    // SAFETY: fake body returns the exact authorized 8-byte struct box, never native struct storage.
    let getter = unsafe { context.bind_with_value_types(metadata, &[boxed_contract()]) }.unwrap();
    let outcome = getter.invoke(None, &mut []).unwrap();
    let InvocationOutcome::Returned(ReturnValue::Boxed(box_root)) = outcome else {
        panic!("expected opaque rooted value box")
    };
    assert_eq!(box_root.target(), Some(boxed));
    assert_eq!(handles(), 1);
    assert_eq!(FAKE.with_borrow(|f| f.unboxes), 0, "returning a struct must not expose or copy its payload");

    let code_metadata = method(&[], (0x0e, false));
    change_method(code_metadata, |method| {
        method.class = STRUCT;
        method.instance = true;
        method.action = Action::ExpectValueReceiver(payload);
        method.result = text.as_ptr().addr();
    });
    // SAFETY: this exact value-type receiver method only reads its own managed payload.
    let code_getter = unsafe { context.bind(code_metadata) }.unwrap();
    let outcome = code_getter.invoke(Some(&box_root), &mut []).unwrap();
    assert!(matches!(outcome, InvocationOutcome::Returned(ReturnValue::Reference(Some(_)))));
    assert_eq!(FAKE.with_borrow(|f| f.unboxes), 1);
    assert_eq!(handles(), 2);
    drop(outcome);
    drop(box_root);
    assert_eq!(handles(), 0);
}

#[test]
fn boxed_result_rejects_null_wrong_class_and_failed_root_without_unboxing() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: immutable fake metadata and registered result objects.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[], (0x11, false));
    // SAFETY: authored method/result contract; intentional malformed results exercise validation.
    let getter = unsafe { context.bind_with_value_types(metadata, &[boxed_contract()]) }.unwrap();
    assert!(matches!(getter.invoke(None, &mut []), Err(InvokeError::UnexpectedReturn)));
    let wrong = object(ENUM, 42);
    change_method(metadata, |method| method.result = wrong.as_ptr().addr());
    assert!(matches!(getter.invoke(None, &mut []), Err(InvokeError::ObjectTypeMismatch)));
    assert_eq!(handles(), 0);
    let correct = object(STRUCT, 42);
    change_method(metadata, |method| method.result = correct.as_ptr().addr());
    FAKE.with_borrow_mut(|f| f.fail_root = true);
    assert!(matches!(getter.invoke(None, &mut []), Err(InvokeError::Runtime(RuntimeError::RootFailed))));
    assert_eq!(FAKE.with_borrow(|f| f.unboxes), 0);
    assert_eq!(calls(), 3, "result errors follow executed bodies");
}

#[test]
fn explicit_box_contracts_reject_bad_size_alignment_class_and_duplicates_before_execution() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: fake immutable runtime, no malformed contract is invoked.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = method(&[], (0x11, false));
    for (size, alignment) in [(0, 8), (-1, 8), (7, 8), (9, 8), (8, 0), (8, 3), (8, 4), (8, 16)] {
        let contract = ValueTypeContract::BoxedReturn {
            class: NonNull::new(pointer(STRUCT)).unwrap(),
            size,
            alignment,
        };
        // SAFETY: only the declared expectations are wrong; all metadata pointers are valid.
        assert!(
            // SAFETY: only the declared expectations are wrong; all metadata pointers are valid.
            unsafe { context.bind_with_value_types(metadata, &[contract]) }.is_err(),
            "size {size}, alignment {alignment}"
        );
    }
    for (class, size, alignment) in [(OWNER, 8, 8), (ENUM, 4, 4)] {
        let contract = ValueTypeContract::BoxedReturn {
            class: NonNull::new(pointer(class)).unwrap(),
            size,
            alignment,
        };
        // SAFETY: reference and enum classes are valid metadata, deliberately unsupported for opaque boxes.
        assert!(unsafe { context.bind_with_value_types(metadata, &[contract]) }.is_err());
    }
    // SAFETY: duplicate declarations are rejected while binding, without executing the body.
    assert!(unsafe { context.bind_with_value_types(metadata, &[boxed_contract(), boxed_contract()]) }.is_err());
    for inflated in [false, true] {
        FAKE.with_borrow_mut(|fake| {
            let class = fake.classes.get_mut(&STRUCT).unwrap();
            class.generic = !inflated;
            class.inflated = inflated;
        });
        // SAFETY: generic/inflated value metadata is deliberately refused before execution.
        assert!(unsafe { context.bind_with_value_types(metadata, &[boxed_contract()]) }.is_err());
    }
    assert_eq!(calls(), 0);
}

#[test]
fn struct_arguments_and_byrefs_remain_unsupported_even_with_explicit_return_contracts() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: this fake runtime only observes rejected metadata in this test.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    for metadata in [method(&[(0x11, false)], (1, false)), method(&[(0x11, true)], (1, false)), method(&[], (0x11, true))] {
        // SAFETY: registered metadata, body is never invoked for unsupported arguments/byref returns.
        assert!(unsafe { context.bind_with_value_types(metadata, &[boxed_contract()]) }.is_err());
    }
    let enum_byref = enum_method(&[ENUM], None);
    FAKE.with_borrow_mut(|f| {
        let parameter = f.methods[&enum_byref.as_ptr().addr()].parameters[0];
        f.types.get_mut(&parameter).unwrap().by_ref = true;
    });
    // SAFETY: the enum class is valid, but ref/out enum native storage is intentionally unsupported.
    assert!(unsafe { context.bind_with_value_types(enum_byref, &[enum_contract(ENUM)]) }.is_err());
    assert_eq!(calls(), 0);
}

#[test]
fn enum_arguments_require_exact_class_tags_and_initialized_int32_storage() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: fake runtime exports and enum metadata satisfy the exact Int32 contract.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = enum_method(&[ENUM, OTHER_ENUM], Some(ENUM));
    let boxed = object(ENUM, u64::from((-12345_i32) as u32));
    change_method(metadata, |method| {
        method.result = boxed.as_ptr().addr();
        method.action = Action::ExpectEnumPair(i32::MIN, i32::MAX);
    });
    // SAFETY: body accepts every initialized int32 value in the exact two enum argument classes.
    let method = unsafe { context.bind_with_value_types(metadata, &[enum_contract(ENUM), enum_contract(OTHER_ENUM)]) }.unwrap();
    let class = NonNull::new(pointer(ENUM)).unwrap();
    let other = NonNull::new(pointer(OTHER_ENUM)).unwrap();
    for (first, second) in [(other, class), (NonNull::dangling(), other)] {
        assert!(matches!(
            method.invoke(None, &mut [Argument::EnumI32 { class: first, value: 1 }, Argument::EnumI32 { class: second, value: 2 }]),
            Err(InvokeError::ArgumentType { index: 0 })
        ));
    }
    assert!(matches!(
        method.invoke(None, &mut [Argument::Scalar(Scalar::I32(1)), Argument::EnumI32 { class: other, value: 2 }]),
        Err(InvokeError::ArgumentType { index: 0 })
    ));
    assert_eq!(calls(), 0);
    let result = method
        .invoke(
            None,
            &mut [Argument::EnumI32 { class, value: i32::MIN }, Argument::EnumI32 { class: other, value: i32::MAX }],
        )
        .unwrap();
    assert!(matches!(result, InvocationOutcome::Returned(ReturnValue::EnumI32 { value: -12345, .. })));
    assert_eq!(handles(), 1);
    assert_eq!(FAKE.with_borrow(|f| f.unboxes), 1);
    drop(result);
    assert_eq!(handles(), 0);
}

#[test]
fn enum_result_requires_its_exact_box_class_and_keeps_managed_exceptions_rooted() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: registered fake enum metadata and live rooted results.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    let metadata = enum_method(&[], Some(ENUM));
    let wrong = object(OTHER_ENUM, 42);
    change_method(metadata, |method| method.result = wrong.as_ptr().addr());
    // SAFETY: body may return an intentionally wrong live fake box; validation must reject it.
    let method = unsafe { context.bind_with_value_types(metadata, &[enum_contract(ENUM)]) }.unwrap();
    assert!(matches!(method.invoke(None, &mut []), Err(InvokeError::ObjectTypeMismatch)));
    assert_eq!(FAKE.with_borrow(|f| f.unboxes), 0);
    assert_eq!(handles(), 0);
    let exception = object(OWNER, 0);
    change_method(metadata, |method| {
        method.result = 0;
        method.exception = exception.as_ptr().addr();
    });
    let outcome = method.invoke(None, &mut []).unwrap();
    assert!(matches!(outcome, InvocationOutcome::Exception(_)));
    assert_eq!(handles(), 1);
    drop(outcome);
    assert_eq!(handles(), 0);
}

#[test]
fn enum_contract_checks_base_identity_flags_width_and_alignment_before_invocation() {
    for invalid in 0..11 {
        let runtime = context();
        let attached = runtime.attach().unwrap();
        // SAFETY: all fake pointers remain valid; individual invalid metadata is rejected below.
        let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
        let metadata = enum_method(&[], Some(ENUM));
        FAKE.with_borrow_mut(|f| match invalid {
            0 => f.classes.get_mut(&ENUM).unwrap().enumeration = false,
            1 => f.classes.get_mut(&ENUM).unwrap().value = false,
            2 => f.classes.get_mut(&ENUM).unwrap().size = 8,
            3 => f.classes.get_mut(&ENUM).unwrap().alignment = 8,
            4 => f.types.get_mut(&ENUM_BASE).unwrap().code = 9,
            5 => f.types.get_mut(&ENUM_BASE).unwrap().class = 9,
            6 => f.types.get_mut(&ENUM_BASE).unwrap().by_ref = true,
            7 => f.corlib = 0,
            8 => f.int32_class = 0,
            9 => f.classes.get_mut(&ENUM).unwrap().enum_base = 0,
            _ => f.classes.get_mut(&ENUM).unwrap().generic = true,
        });
        // SAFETY: the fake runtime contract is intentionally invalid only for binding checks.
        assert!(
            // SAFETY: the fake runtime contract is intentionally invalid only for binding checks.
            unsafe { context.bind_with_value_types(metadata, &[enum_contract(ENUM)]) }.is_err(),
            "invalid enum contract {invalid}"
        );
        assert_eq!(calls(), 0);
    }
}

#[test]
fn default_bind_still_rejects_enums_and_explicit_contracts_do_not_enable_generic_methods() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: live authored fake metadata, no rejected method body is invoked.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    for metadata in [enum_method(&[ENUM], None), enum_method(&[], Some(ENUM))] {
        // SAFETY: checking metadata only; normal binding must retain its old unsupported boundary.
        assert!(matches!(unsafe { context.bind(metadata) }, Err(InvokeError::UnsupportedType { .. })));
    }
    let generic = enum_method(&[], Some(ENUM));
    change_method(generic, |method| method.generic = true);
    // SAFETY: generic method metadata is valid but deliberately not supported by this adapter.
    assert!(matches!(
        // SAFETY: generic method metadata is valid but deliberately not supported by this adapter.
        unsafe { context.bind_with_value_types(generic, &[enum_contract(ENUM)]) },
        Err(InvokeError::GenericMethod)
    ));
    assert_eq!(calls(), 0);
}

#[test]
fn nongeneric_object_return_keeps_a_closed_generic_value_box_rooted_without_payload_exposure() {
    let runtime = context();
    let attached = runtime.attach().unwrap();
    // SAFETY: authored nongeneric Array-like method returns Object; actual result is a generic box.
    let context = unsafe { InvocationContext::new(&attached, invocation_api()) };
    FAKE.with_borrow_mut(|f| {
        f.classes.get_mut(&ARRAY).unwrap().parent = OWNER;
        let entry = f.classes.get_mut(&STRUCT).unwrap();
        entry.parent = OWNER;
        entry.inflated = true;
    });
    let array = object(ARRAY, 0);
    let entry = object(STRUCT, 0x1234);
    let metadata = method(&[(8, false)], (0x1c, false));
    change_method(metadata, |method| {
        method.instance = true;
        method.action = Action::ExpectValueReceiver(array.as_ptr().addr());
        method.result = entry.as_ptr().addr();
    });
    // SAFETY: the receiver and method are live objects/metadata from this fake engine.
    let array_root = unsafe { attached.root(array) }.unwrap();
    // SAFETY: live MethodInfo and the explicitly authorized body belong to this owned fake runtime.
    let method = unsafe { context.bind(metadata) }.unwrap();
    let result = method.invoke(Some(&array_root), &mut [Argument::Scalar(Scalar::I32(0))]).unwrap();
    let InvocationOutcome::Returned(ReturnValue::Reference(Some(entry_root))) = result else {
        panic!("expected rooted Object box");
    };
    assert_eq!(entry_root.target(), Some(entry));
    assert_eq!(FAKE.with_borrow(|f| f.unboxes), 0);
    drop(array_root);
    assert_eq!(handles(), 1, "the generic value box is independently rooted");
    drop(entry_root);
    assert_eq!(handles(), 0);
}
