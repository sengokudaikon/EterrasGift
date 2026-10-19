use super::*;
use crate::il2cpp::{ThreadContext, discovery::TestExportStub};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::{c_char, c_void},
};

type Ptr = *mut c_void;
type Class = *mut raw::Class;
const GAME: usize = 0x100;
const CORLIB: usize = 0x200;
const LEAF: usize = 1;
const BASE: usize = 2;
const PROPERTY: usize = 3;
const STAT: usize = 4;
const LOCALIZED: usize = 5;
const STATS: usize = 6;
const MOD: usize = 7;
const ROUNDING: usize = 8;
const ENTRY: usize = 9;
const FORMATTER: usize = 10;
const LIST: usize = 11;
const ARRAY: usize = 12;
const OBJECT: usize = 13;
const SINGLE: usize = 14;
const BYTE: usize = 15;
const I32: usize = 16;
const BOOLEAN: usize = 17;
const STRING: usize = 18;
const PLAYER: usize = 19;
const DERIVED: usize = 20;
const PREFORMATTED: usize = 21;
const I64: usize = 22;
const OTHER_SINGLE: usize = 23;
const INPUT: usize = 0x10000;
const PROPERTY_OBJECT: usize = 0x11000;
const LIST_OBJECT: usize = 0x12000;
const ENTRY1: usize = 0x13000;
const ENTRY2: usize = 0x14000;
const OUTPUT: usize = 0x15000;
const EXCEPTION: usize = 0x16000;
const METHOD: usize = 0x17000;
const VALUE: usize = 100;
const MINIMUM: usize = 101;
const MAXIMUM: usize = 102;
const PROPERTY_FIELD: usize = 103;
const MOD_FIELD: usize = 104;
const TAG: usize = 105;
const ADDED: usize = 106;
const MORE: usize = 107;
const PERCENT: usize = 108;
const TENTH: usize = 109;
const OVERRIDES: usize = 110;
const SIZE: usize = 111;
const ITEMS: usize = 112;
const ENTRY_TAG: usize = 113;
const ENTRY_ROUND: usize = 114;
const FIELD_BIAS: usize = 0x200000;
const TYPE_BIAS: usize = 0x300000;
const REF_TYPE_BIAS: usize = 0x400000;
fn ptr<T>(value: usize) -> *mut T {
    value as *mut T
}
fn class(value: usize) -> Class {
    ptr(value * 256)
}
fn ci(value: *const raw::Class) -> usize {
    value as usize / 256
}
fn ty(value: usize) -> *const raw::Type {
    ptr(TYPE_BIAS + value)
}
fn field(value: usize) -> Ptr {
    ptr(FIELD_BIAS + value)
}
fn fi(value: Ptr) -> usize {
    value as usize - FIELD_BIAS
}
fn nn<T>(value: usize) -> NonNull<T> {
    NonNull::new(ptr(value)).unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    Parent,
    ObjectBase,
    Derived,
    Preformatted,
    ValueClass,
    Generic,
    Inflated,
    WrongImage,
    Owner,
    Static,
    Literal,
    Readonly,
    FieldType,
    FieldIdentity,
    Byref,
    Missing,
    NonEnum,
    EnumBase,
    NullProperty,
    DerivedProperty,
    ChildRoot,
    BadList,
    BadOverride,
    Signature,
    NullReturn,
    Exception,
    ReturnRoot,
    Utf16,
    Nul,
    Oversized,
}
struct State {
    fault: Fault,
    field: usize,
    roots: BTreeMap<usize, usize>,
    metadata: usize,
    reads: Vec<usize>,
    pending_child: bool,
    enum_bases: usize,
    invokes: usize,
    values: [f32; 3],
    modifier: i32,
    tag: u8,
    text: Vec<u16>,
    array: Box<[usize; 6]>,
}
impl State {
    fn new() -> Self {
        Self {
            fault: Fault::None,
            field: VALUE,
            roots: BTreeMap::new(),
            metadata: 0,
            reads: vec![],
            pending_child: false,
            enum_bases: 0,
            invokes: 0,
            values: [1.25, 0.0, 2.0],
            modifier: 0,
            tag: 2,
            text: "1.25".encode_utf16().collect(),
            array: Box::new([0, 0, 0, 0, ENTRY1, ENTRY2]),
        }
    }
}
thread_local! { static S:RefCell<State>=RefCell::new(State::new()); }
fn fault() -> Fault {
    S.with_borrow(|s| s.fault)
}
fn metadata() {
    S.with_borrow_mut(|s| {
        assert!(!s.pending_child, "reference must root before any new metadata export");
        s.metadata += 1;
    });
}
fn rooted(object: usize) {
    S.with_borrow(|s| assert!(s.roots.get(&object).copied().unwrap_or(0) > 0, "object must have its own strong root before inspection"));
}
fn owner(f: usize) -> usize {
    match f {
        VALUE => BASE,
        MINIMUM | MAXIMUM | PROPERTY_FIELD | MOD_FIELD | TAG => LEAF,
        ADDED | MORE | PERCENT | TENTH => PROPERTY,
        OVERRIDES => STAT,
        SIZE | ITEMS => LIST,
        ENTRY_TAG | ENTRY_ROUND => ENTRY,
        _ => panic!("unknown field"),
    }
}
fn field_class(f: usize) -> usize {
    match f {
        VALUE | MINIMUM | MAXIMUM => SINGLE,
        PROPERTY_FIELD => PROPERTY,
        MOD_FIELD => MOD,
        TAG | ENTRY_TAG => BYTE,
        ADDED | MORE | ENTRY_ROUND => ROUNDING,
        PERCENT | TENTH => BOOLEAN,
        OVERRIDES => LIST,
        SIZE => I32,
        ITEMS => ARRAY,
        _ => panic!("unknown field"),
    }
}
unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
    ptr(1)
}
unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
    ptr(2)
}
unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
    panic!("already attached")
}
unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
    panic!("borrowed attachment")
}
unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> Ptr {
    assert!(!pinned);
    let object = object as usize;
    S.with_borrow_mut(|s| {
        if s.pending_child {
            assert_eq!(object, PROPERTY_OBJECT);
            s.pending_child = false;
        }
        if (s.fault == Fault::ChildRoot && object == PROPERTY_OBJECT) || (s.fault == Fault::ReturnRoot && object == OUTPUT) {
            return std::ptr::null_mut();
        }
        *s.roots.entry(object).or_default() += 1;
        ptr(object)
    })
}
unsafe extern "C-unwind" fn target(handle: Ptr) -> *mut raw::Object {
    rooted(handle as usize);
    handle.cast()
}
unsafe extern "C-unwind" fn free(handle: Ptr) {
    S.with_borrow_mut(|s| {
        let count = s.roots.get_mut(&(handle as usize)).unwrap();
        *count -= 1;
        if *count == 0 {
            s.roots.remove(&(handle as usize));
        }
    });
}
fn context() -> ThreadContext {
    unsafe {
        ThreadContext::new(raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: attach,
            thread_detach: detach,
            gchandle_new: root,
            gchandle_free: free,
            gchandle_get_target: target,
        })
    }
}
unsafe extern "C-unwind" fn assemblies(_: *const raw::Domain, count: *mut usize) -> *const Ptr {
    metadata();
    unsafe { *count = 1 };
    static VALUES: [usize; 1] = [GAME];
    VALUES.as_ptr().cast()
}
unsafe extern "C-unwind" fn image(value: Ptr) -> Ptr {
    metadata();
    value
}
unsafe extern "C-unwind" fn corlib() -> Ptr {
    metadata();
    ptr(CORLIB)
}
unsafe extern "C-unwind" fn api_corlib() -> *const c_void {
    unsafe { corlib().cast() }
}
unsafe extern "C-unwind" fn from_name(image: Ptr, ns: *const c_char, name: *const c_char) -> Class {
    metadata();
    let (ns, name) = unsafe { (CStr::from_ptr(ns).to_bytes(), CStr::from_ptr(name).to_bytes()) };
    let id = if image as usize == CORLIB {
        assert_eq!(ns, b"System");
        match name {
            b"Object" => OBJECT,
            b"Single" => SINGLE,
            b"Byte" => BYTE,
            b"Int32" => I32,
            b"Boolean" => BOOLEAN,
            b"String" => STRING,
            _ => 0,
        }
    } else {
        match (ns, name) {
            (b"", b"PlayerFinder") => PLAYER,
            (b"LE.UI.Tooltips", b"ItemTooltipModifierInfo") => BASE,
            (b"LE.UI.Tooltips", b"ItemTooltipPropertyModifierInfo") => LEAF,
            (b"", b"BasePropertyInfo") => PROPERTY,
            (b"", b"StatPropertyInfo") => STAT,
            (b"", b"PropertyInfoWithLocKey") => LOCALIZED,
            (b"", b"BaseStats") => STATS,
            (b"", b"PropertyRounding") => ROUNDING,
            (b"", b"TooltipItemManager") => FORMATTER,
            _ => 0,
        }
    };
    class(id)
}
unsafe extern "C-unwind" fn api_from_name(image: *const c_void, ns: *const c_char, name: *const c_char) -> Class {
    unsafe { from_name(image.cast_mut().cast(), ns, name) }
}
unsafe extern "C-unwind" fn parent(value: Class) -> Class {
    metadata();
    class(match ci(value) {
        LEAF => {
            if fault() == Fault::Parent {
                OBJECT
            } else {
                BASE
            }
        }
        BASE => {
            if fault() == Fault::ObjectBase {
                PROPERTY
            } else {
                OBJECT
            }
        }
        STAT => LOCALIZED,
        LOCALIZED => PROPERTY,
        OBJECT => 0,
        _ => OBJECT,
    })
}
unsafe extern "C-unwind" fn nested(value: Class, iter: *mut Ptr) -> Class {
    metadata();
    let index = unsafe { *iter } as usize;
    unsafe { *iter = ptr(index + 1) };
    if index > 0 {
        return std::ptr::null_mut();
    }
    class(match ci(value) {
        STATS => MOD,
        STAT => ENTRY,
        _ => panic!("unexpected nested owner"),
    })
}
unsafe extern "C-unwind" fn name(value: Class) -> *const c_char {
    metadata();
    match ci(value) {
        MOD => c"ModType",
        ENTRY => c"MoreRoundingOverride",
        LIST => c"List`1",
        _ => panic!("unexpected class name"),
    }
    .as_ptr()
}
unsafe extern "C-unwind" fn namespace(value: Class) -> *const c_char {
    metadata();
    assert_eq!(ci(value), LIST);
    c"System.Collections.Generic".as_ptr()
}
unsafe extern "C-unwind" fn class_image(value: Class) -> Ptr {
    metadata();
    ptr(
        if (fault() == Fault::WrongImage && ci(value) == LEAF) || matches!(ci(value), OBJECT | SINGLE | BYTE | I32 | BOOLEAN | STRING | LIST | ARRAY | I64 | OTHER_SINGLE) {
            CORLIB
        } else {
            GAME
        },
    )
}
unsafe extern "C-unwind" fn get_field(value: Class, name: *const c_char) -> Ptr {
    metadata();
    let name = unsafe { CStr::from_ptr(name).to_bytes() };
    let f = match (ci(value), name) {
        (BASE, b"<Value>k__BackingField") => VALUE,
        (LEAF, b"<MinValue>k__BackingField") => MINIMUM,
        (LEAF, b"<MaxValue>k__BackingField") => MAXIMUM,
        (LEAF, b"_propertyInfo") => PROPERTY_FIELD,
        (LEAF, b"_modType") => MOD_FIELD,
        (LEAF, b"_specialTag") => TAG,
        (PROPERTY, b"roundingForAdded") => ADDED,
        (PROPERTY, b"roundingForMore") => MORE,
        (PROPERTY, b"displayAddedAsPercentage") => PERCENT,
        (PROPERTY, b"displayAddedAsTenthOfValue") => TENTH,
        (STAT, b"moreRoundingOverrides") => OVERRIDES,
        (LIST, b"_size") => SIZE,
        (LIST, b"_items") => ITEMS,
        (ENTRY, b"specialTag") => ENTRY_TAG,
        (ENTRY, b"roundingForMore") => ENTRY_ROUND,
        _ => return std::ptr::null_mut(),
    };
    if fault() == Fault::Missing && S.with_borrow(|s| s.field == f) {
        std::ptr::null_mut()
    } else {
        field(f)
    }
}
unsafe extern "C-unwind" fn field_parent(value: Ptr) -> Class {
    metadata();
    class(if fault() == Fault::Owner && S.with_borrow(|s| s.field == fi(value)) {
        OBJECT
    } else {
        owner(fi(value))
    })
}
unsafe extern "C-unwind" fn field_flags(value: Ptr) -> i32 {
    metadata();
    let f = fi(value);
    let mut result = if matches!(f, MINIMUM | MAXIMUM | PROPERTY_FIELD | MOD_FIELD | TAG) { 0x20 } else { 0 };
    if S.with_borrow(|s| s.field == f) {
        match fault() {
            Fault::Static => result |= 0x10,
            Fault::Literal => result |= 0x40,
            Fault::Readonly => result ^= 0x20,
            _ => (),
        }
    }
    result
}
unsafe extern "C-unwind" fn field_type(value: Ptr) -> *const raw::Type {
    metadata();
    let f = fi(value);
    if S.with_borrow(|s| s.field == f) {
        match fault() {
            Fault::FieldType => return ty(I64),
            Fault::FieldIdentity => return ty(OTHER_SINGLE),
            Fault::Byref => return ptr(REF_TYPE_BIAS + f),
            _ => (),
        }
    }
    ty(field_class(f))
}
fn type_id(value: *const raw::Type) -> usize {
    let value = value as usize;
    if value >= REF_TYPE_BIAS {
        field_class(value - REF_TYPE_BIAS)
    } else {
        value - TYPE_BIAS
    }
}
unsafe extern "C-unwind" fn type_class(value: *const raw::Type) -> Class {
    metadata();
    class(type_id(value))
}
unsafe extern "C-unwind" fn class_type(value: Class) -> *const raw::Type {
    metadata();
    ty(ci(value))
}
unsafe extern "C-unwind" fn type_code(value: *const raw::Type) -> i32 {
    metadata();
    match type_id(value) {
        SINGLE | OTHER_SINGLE => 0x0c,
        BYTE => 5,
        I32 => 8,
        BOOLEAN => 2,
        STRING => 0x0e,
        OBJECT => 0x1c,
        MOD | ROUNDING => 0x11,
        LIST => 0x15,
        ARRAY => 0x1d,
        I64 => 0x0a,
        _ => 0x12,
    }
}
unsafe extern "C-unwind" fn byref(value: *const raw::Type) -> bool {
    metadata();
    value as usize >= REF_TYPE_BIAS
}
unsafe extern "C-unwind" fn value_type(value: *const raw::Class) -> bool {
    metadata();
    matches!(ci(value), SINGLE | BYTE | I32 | BOOLEAN | MOD | ROUNDING | I64 | OTHER_SINGLE) || (ci(value) == LEAF && fault() == Fault::ValueClass)
}
unsafe extern "C-unwind" fn generic(value: *const raw::Class) -> bool {
    metadata();
    ci(value) == LEAF && fault() == Fault::Generic
}
unsafe extern "C-unwind" fn inflated(value: *const raw::Class) -> bool {
    metadata();
    ci(value) == LEAF && fault() == Fault::Inflated
}
unsafe extern "C-unwind" fn is_enum(value: *const raw::Class) -> bool {
    metadata();
    matches!(ci(value), MOD | ROUNDING) && !(ci(value) == MOD && fault() == Fault::NonEnum)
}
unsafe extern "C-unwind" fn enum_base(value: Class) -> *const raw::Type {
    metadata();
    assert!(unsafe { is_enum(value) });
    S.with_borrow_mut(|s| s.enum_bases += 1);
    ty(if fault() == Fault::EnumBase && ci(value) == MOD { BYTE } else { I32 })
}
unsafe extern "C-unwind" fn value_size(_: Class, alignment: *mut u32) -> i32 {
    metadata();
    unsafe { *alignment = 4 };
    4
}
unsafe extern "C-unwind" fn object_class(value: *mut raw::Object) -> Class {
    metadata();
    let value = value as usize;
    rooted(value);
    class(match value {
        INPUT => match fault() {
            Fault::Derived => DERIVED,
            Fault::Preformatted => PREFORMATTED,
            _ => LEAF,
        },
        PROPERTY_OBJECT => {
            if fault() == Fault::DerivedProperty {
                DERIVED
            } else {
                STAT
            }
        }
        LIST_OBJECT => LIST,
        ENTRY1 | ENTRY2 => ENTRY,
        OUTPUT => STRING,
        EXCEPTION => OBJECT,
        _ => {
            assert_eq!(value, S.with_borrow(|s| s.array.as_ptr() as usize));
            ARRAY
        }
    })
}
unsafe extern "C-unwind" fn assignable(expected: *mut raw::Class, actual: *mut raw::Class) -> bool {
    metadata();
    expected == actual || (ci(expected) == PROPERTY && ci(actual) == STAT)
}
unsafe extern "C-unwind" fn element(value: Class) -> Class {
    metadata();
    assert_eq!(ci(value), ARRAY);
    class(ENTRY)
}
unsafe extern "C-unwind" fn rank(value: *const raw::Class) -> i32 {
    metadata();
    assert_eq!(ci(value), ARRAY);
    1
}
unsafe extern "C-unwind" fn header() -> u32 {
    metadata();
    32
}
unsafe extern "C-unwind" fn length_offset() -> u32 {
    metadata();
    24
}
unsafe extern "C-unwind" fn array_length(value: *mut raw::Object) -> u32 {
    metadata();
    rooted(value as usize);
    2
}
unsafe extern "C-unwind" fn get(value: *mut raw::Object, field: Ptr, out: Ptr) {
    metadata();
    rooted(value as usize);
    let f = fi(field);
    S.with_borrow_mut(|s| s.reads.push(f));
    unsafe {
        match f {
            PROPERTY_FIELD => {
                out.cast::<usize>().write(if fault() == Fault::NullProperty { 0 } else { PROPERTY_OBJECT });
                if fault() != Fault::NullProperty {
                    S.with_borrow_mut(|s| s.pending_child = true)
                }
            }
            VALUE | MINIMUM | MAXIMUM => out.cast::<f32>().write(S.with_borrow(|s| s.values[f - VALUE])),
            MOD_FIELD => out.cast::<i32>().write(S.with_borrow(|s| s.modifier)),
            TAG => out.cast::<u8>().write(S.with_borrow(|s| s.tag)),
            ADDED => out.cast::<i32>().write(1),
            MORE => out.cast::<i32>().write(0),
            PERCENT | TENTH => out.cast::<u8>().write(0),
            OVERRIDES => out.cast::<usize>().write(if fault() == Fault::BadList { 0 } else { LIST_OBJECT }),
            SIZE => out.cast::<i32>().write(2),
            ITEMS => out.cast::<usize>().write(S.with_borrow(|s| s.array.as_ptr() as usize)),
            ENTRY_TAG => out.cast::<u8>().write(2),
            ENTRY_ROUND => out.cast::<i32>().write(if fault() == Fault::BadOverride && value as usize == ENTRY2 {
                4
            } else if value as usize == ENTRY1 {
                3
            } else {
                1
            }),
            _ => panic!("unexpected field copy"),
        }
    }
}
unsafe extern "C-unwind" fn methods(value: Class, iter: *mut Ptr) -> *const raw::MethodInfo {
    metadata();
    assert_eq!(ci(value), FORMATTER);
    let i = unsafe { *iter } as usize;
    unsafe { *iter = ptr(i + 1) };
    if i == 0 { ptr(METHOD) } else { std::ptr::null() }
}
unsafe extern "C-unwind" fn method_name(_: *const raw::MethodInfo) -> *const c_char {
    metadata();
    c"DisplayStringForValue".as_ptr()
}
unsafe extern "C-unwind" fn method_class(_: *const raw::MethodInfo) -> Class {
    metadata();
    class(FORMATTER)
}
unsafe extern "C-unwind" fn method_false(_: *const raw::MethodInfo) -> bool {
    metadata();
    false
}
unsafe extern "C-unwind" fn method_count(_: *const raw::MethodInfo) -> u32 {
    metadata();
    5
}
unsafe extern "C-unwind" fn method_param(_: *const raw::MethodInfo, index: u32) -> *const raw::Type {
    metadata();
    ty([SINGLE, PROPERTY, MOD, BYTE, BOOLEAN][index as usize])
}
unsafe extern "C-unwind" fn method_return(_: *const raw::MethodInfo) -> *const raw::Type {
    metadata();
    ty(STRING)
}
unsafe extern "C-unwind" fn method_flags(_: *const raw::MethodInfo, _: *mut u32) -> u32 {
    metadata();
    if fault() == Fault::Signature { 0x410 } else { 0x10 }
}
unsafe extern "C-unwind" fn invoke(method: *const raw::MethodInfo, receiver: Ptr, args: *mut Ptr, error: *mut *mut raw::Exception) -> *mut raw::Object {
    metadata();
    assert_eq!(method as usize, METHOD);
    assert!(receiver.is_null());
    for o in [INPUT, PROPERTY_OBJECT, LIST_OBJECT, ENTRY1, ENTRY2] {
        rooted(o)
    }
    unsafe {
        let args = std::slice::from_raw_parts(args, 5);
        assert_eq!(*args[0].cast::<f32>(), S.with_borrow(|s| s.values[0]));
        assert_eq!(args[1] as usize, PROPERTY_OBJECT);
        assert_eq!(*args[2].cast::<i32>(), S.with_borrow(|s| s.modifier));
        assert_eq!(*args[3].cast::<u8>(), S.with_borrow(|s| s.tag));
        assert_eq!(*args[4].cast::<u8>(), 0);
    }
    S.with_borrow_mut(|s| s.invokes += 1);
    match fault() {
        Fault::NullReturn => std::ptr::null_mut(),
        Fault::Exception => {
            unsafe { *error = ptr(EXCEPTION) };
            std::ptr::null_mut()
        }
        _ => ptr(OUTPUT),
    }
}
unsafe extern "C-unwind" fn string_length(value: *mut raw::Object) -> i32 {
    metadata();
    assert_eq!(value as usize, OUTPUT);
    for o in [INPUT, PROPERTY_OBJECT, LIST_OBJECT, ENTRY1, ENTRY2, OUTPUT] {
        rooted(o)
    }
    if fault() == Fault::Oversized { 4097 } else { S.with_borrow(|s| s.text.len() as i32) }
}
unsafe extern "C-unwind" fn chars(value: *mut raw::Object) -> *mut u16 {
    metadata();
    rooted(value as usize);
    S.with_borrow_mut(|s| s.text.as_mut_ptr())
}
fn fixture() -> (RuntimeAccess, raw::InvocationApi) {
    S.with_borrow_mut(|s| {
        assert!(s.roots.is_empty());
        *s = State::new()
    });
    let mut a = RuntimeAccess::test_stubs();
    a.domain_get = domain;
    a.assemblies = assemblies;
    a.assembly_image = image;
    a.class_from_name = from_name;
    a.class_parent = parent;
    a.class_nested = nested;
    a.class_name = name;
    a.class_namespace = namespace;
    a.class_image = class_image;
    a.class_field = get_field;
    a.field_parent = field_parent;
    a.field_flags = field_flags;
    a.field_type = field_type;
    a.type_class = type_class;
    a.class_type = class_type;
    a.type_code = type_code;
    a.type_byref = byref;
    a.class_value = value_type;
    a.class_enum = is_enum;
    a.enum_base = enum_base;
    a.object_class = object_class;
    a.assignable = assignable;
    a.field_get = get;
    a.corlib = corlib;
    a.class_methods = methods;
    a.method_name = method_name;
    a.class_element = element;
    a.class_rank = rank;
    a.array_header_size = header;
    a.array_length_offset = length_offset;
    a.array_length = array_length;
    a.string_length = string_length;
    a.string_chars = chars;
    macro_rules! stub {
        ($t:ty) => {
            <$t as TestExportStub>::test_stub()
        };
    }
    let api = raw::InvocationApi {
        runtime_invoke: invoke,
        method_get_return_type: method_return,
        method_get_class: method_class,
        method_is_generic: method_false,
        method_is_inflated: method_false,
        method_is_instance: method_false,
        method_get_param_count: method_count,
        method_get_param: method_param,
        method_get_flags: method_flags,
        type_get_type: type_code,
        type_is_byref: byref,
        class_from_type: type_class,
        class_is_valuetype: value_type,
        class_is_generic: generic,
        class_is_inflated: inflated,
        class_is_assignable_from: assignable,
        object_get_class: object_class,
        object_unbox: stub!(raw::ObjectUnbox),
        object_get_virtual_method: stub!(raw::ObjectGetVirtualMethod),
        get_corlib: api_corlib,
        class_from_name: api_from_name,
        class_is_enum: is_enum,
        class_enum_basetype: enum_base,
        class_value_size: value_size,
    };
    (a, api)
}
fn run(a: &RuntimeAccess, api: raw::InvocationApi, f: impl FnOnce(&AttachedThread<'_>, &StrongRoot<'_, '_>)) {
    let ctx = context();
    let attached = ctx.attach().unwrap();
    let input = unsafe { attached.root(nn(INPUT)) }.unwrap();
    f(&attached, &input);
    drop(input);
    S.with_borrow(|s| assert!(s.roots.is_empty()));
    let _ = (a, api);
}

#[test]
fn full_entry_keeps_all_graph_roots_and_passes_exact_values_for_every_modifier() {
    let (a, api) = fixture();
    run(&a, api, |attached, input| {
        for modifier in 0..=3 {
            S.with_borrow_mut(|s| s.modifier = modifier);
            let out = unsafe { a.format_numeric_modifier(attached, api, input) }.unwrap();
            assert_eq!(out.value_bits, 1.25_f32.to_bits());
            assert_eq!(out.minimum_bits, 0_f32.to_bits());
            assert_eq!(out.maximum_bits, 2_f32.to_bits());
            assert!(out.varying);
            assert_eq!(out.modifier_type, modifier);
            assert_eq!(out.special_tag, 2);
            assert_eq!(out.formatted.display, "1.25");
            assert_eq!(out.formatted.unit, "");
            S.with_borrow(|s| {
                assert_eq!(s.roots.len(), 1);
                assert!(s.roots.contains_key(&INPUT))
            });
        }
    });
    assert_eq!(S.with_borrow(|s| s.invokes), 4);
}
#[test]
fn full_entry_preserves_inverted_ranges_and_signed_zero_comparison_without_normalizing_bits() {
    let (a, api) = fixture();
    run(&a, api, |attached, input| {
        for (minimum, maximum, varying) in [(3_f32, -2_f32, true), (-0_f32, 0_f32, false), (2_f32, 2_f32, false)] {
            S.with_borrow_mut(|s| s.values = [100.0, minimum, maximum]);
            let out = unsafe { a.format_numeric_modifier(attached, api, input) }.unwrap();
            assert_eq!(out.value_bits, 100_f32.to_bits());
            assert_eq!(out.minimum_bits, minimum.to_bits());
            assert_eq!(out.maximum_bits, maximum.to_bits());
            assert_eq!(out.varying, varying);
        }
    });
}
#[test]
fn full_entry_refuses_foreign_attachment_before_metadata_or_field_copy() {
    let (a, api) = fixture();
    let ctx = context();
    let attached = ctx.attach().unwrap();
    let input = unsafe { attached.root(nn(INPUT)) }.unwrap();
    let ctx2 = context();
    let attached2 = ctx2.attach().unwrap();
    assert!(matches!(unsafe { a.format_numeric_modifier(&attached2, api, &input) }, Err(Unavailable::Context)));
    S.with_borrow(|s| {
        assert_eq!(s.metadata, 0);
        assert!(s.reads.is_empty());
        assert_eq!(s.invokes, 0)
    });
}
#[test]
fn full_entry_requires_exact_modifier_identity_hierarchy_closed_reference_and_image() {
    for fault in [
        Fault::Parent,
        Fault::ObjectBase,
        Fault::Derived,
        Fault::Preformatted,
        Fault::ValueClass,
        Fault::Generic,
        Fault::Inflated,
        Fault::WrongImage,
    ] {
        let (a, api) = fixture();
        S.with_borrow_mut(|s| s.fault = fault);
        run(&a, api, |attached, input| assert!(unsafe { a.format_numeric_modifier(attached, api, input) }.is_err()));
        S.with_borrow(|s| {
            assert!(s.reads.is_empty());
            assert_eq!(s.invokes, 0)
        });
    }
}
#[test]
fn full_entry_checks_every_destination_before_copy_wrong_owner_flags_width_identity_or_byref() {
    for f in VALUE..=TAG {
        for fault in [
            Fault::Owner,
            Fault::Static,
            Fault::Literal,
            Fault::Readonly,
            Fault::FieldType,
            Fault::FieldIdentity,
            Fault::Byref,
            Fault::Missing,
        ] {
            let (a, api) = fixture();
            S.with_borrow_mut(|s| {
                s.fault = fault;
                s.field = f;
            });
            run(&a, api, |attached, input| {
                assert!(unsafe { a.format_numeric_modifier(attached, api, input) }.is_err(), "{f} {fault:?}")
            });
            S.with_borrow(|s| {
                assert!(s.reads.is_empty(), "{f} {fault:?}");
                assert_eq!(s.invokes, 0)
            });
        }
    }
}
#[test]
fn full_entry_requires_exact_int32_enum_and_never_queries_base_for_nonenum() {
    for fault in [Fault::NonEnum, Fault::EnumBase] {
        let (a, api) = fixture();
        S.with_borrow_mut(|s| s.fault = fault);
        run(&a, api, |attached, input| assert!(unsafe { a.format_numeric_modifier(attached, api, input) }.is_err()));
        S.with_borrow(|s| {
            assert!(s.reads.is_empty());
            assert_eq!(s.invokes, 0);
            assert_eq!(s.enum_bases, usize::from(fault == Fault::EnumBase))
        });
    }
}
#[test]
fn full_entry_roots_child_immediately_and_refuses_null_derived_or_failed_handle_before_scalars() {
    for fault in [Fault::NullProperty, Fault::DerivedProperty, Fault::ChildRoot] {
        let (a, api) = fixture();
        S.with_borrow_mut(|s| s.fault = fault);
        run(&a, api, |attached, input| assert!(unsafe { a.format_numeric_modifier(attached, api, input) }.is_err()));
        S.with_borrow(|s| {
            assert_eq!(s.reads, [PROPERTY_FIELD]);
            assert_eq!(s.invokes, 0);
            assert!(!s.pending_child)
        });
    }
}
#[test]
fn full_entry_refuses_every_nonfinite_endpoint_and_unknown_modtype_before_formatter() {
    for index in 0..3 {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (a, api) = fixture();
            S.with_borrow_mut(|s| s.values[index] = value);
            run(&a, api, |attached, input| {
                assert!(matches!(unsafe { a.format_numeric_modifier(attached, api, input) }, Err(Unavailable::Input)))
            });
            assert_eq!(S.with_borrow(|s| s.invokes), 0);
        }
    }
    for modifier in [-1, 4, i32::MAX] {
        let (a, api) = fixture();
        S.with_borrow_mut(|s| s.modifier = modifier);
        run(&a, api, |attached, input| {
            assert!(matches!(unsafe { a.format_numeric_modifier(attached, api, input) }, Err(Unavailable::Input)))
        });
        assert_eq!(S.with_borrow(|s| s.invokes), 0);
    }
}
#[test]
fn full_entry_reuses_complete_formatter_graph_and_signature_refusals_before_body_for_all_modtypes() {
    for fault in [Fault::BadList, Fault::BadOverride, Fault::Signature] {
        for modifier in 0..=3 {
            let (a, api) = fixture();
            S.with_borrow_mut(|s| {
                s.fault = fault;
                s.modifier = modifier
            });
            run(&a, api, |attached, input| assert!(unsafe { a.format_numeric_modifier(attached, api, input) }.is_err()));
            assert_eq!(S.with_borrow(|s| s.invokes), 0);
        }
    }
}
#[test]
fn full_entry_keeps_roots_and_reports_bounded_text_exception_or_result_root_failure_after_body() {
    for fault in [Fault::NullReturn, Fault::Exception, Fault::ReturnRoot, Fault::Utf16, Fault::Nul, Fault::Oversized] {
        let (a, api) = fixture();
        S.with_borrow_mut(|s| {
            s.fault = fault;
            if fault == Fault::Utf16 {
                s.text = vec![0xd800]
            }
            if fault == Fault::Nul {
                s.text = vec![0]
            }
        });
        run(&a, api, |attached, input| {
            assert!(unsafe { a.format_numeric_modifier(attached, api, input) }.is_err(), "{fault:?}")
        });
        assert_eq!(S.with_borrow(|s| s.invokes), 1);
    }
    let (a, api) = fixture();
    S.with_borrow_mut(|s| s.text = "12.5%".encode_utf16().collect());
    run(&a, api, |attached, input| {
        let out = unsafe { a.format_numeric_modifier(attached, api, input) }.unwrap();
        assert_eq!(out.formatted.unit, "%");
    });
}
