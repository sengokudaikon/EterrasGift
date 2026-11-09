use super::*;
use crate::metadata::Ptr;
use std::ffi::{CString, c_char};

// Authored metadata: a class is a `FakeClass`, a method a `FakeMethod` and a type a `FakeType`, each behind the pointer
// the runtime would hand out.
struct FakeType {
    name: &'static str,
    byref: bool,
    code: i32,
    class: usize,
}
struct FakeMethod {
    name: Option<&'static CStr>,
    params: &'static [FakeType],
    returns: FakeType,
    instance: bool,
    generic: bool,
    flags: u32,
    /// The class that declares it, when not the one listing it.
    declared_by: Option<usize>,
}
struct FakeClass {
    methods: Vec<FakeMethod>,
    parent: Option<Box<FakeClass>>,
}

const INT: FakeType = FakeType {
    name: "System.Int32",
    byref: false,
    code: raw::type_code::I4,
    class: 0x40,
};
const INT_REF: FakeType = FakeType {
    name: "System.Int32&",
    byref: true,
    code: raw::type_code::I4,
    class: 0x40,
};
const VOID: FakeType = FakeType {
    name: "System.Void",
    byref: false,
    code: raw::type_code::VOID,
    class: 0x50,
};
const TEXT: FakeType = FakeType {
    name: "System.String",
    byref: false,
    code: raw::type_code::STRING,
    class: 0x60,
};

fn method(name: &'static CStr, params: &'static [FakeType]) -> FakeMethod {
    FakeMethod {
        name: Some(name),
        params,
        returns: VOID,
        instance: true,
        generic: false,
        flags: 0,
        declared_by: None,
    }
}
fn class(methods: Vec<FakeMethod>) -> FakeClass {
    FakeClass { methods, parent: None }
}
fn class_ptr(class: &FakeClass) -> Class {
    (class as *const FakeClass).cast_mut().cast()
}

unsafe extern "C-unwind" fn methods(owner: Class, iterator: *mut Ptr) -> Method {
    // SAFETY: the owner is a FakeClass and the iterator slot holds the next index.
    unsafe {
        let class = &*owner.cast::<FakeClass>();
        let index = *iterator as usize;
        *iterator = (index + 1) as Ptr;
        class.methods.get(index).map_or(std::ptr::null(), |method| (method as *const FakeMethod).cast())
    }
}
fn fake<'a>(method: Method) -> &'a FakeMethod {
    // SAFETY: every method handed out is a FakeMethod that outlives the lookup.
    unsafe { &*method.cast::<FakeMethod>() }
}
fn fake_type<'a>(ty: Type) -> &'a FakeType {
    // SAFETY: every type handed out is a FakeType that outlives the lookup.
    unsafe { &*ty.cast::<FakeType>() }
}
unsafe extern "C-unwind" fn name(method: Method) -> *const c_char {
    fake(method).name.map_or(std::ptr::null(), CStr::as_ptr)
}
unsafe extern "C-unwind" fn count(method: Method) -> u32 {
    fake(method).params.len() as u32
}
unsafe extern "C-unwind" fn param(method: Method, index: u32) -> Type {
    (&fake(method).params[index as usize] as *const FakeType).cast()
}
unsafe extern "C-unwind" fn returns(method: Method) -> Type {
    (&fake(method).returns as *const FakeType).cast()
}
unsafe extern "C-unwind" fn instance(method: Method) -> bool {
    fake(method).instance
}
unsafe extern "C-unwind" fn generic(method: Method) -> bool {
    fake(method).generic
}
unsafe extern "C-unwind" fn flags(method: Method, _: *mut u32) -> u32 {
    fake(method).flags
}
unsafe extern "C-unwind" fn byref(ty: Type) -> bool {
    fake_type(ty).byref
}
unsafe extern "C-unwind" fn type_name(ty: Type) -> *mut c_char {
    CString::new(fake_type(ty).name).unwrap().into_raw()
}
unsafe extern "C-unwind" fn free(text: Ptr) {
    // SAFETY: every text freed came from type_name.
    unsafe { drop(CString::from_raw(text.cast())) }
}
unsafe extern "C-unwind" fn type_class(ty: Type) -> Class {
    fake_type(ty).class as Class
}
unsafe extern "C-unwind" fn type_code(ty: Type) -> i32 {
    fake_type(ty).code
}
unsafe extern "C-unwind" fn parent(class: Class) -> Class {
    // SAFETY: the class is a FakeClass.
    unsafe { (*class.cast::<FakeClass>()).parent.as_deref().map_or(std::ptr::null_mut(), class_ptr) }
}

thread_local! {
    /// The class `method_class` says declares a method listed without `declared_by`.
    static LISTING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe extern "C-unwind" fn declaring(method: Method) -> Class {
    fake(method).declared_by.unwrap_or_else(|| LISTING.get()) as Class
}

fn api() -> Exports {
    let mut api = Exports::test_stubs();
    api.class_methods = methods;
    api.method_name = name;
    api.method_params = count;
    api.method_param = param;
    api.method_return = returns;
    api.method_instance = instance;
    api.method_generic = generic;
    api.method_inflated = generic;
    api.method_flags = flags;
    api.method_class = declaring;
    api.type_byref = byref;
    api.type_name = type_name;
    api.runtime_free = free;
    api.type_class = type_class;
    api.type_code = type_code;
    api.class_parent = parent;
    api
}

/// The lookup's outcome: the index of the method found on `class`, or why none was.
fn find(class: &FakeClass, query: &MethodQuery<'_>) -> Result<usize, MethodError> {
    let owner = class_ptr(class);
    LISTING.set(owner as usize);
    // SAFETY: the fake class and its methods live for the call.
    let found = unsafe { api().find_method(owner, query) }?;
    Ok(class.methods.iter().position(|method| std::ptr::eq(method, fake(found))).unwrap())
}
fn kind(result: Result<usize, MethodError>) -> &'static str {
    match result {
        Ok(_) => "found",
        Err(MethodError::Metadata(_)) => "metadata",
        Err(MethodError::Unnamed) => "unnamed",
        Err(MethodError::Untyped) => "untyped",
        Err(MethodError::Missing) => "missing",
        Err(MethodError::Twice) => "twice",
        Err(MethodError::Signature) => "signature",
        Err(MethodError::Parameters) => "parameters",
    }
}

#[test]
fn what_another_method_of_the_name_does_is_the_querys() {
    let overloaded = class(vec![method(c"Set", &[]), method(c"Set", &[TEXT]), method(c"Set", &[INT])]);
    let named = |others| MethodQuery {
        params: Params::Named(&["System.Int32"]),
        others,
        ..MethodQuery::named(c"Set", 1)
    };
    assert_eq!(kind(find(&overloaded, &named(Others::Overloads))), "found");
    assert_eq!(find(&overloaded, &named(Others::Overloads)).unwrap(), 2);
    // Of the same count, the first one with other types fails it; another count fails only `Fail`.
    assert_eq!(kind(find(&overloaded, &named(Others::OtherCounts))), "parameters");
    assert_eq!(kind(find(&overloaded, &named(Others::Fail))), "signature");
    let counted = |others| MethodQuery {
        others,
        ..MethodQuery::named(c"Set", 0)
    };
    assert_eq!(find(&overloaded, &counted(Others::OtherCounts)).unwrap(), 0);
    assert_eq!(kind(find(&overloaded, &counted(Others::Fail))), "signature");
    // Two of the name, count and types: the lookup cannot choose.
    let twice = class(vec![method(c"Set", &[INT]), method(c"Set", &[INT])]);
    assert_eq!(kind(find(&twice, &named(Others::Overloads))), "twice");
    assert_eq!(kind(find(&twice, &MethodQuery::named(c"Set", 1))), "twice");
    assert_eq!(kind(find(&overloaded, &MethodQuery::named(c"Get", 0))), "missing");
}

#[test]
fn parameters_match_by_name_with_their_reference_or_by_class_by_value() {
    let by_ref = class(vec![method(c"Read", &[INT_REF])]);
    let named = |names: &'static [&'static str]| MethodQuery {
        params: Params::Named(names),
        others: Others::Overloads,
        ..MethodQuery::named(c"Read", 1)
    };
    assert_eq!(kind(find(&by_ref, &named(&["System.Int32&"]))), "found");
    assert_eq!(kind(find(&by_ref, &named(&["System.Int32"]))), "missing");
    let classes = |others| MethodQuery {
        params: Params::Classes(&[0x40 as Class]),
        others,
        ..MethodQuery::named(c"Read", 1)
    };
    // A by-reference parameter is not its class by value.
    assert_eq!(kind(find(&by_ref, &classes(Others::Overloads))), "missing");
    assert_eq!(kind(find(&by_ref, &classes(Others::OtherCounts))), "parameters");
    assert_eq!(kind(find(&class(vec![method(c"Read", &[INT])]), &classes(Others::OtherCounts))), "found");
}

#[test]
fn the_one_chosen_meets_every_part_of_the_contract() {
    let strict = MethodQuery {
        instance: Some(true),
        returns: Returns::Code(raw::type_code::VOID),
        refused: raw::METHOD_ATTRIBUTE_VIRTUAL | raw::METHOD_ATTRIBUTE_ABSTRACT,
        refuse_generic: true,
        declared: true,
        ..MethodQuery::named(c"Run", 0)
    };
    let changed = |change: fn(&mut FakeMethod)| {
        let mut run = method(c"Run", &[]);
        change(&mut run);
        kind(find(&class(vec![run]), &strict))
    };
    assert_eq!(changed(|_| {}), "found");
    assert_eq!(changed(|m| m.instance = false), "signature");
    assert_eq!(changed(|m| m.generic = true), "signature");
    assert_eq!(changed(|m| m.flags = raw::METHOD_ATTRIBUTE_VIRTUAL), "signature");
    assert_eq!(changed(|m| m.flags = raw::METHOD_ATTRIBUTE_PINVOKE_IMPL), "found", "only the refused attributes count");
    assert_eq!(changed(|m| m.declared_by = Some(0x99)), "signature");
    assert_eq!(changed(|m| m.returns = TEXT), "signature");
    assert_eq!(changed(|m| m.returns = INT_REF), "signature", "a return by reference");
    let returning = |returns| {
        let mut get = method(c"Get", &[]);
        get.returns = TEXT;
        kind(find(
            &class(vec![get]),
            &MethodQuery {
                returns,
                ..MethodQuery::named(c"Get", 0)
            },
        ))
    };
    assert_eq!(returning(Returns::Reference), "found");
    assert_eq!(returning(Returns::Named("System.String")), "found");
    assert_eq!(returning(Returns::Named("System.Object")), "signature");
    assert_eq!(returning(Returns::Class(0x60 as Class)), "found");
    assert_eq!(returning(Returns::Class(0x40 as Class)), "signature");
    assert_eq!(returning(Returns::Any), "found");
}

#[test]
fn a_method_without_a_name_fails_the_lookup() {
    let mut unnamed = method(c"Run", &[]);
    unnamed.name = None;
    assert_eq!(kind(find(&class(vec![method(c"Run", &[]), unnamed]), &MethodQuery::named(c"Run", 0))), "unnamed");
}

#[test]
fn the_nearest_class_that_declares_the_method_is_found_up_its_parents() {
    let base = class(vec![method(c"Refresh", &[]), method(c"Format", &[INT])]);
    let middle = FakeClass {
        methods: vec![method(c"Refresh", &[INT])],
        parent: Some(Box::new(base)),
    };
    let leaf = FakeClass {
        methods: vec![method(c"Other", &[])],
        parent: Some(Box::new(middle)),
    };
    let nearest = |name, count| {
        // SAFETY: the fake classes live for the call.
        unsafe { api().nearest_declaring(class_ptr(&leaf), name, count) }
    };
    let middle = leaf.parent.as_deref().unwrap();
    let base = middle.parent.as_deref().unwrap();
    assert_eq!(nearest(c"Refresh", 1).unwrap(), class_ptr(middle));
    assert_eq!(nearest(c"Refresh", 0).unwrap(), class_ptr(base));
    assert_eq!(nearest(c"Format", 1).unwrap(), class_ptr(base));
    assert!(matches!(nearest(c"Format", 2), Err(MethodError::Missing)));
}
