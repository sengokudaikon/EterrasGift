//! Authored exports use identity tokens only; target object addresses are never dereferenced.
use super::*;
use crate::game::{Class, Ptr};
use eg_il2cpp_runtime::{ThreadContext, raw};
use eg_trainer_wire::fields::{AttackLeechUnavailable, ObservedObjectIdentity};
use std::ffi::CStr;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ffi::c_char,
};

const FINDER: usize = 101;
const MANAGER_CLASS: usize = 102;
const ACTOR_CLASS: usize = 103;
const BASE: usize = 104;
const CHARACTER: usize = 105;
const OBJECT: usize = 106;
const ENVIRONMENT: usize = 107;
const SINGLE: usize = 108;
const BOOLEAN: usize = 109;
const INTPTR: usize = 110;
const UNSUPPORTED_BASE: usize = 111;
const SHADOW_CHARACTER: usize = 112;
const WRONG_PRIMITIVE: usize = 113;
const MANAGER: usize = 0x10000;
const ACTOR: usize = 0x20000;
const STATS: usize = 0x30000;

const ONLINE: usize = 201;
const MANAGER_STATIC: usize = 202;
const ACTOR_STATIC: usize = 203;
const CACHED: usize = 204;
const MANAGER_ACTOR: usize = 205;
const ACTOR_MANAGER: usize = 206;
const ACTOR_STATS: usize = 207;
const STATS_ACTOR: usize = 208;
const ATTACK: usize = 209;
const LEECH: usize = 210;
const TYPE_BASE: usize = 1000;

#[derive(Clone, Copy, Debug)]
enum Fault {
    Type(usize),
    Flags(i32),
    Parent(usize),
    ByRef,
    ThreadStatic,
    Missing,
}

#[derive(Clone, Copy, Debug, Default)]
enum StoreMode {
    #[default]
    Normal,
    Ignore,
    OtherBits,
    Online,
    MissingManager,
    ReplacedManager,
    ManagerNative,
    DeadManager,
    ActorEdge,
    StatsEdge,
    DeadActor,
    DeadStats,
    LostRootTarget,
    InvalidBoolean,
}

#[derive(Clone, Copy, Debug, Default)]
enum Drift {
    #[default]
    None,
    Online,
    Manager,
    ReplacedManager,
    ManagerNative,
    DeadManager,
    Actor,
    Stats,
    ManagerActor,
    ActorManager,
    StatsActor,
    Native,
    Dead,
}

#[derive(Clone, Debug)]
struct Graph {
    online: u8,
    manager: usize,
    actor: usize,
    stats: usize,
    manager_actor: usize,
    actor_manager: usize,
    stats_actor: usize,
    native: [usize; 3],
    stats_class: usize,
    attack: u32,
    leech: u32,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            online: 0,
            manager: MANAGER,
            actor: ACTOR,
            stats: STATS,
            manager_actor: ACTOR,
            actor_manager: MANAGER,
            stats_actor: ACTOR,
            native: [0xa000, 0xb000, 0xc000],
            stats_class: CHARACTER,
            attack: 0x3f8ccccd,
            leech: 0xbe000000,
        }
    }
}

thread_local! {
    static GRAPH: RefCell<Graph> = RefCell::new(Graph::default());
    static ROOTS: RefCell<BTreeMap<usize, u32>> = const { RefCell::new(BTreeMap::new()) };
    static FAULT: Cell<Option<(usize, Fault)>> = const { Cell::new(None) };
    static VALUE_CLASS: Cell<usize> = const { Cell::new(0) };
    static SINGLE_CODE: Cell<i32> = const { Cell::new(0x0c) };
    static BOOLEAN_CODE: Cell<i32> = const { Cell::new(0x02) };
    static INTPTR_CODE: Cell<i32> = const { Cell::new(0x18) };
    static NONVALUE_PRIMITIVE: Cell<usize> = const { Cell::new(0) };
    static NULL_PRIMITIVE_TYPE: Cell<usize> = const { Cell::new(0) };
    static BYREF_PRIMITIVE: Cell<usize> = const { Cell::new(0) };
    static FAIL_ROOT: Cell<usize> = const { Cell::new(0) };
    static RATE_READS: Cell<u32> = const { Cell::new(0) };
    static POINTER_READS: Cell<u32> = const { Cell::new(0) };
    static DRIFT: Cell<Drift> = const { Cell::new(Drift::None) };
    static ASSEMBLIES: [Ptr; 1] = const { [std::ptr::without_provenance_mut(9)] };
    static STORES: Cell<u32> = const { Cell::new(0) };
    static LAST_STORE: Cell<(usize,u32)> = const { Cell::new((0,0)) };
    static STORE_MODE: Cell<StoreMode> = const { Cell::new(StoreMode::Normal) };
    static LOST_TARGET: Cell<usize> = const { Cell::new(0) };
    static FIELD_CODE_FAULT: Cell<Option<(usize,i32)>> = const { Cell::new(None) };
}

fn token<T>(id: usize) -> *mut T {
    std::ptr::without_provenance_mut(id)
}
fn fault(field: usize) -> Option<Fault> {
    FAULT.get().filter(|(id, _)| *id == field).map(|(_, fault)| fault)
}
fn rooted(object: usize) {
    ROOTS.with_borrow(|roots| assert!(roots.get(&object).is_some_and(|count| *count != 0), "unrooted {object:x}"));
}

fn field_schema(field: usize) -> (usize, usize, bool) {
    match field {
        ONLINE => (ENVIRONMENT, BOOLEAN, true),
        MANAGER_STATIC => (FINDER, MANAGER_CLASS, true),
        ACTOR_STATIC => (FINDER, ACTOR_CLASS, true),
        CACHED => (OBJECT, INTPTR, false),
        MANAGER_ACTOR => (MANAGER_CLASS, ACTOR_CLASS, false),
        ACTOR_MANAGER => (ACTOR_CLASS, MANAGER_CLASS, false),
        ACTOR_STATS => (ACTOR_CLASS, BASE, false),
        STATS_ACTOR => (BASE, ACTOR_CLASS, false),
        ATTACK => (CHARACTER, SINGLE, false),
        LEECH => (BASE, SINGLE, false),
        _ => panic!("unexpected field {field}"),
    }
}

unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
    token(1)
}
unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
    token(2)
}
unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
    panic!("already attached")
}
unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
    panic!("borrowed attachment")
}
unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> Ptr {
    assert!(!pinned);
    if FAIL_ROOT.get() == object as usize {
        return std::ptr::null_mut();
    }
    ROOTS.with_borrow_mut(|roots| *roots.entry(object as usize).or_default() += 1);
    Box::into_raw(Box::new(object)).cast()
}
unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
    // SAFETY: handle owns one initialized Box pointer slot until its sole root_free.
    let object = unsafe { handle.cast::<*mut raw::Object>().read() };
    if object as usize == LOST_TARGET.get() { std::ptr::null_mut() } else { object }
}
unsafe extern "C-unwind" fn root_free(handle: Ptr) {
    // SAFETY: StrongRoot frees exactly once, after its last target access.
    let object = unsafe { Box::from_raw(handle.cast::<*mut raw::Object>()) };
    ROOTS.with_borrow_mut(|roots| {
        let count = roots.get_mut(&(*object as usize)).unwrap();
        *count -= 1;
        if *count == 0 {
            roots.remove(&(*object as usize));
        }
    });
}
unsafe extern "C-unwind" fn assemblies(_: *const raw::Domain, count: *mut usize) -> *const Ptr {
    // SAFETY: images() supplies one initialized writable usize count slot.
    unsafe {
        count.write(1);
    }
    ASSEMBLIES.with(|values| values.as_ptr())
}
unsafe extern "C-unwind" fn assembly_image(_: Ptr) -> Ptr {
    token(10)
}
unsafe extern "C-unwind" fn corlib() -> *const std::ffi::c_void {
    token::<std::ffi::c_void>(11).cast_const()
}
unsafe extern "C-unwind" fn class_from_name(_: *const std::ffi::c_void, namespace: *const c_char, name: *const c_char) -> Class {
    // SAFETY: adapter supplies static, nul-terminated metadata names.
    let (namespace, name) = unsafe { (CStr::from_ptr(namespace), CStr::from_ptr(name)) };
    token(match (namespace, name) {
        (ns, name) if ns == c"" && name == c"PlayerFinder" => FINDER,
        (ns, name) if ns == c"" && name == c"ItemContainersManager" => MANAGER_CLASS,
        (ns, name) if ns == c"" && name == c"Actor" => ACTOR_CLASS,
        (ns, name) if ns == c"" && name == c"BaseStats" => BASE,
        (ns, name) if ns == c"" && name == c"CharacterStats" => CHARACTER,
        (ns, name) if ns == c"EHG.Multiplayer" && name == c"GameplayEnvironment" => ENVIRONMENT,
        (ns, name) if ns == c"UnityEngine" && name == c"Object" => OBJECT,
        (ns, name) if ns == c"System" && name == c"Boolean" => BOOLEAN,
        (ns, name) if ns == c"System" && name == c"Single" => SINGLE,
        (ns, name) if ns == c"System" && name == c"IntPtr" => INTPTR,
        _ => panic!("unexpected metadata lookup {namespace:?}.{name:?}"),
    })
}
unsafe extern "C-unwind" fn class_value(class: *const raw::Class) -> bool {
    class as usize != NONVALUE_PRIMITIVE.get() && [SINGLE, BOOLEAN, INTPTR, VALUE_CLASS.get()].contains(&(class as usize))
}
unsafe extern "C-unwind" fn class_parent(_: Class) -> Class {
    std::ptr::null_mut()
}
unsafe extern "C-unwind" fn class_field(owner: Class, name: *const c_char) -> Ptr {
    // SAFETY: known metadata names supplied by the reader.
    let name = unsafe { CStr::from_ptr(name) };
    let field = match (owner as usize, name) {
        (ENVIRONMENT, name) if name == c"_isOnlinePlay" => ONLINE,
        (FINDER, name) if name == c"localItemContainersManager" => MANAGER_STATIC,
        (FINDER, name) if name == c"playerActorComponent" => ACTOR_STATIC,
        (OBJECT, name) if name == c"m_CachedPtr" => CACHED,
        (MANAGER_CLASS, name) if name == c"actor" => MANAGER_ACTOR,
        (ACTOR_CLASS, name) if name == c"itemContainersManager" => ACTOR_MANAGER,
        (ACTOR_CLASS, name) if name == c"stats" => ACTOR_STATS,
        (BASE, name) if name == c"actor" => STATS_ACTOR,
        (CHARACTER, name) if name == c"attackRate" => ATTACK,
        (BASE, name) if name == c"increasedLeechRate" => LEECH,
        _ => panic!("unexpected declared owner/name {owner:p}/{name:?}"),
    };
    if matches!(fault(field), Some(Fault::Missing)) {
        return std::ptr::null_mut();
    }
    token(field)
}
unsafe extern "C-unwind" fn class_type(class: Class) -> crate::game::Type {
    if class as usize == NULL_PRIMITIVE_TYPE.get() {
        std::ptr::null()
    } else {
        class.cast_const().cast()
    }
}
unsafe extern "C-unwind" fn type_code(ty: crate::game::Type) -> i32 {
    if ty as usize >= TYPE_BASE {
        let field = ty as usize - TYPE_BASE;
        if let Some((selected, code)) = FIELD_CODE_FAULT.get()
            && selected == field
        {
            return code;
        }
        return match field_schema(field).1 {
            SINGLE => 0x0c,
            BOOLEAN => 0x02,
            INTPTR => 0x18,
            _ => 0x12,
        };
    }
    match ty as usize {
        SINGLE => SINGLE_CODE.get(),
        BOOLEAN => BOOLEAN_CODE.get(),
        INTPTR => INTPTR_CODE.get(),
        _ => panic!("unexpected primitive type"),
    }
}
unsafe extern "C-unwind" fn field_type(field: Ptr) -> crate::game::Type {
    token(TYPE_BASE + field as usize)
}
unsafe extern "C-unwind" fn type_class(ty: crate::game::Type) -> Class {
    let field = ty as usize - TYPE_BASE;
    token(match fault(field) {
        Some(Fault::Type(class)) => class,
        _ => field_schema(field).1,
    })
}
unsafe extern "C-unwind" fn byref(ty: crate::game::Type) -> bool {
    if (ty as usize) < TYPE_BASE {
        ty as usize == BYREF_PRIMITIVE.get()
    } else {
        matches!(fault(ty as usize - TYPE_BASE), Some(Fault::ByRef))
    }
}
unsafe extern "C-unwind" fn parent(field: Ptr) -> Class {
    token(match fault(field as usize) {
        Some(Fault::Parent(class)) => class,
        _ => field_schema(field as usize).0,
    })
}
unsafe extern "C-unwind" fn flags(field: Ptr) -> i32 {
    match fault(field as usize) {
        Some(Fault::Flags(flags)) => flags,
        _ => {
            if field_schema(field as usize).2 {
                0x10
            } else {
                0
            }
        }
    }
}
unsafe extern "C-unwind" fn offset(field: Ptr) -> usize {
    if matches!(fault(field as usize), Some(Fault::ThreadStatic)) { usize::MAX } else { 128 }
}
unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
    rooted(object as usize);
    token(match object as usize {
        MANAGER => MANAGER_CLASS,
        ACTOR => ACTOR_CLASS,
        STATS => GRAPH.with_borrow(|graph| graph.stats_class),
        _ => panic!("unexpected object"),
    })
}
unsafe extern "C-unwind" fn assignable(expected: Class, actual: Class) -> bool {
    let (expected, actual) = (expected as usize, actual as usize);
    expected == actual
        || (expected == OBJECT && [MANAGER_CLASS, ACTOR_CLASS, BASE, CHARACTER, UNSUPPORTED_BASE, SHADOW_CHARACTER].contains(&actual))
        || (expected == BASE && [CHARACTER, UNSUPPORTED_BASE, SHADOW_CHARACTER].contains(&actual))
}
unsafe extern "C-unwind" fn static_get(field: Ptr, destination: Ptr) {
    assert!(fault(field as usize).is_none(), "static destination not validated");
    GRAPH.with_borrow(|graph| match field as usize {
        ONLINE => {
            // SAFETY: exact Boolean identity was validated before its u8 destination.
            unsafe {
                destination.cast::<u8>().write(graph.online);
            }
        }
        MANAGER_STATIC | ACTOR_STATIC => {
            POINTER_READS.set(POINTER_READS.get() + 1);
            // SAFETY: metadata admits only a reference-typed pointer-sized destination.
            unsafe {
                destination
                    .cast::<*mut raw::Object>()
                    .write(token(if field as usize == MANAGER_STATIC { graph.manager } else { graph.actor }));
            }
        }
        _ => panic!("unexpected static field"),
    });
}
unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, field: Ptr, destination: Ptr) {
    rooted(object as usize);
    assert!(fault(field as usize).is_none(), "instance destination not validated");
    GRAPH.with_borrow(|graph| match field as usize {
        CACHED => {
            let native = graph.native[match object as usize {
                MANAGER => 0,
                ACTOR => 1,
                STATS => 2,
                _ => panic!("unexpected cached wrapper"),
            }];
            // SAFETY: exact IntPtr metadata guarantees one writable usize slot.
            unsafe {
                destination.cast::<usize>().write(native);
            }
        }
        MANAGER_ACTOR | ACTOR_MANAGER | ACTOR_STATS | STATS_ACTOR => {
            let pointer = match field as usize {
                MANAGER_ACTOR => graph.manager_actor,
                ACTOR_MANAGER => graph.actor_manager,
                ACTOR_STATS => graph.stats,
                STATS_ACTOR => graph.stats_actor,
                _ => unreachable!(),
            };
            // SAFETY: exact reference metadata guarantees one writable pointer slot.
            unsafe {
                destination.cast::<*mut raw::Object>().write(token(pointer));
            }
        }
        ATTACK | LEECH => {
            assert_eq!(object as usize, STATS);
            ROOTS.with_borrow(|roots| assert_eq!(roots.len(), 3));
            RATE_READS.set(RATE_READS.get() + 1);
            // SAFETY: exact Single/R4 metadata guarantees a writable four-byte slot.
            unsafe {
                destination
                    .cast::<f32>()
                    .write(f32::from_bits(if field as usize == ATTACK { graph.attack } else { graph.leech }));
            }
        }
        _ => panic!("unexpected instance field"),
    });
    if field as usize == LEECH {
        // Deterministic fake-export seam, not a concurrent real-object data race.
        GRAPH.with_borrow_mut(|graph| match DRIFT.get() {
            Drift::None => {}
            Drift::Online => graph.online = 1,
            Drift::Manager => graph.manager = 0,
            Drift::ReplacedManager => graph.manager = MANAGER + 0x1000,
            Drift::ManagerNative => graph.native[0] += 8,
            Drift::DeadManager => graph.native[0] = 0,
            Drift::Actor => graph.actor = 0,
            Drift::Stats => graph.stats = 0,
            Drift::ManagerActor => graph.manager_actor = 0,
            Drift::ActorManager => graph.actor_manager = 0,
            Drift::StatsActor => graph.stats_actor = 0,
            Drift::Native => graph.native[2] += 8,
            Drift::Dead => graph.native[2] = 0,
        });
    }
}

fn fixture() -> (RuntimeAccess, ThreadContext) {
    ROOTS.with_borrow(|roots| assert!(roots.is_empty()));
    GRAPH.set(Graph::default());
    FAULT.set(None);
    VALUE_CLASS.set(0);
    SINGLE_CODE.set(0x0c);
    BOOLEAN_CODE.set(0x02);
    INTPTR_CODE.set(0x18);
    NONVALUE_PRIMITIVE.set(0);
    NULL_PRIMITIVE_TYPE.set(0);
    BYREF_PRIMITIVE.set(0);
    FAIL_ROOT.set(0);
    RATE_READS.set(0);
    POINTER_READS.set(0);
    DRIFT.set(Drift::None);
    STORES.set(0);
    LAST_STORE.set((0, 0));
    STORE_MODE.set(StoreMode::Normal);
    LOST_TARGET.set(0);
    FIELD_CODE_FAULT.set(None);
    let mut api = RuntimeAccess::test_stubs();
    api.domain_get = domain;
    api.assemblies = assemblies;
    api.assembly_image = assembly_image;
    api.corlib = corlib;
    api.class_from_name = class_from_name;
    api.class_value = class_value;
    api.class_field = class_field;
    api.class_parent = class_parent;
    api.class_type = class_type;
    api.type_code = type_code;
    api.field_type = field_type;
    api.type_class = type_class;
    api.type_byref = byref;
    api.field_parent = parent;
    api.field_flags = flags;
    api.field_offset = offset;
    api.object_class = object_class;
    api.assignable = assignable;
    api.static_get = static_get;
    api.field_get = field_get;
    // SAFETY: fake APIs implement one borrowed attached thread and owned Box roots;
    // no target process, raw object memory or foreign runtime is accessed.
    let thread = unsafe {
        ThreadContext::new(raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: attach,
            thread_detach: detach,
            gchandle_new: root,
            gchandle_free: root_free,
            gchandle_get_target: root_target,
        })
    };
    (api, thread)
}

fn read(api: &RuntimeAccess, thread: &ThreadContext, expected: u64) -> Result<AttackLeechRead, GameError> {
    let attached = thread.attach().unwrap();
    // SAFETY: deterministic same-thread owned fake graph/metadata; every export
    // validates destination width and root presence, without dereferencing tokens.
    let result = unsafe { api.read_attack_leech(&attached, expected) };
    ROOTS.with_borrow(|roots| assert!(roots.is_empty(), "root leaked after read"));
    result
}

#[test]
fn full_reader_owns_exact_raw_snapshot_and_releases_all_roots() {
    let (api, thread) = fixture();
    let AttackLeechRead::Available(value) = read(&api, &thread, MANAGER as u64).unwrap() else {
        panic!("not available")
    };
    assert_eq!(
        (value.manager.wrapper_token(), value.actor.wrapper_token(), value.stats.wrapper_token()),
        (MANAGER as u64, ACTOR as u64, STATS as u64)
    );
    assert_eq!(
        (value.manager.native_token(), value.actor.native_token(), value.stats.native_token()),
        (0xa000, 0xb000, 0xc000)
    );
    assert_eq!((value.attack_rate.raw_bits, value.increased_leech_rate.raw_bits), (0x3f8ccccd, 0xbe000000));
    assert_eq!(RATE_READS.get(), 2);
    for bits in [0, 0x80000000, 0x3c23d70a, 0x3c23d709, 0x7f800000, 0x7fc12345, 101_f32.to_bits()] {
        GRAPH.with_borrow_mut(|graph| {
            graph.attack = bits;
            graph.leech = bits;
        });
        let AttackLeechRead::Available(snapshot) = read(&api, &thread, MANAGER as u64).unwrap() else {
            panic!("raw value rejected")
        };
        assert_eq!((snapshot.attack_rate.raw_bits, snapshot.increased_leech_rate.raw_bits), (bits, bits));
    }
}

#[test]
fn offline_and_graph_absence_are_typed_before_rate_copy() {
    use AttackLeechUnavailable::*;
    let (api, thread) = fixture();
    for (change, reason) in [
        (0, OfflineRequired),
        (1, ManagerAbsent),
        (2, ActorCacheAbsent),
        (3, StatsAbsent),
        (4, GraphChanged),
        (5, GraphChanged),
        (6, GraphChanged),
    ] {
        GRAPH.set(Graph::default());
        GRAPH.with_borrow_mut(|graph| match change {
            0 => graph.online = 1,
            1 => graph.manager = 0,
            2 => graph.actor = 0,
            3 => graph.stats = 0,
            4 => graph.manager_actor = 0,
            5 => graph.actor_manager = 0,
            6 => graph.stats_actor = 0,
            _ => unreachable!(),
        });
        assert_eq!(read(&api, &thread, MANAGER as u64).unwrap(), AttackLeechRead::Unavailable(reason));
        assert_eq!(RATE_READS.get(), 0);
    }
    GRAPH.set(Graph::default());
    for expected in [0, MANAGER as u64 + 1] {
        assert_eq!(read(&api, &thread, expected).unwrap(), AttackLeechRead::Unavailable(ManagerChanged));
    }
    for index in 0..3 {
        GRAPH.set(Graph::default());
        GRAPH.with_borrow_mut(|graph| graph.native[index] = 0);
        let expected = if index == 0 { ManagerAbsent } else { NativeObjectDestroyed };
        assert_eq!(read(&api, &thread, MANAGER as u64).unwrap(), AttackLeechRead::Unavailable(expected));
    }
    assert_eq!(RATE_READS.get(), 0);
}

#[test]
fn valid_unsupported_stats_classes_are_unavailable_while_wrong_reference_class_is_contract() {
    let (api, thread) = fixture();
    for class in [BASE, UNSUPPORTED_BASE, SHADOW_CHARACTER] {
        GRAPH.with_borrow_mut(|graph| graph.stats_class = class);
        assert_eq!(
            read(&api, &thread, MANAGER as u64).unwrap(),
            AttackLeechRead::Unavailable(AttackLeechUnavailable::UnsupportedStatsClass)
        );
        assert_eq!(RATE_READS.get(), 0);
    }
    GRAPH.with_borrow_mut(|graph| graph.stats_class = ACTOR_CLASS);
    assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
    assert_eq!(RATE_READS.get(), 0);
}

#[test]
fn wrong_field_width_declaring_owner_storage_and_reference_shape_never_reach_destination() {
    let (api, thread) = fixture();
    for field in [
        ONLINE,
        MANAGER_STATIC,
        ACTOR_STATIC,
        CACHED,
        MANAGER_ACTOR,
        ACTOR_MANAGER,
        ACTOR_STATS,
        STATS_ACTOR,
        ATTACK,
        LEECH,
    ] {
        let static_field = field_schema(field).2;
        for fault in [
            Fault::Type(WRONG_PRIMITIVE),
            Fault::Parent(FINDER + 500),
            Fault::ByRef,
            Fault::ThreadStatic,
            Fault::Missing,
            Fault::Flags(if static_field { 0 } else { 0x10 }),
            Fault::Flags(0x40),
        ] {
            FAULT.set(Some((field, fault)));
            assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))), "field {field}, {fault:?}");
            assert_eq!(RATE_READS.get(), 0);
        }
    }
    FAULT.set(None);
    for class in [FINDER, MANAGER_CLASS, ACTOR_CLASS, BASE, CHARACTER, OBJECT] {
        VALUE_CLASS.set(class);
        POINTER_READS.set(0);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        assert_eq!(POINTER_READS.get(), 0);
    }
    VALUE_CLASS.set(0);
    SINGLE_CODE.set(0x0d);
    assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
    GRAPH.with_borrow_mut(|graph| graph.online = 2);
    SINGLE_CODE.set(0x0c);
    assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
    assert_eq!(RATE_READS.get(), 0);
}

#[test]
fn end_gate_rejects_each_identity_edge_and_native_lifetime_drift_without_partial_result() {
    let (api, thread) = fixture();
    for drift in [
        Drift::Online,
        Drift::Manager,
        Drift::ReplacedManager,
        Drift::ManagerNative,
        Drift::DeadManager,
        Drift::Actor,
        Drift::Stats,
        Drift::ManagerActor,
        Drift::ActorManager,
        Drift::StatsActor,
        Drift::Native,
        Drift::Dead,
    ] {
        GRAPH.set(Graph::default());
        DRIFT.set(drift);
        let expected = match drift {
            Drift::Online => AttackLeechUnavailable::OfflineRequired,
            Drift::Manager | Drift::DeadManager => AttackLeechUnavailable::ManagerAbsent,
            Drift::ReplacedManager | Drift::ManagerNative => AttackLeechUnavailable::ManagerChanged,
            Drift::Dead => AttackLeechUnavailable::NativeObjectDestroyed,
            _ => AttackLeechUnavailable::GraphChanged,
        };
        assert_eq!(read(&api, &thread, MANAGER as u64).unwrap(), AttackLeechRead::Unavailable(expected), "{drift:?}");
    }
    GRAPH.set(Graph::default());
    DRIFT.set(Drift::None);
    assert!(matches!(read(&api, &thread, MANAGER as u64).unwrap(), AttackLeechRead::Available(_)));
}

#[test]
fn failed_root_acquisition_releases_previously_retained_parents_without_rate_read() {
    let (api, thread) = fixture();
    for object in [MANAGER, ACTOR, STATS] {
        FAIL_ROOT.set(object);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Runtime(_))));
        assert_eq!(RATE_READS.get(), 0);
    }
}

#[test]
fn boolean_intptr_and_single_must_have_exact_value_type_codes_before_any_copy() {
    let (api, thread) = fixture();
    for class in [BOOLEAN, INTPTR, SINGLE] {
        NONVALUE_PRIMITIVE.set(class);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        NONVALUE_PRIMITIVE.set(0);
        NULL_PRIMITIVE_TYPE.set(class);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        NULL_PRIMITIVE_TYPE.set(0);
        BYREF_PRIMITIVE.set(class);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        BYREF_PRIMITIVE.set(0);
    }
    for wrong in [0, 0x0d, 0x12, i32::MAX] {
        BOOLEAN_CODE.set(wrong);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        BOOLEAN_CODE.set(0x02);
        INTPTR_CODE.set(wrong);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        INTPTR_CODE.set(0x18);
        SINGLE_CODE.set(wrong);
        assert!(matches!(read(&api, &thread, MANAGER as u64), Err(GameError::Contract(_))));
        SINGLE_CODE.set(0x0c);
    }
    assert_eq!(POINTER_READS.get(), 0);
    assert_eq!(RATE_READS.get(), 0);
}

unsafe extern "C-unwind" fn set_rate(object: *mut raw::Object, field: *mut raw::FieldInfo, value: Ptr) {
    assert_eq!(object as usize, STATS);
    rooted(MANAGER);
    rooted(ACTOR);
    rooted(STATS);
    ROOTS.with_borrow(|roots| assert_eq!(roots.len(), 3));
    assert!(fault(field as usize).is_none());
    assert!([ATTACK, LEECH].contains(&(field as usize)));
    assert!(FIELD_CODE_FAULT.get().is_none());
    // SAFETY: the native candidate passes one initialized aligned f32 value slot
    // after exact mutable Single/R4 validation. No object token is dereferenced.
    let bits = unsafe { value.cast::<f32>().read().to_bits() };
    STORES.set(STORES.get() + 1);
    LAST_STORE.set((field as usize, bits));
    GRAPH.with_borrow_mut(|graph| {
        if !matches!(STORE_MODE.get(), StoreMode::Ignore) {
            if field as usize == ATTACK {
                graph.attack = bits;
            } else {
                graph.leech = bits;
            }
        }
        match STORE_MODE.get() {
            StoreMode::Normal | StoreMode::Ignore => {}
            StoreMode::OtherBits => {
                if field as usize == ATTACK {
                    graph.leech ^= 1;
                } else {
                    graph.attack ^= 1;
                }
            }
            StoreMode::Online => graph.online = 1,
            StoreMode::MissingManager => graph.manager = 0,
            StoreMode::ReplacedManager => graph.manager = MANAGER + 0x1000,
            StoreMode::ManagerNative => graph.native[0] += 8,
            StoreMode::DeadManager => graph.native[0] = 0,
            StoreMode::ActorEdge => graph.actor_manager = 0,
            StoreMode::StatsEdge => graph.stats_actor = 0,
            StoreMode::DeadActor => graph.native[1] = 0,
            StoreMode::DeadStats => graph.native[2] = 0,
            StoreMode::LostRootTarget => LOST_TARGET.set(STATS),
            StoreMode::InvalidBoolean => graph.online = 2,
        }
    });
}

fn current_observation(api: &RuntimeAccess, thread: &ThreadContext) -> AttackLeechObservation {
    let AttackLeechRead::Available(observation) = read(api, thread, MANAGER as u64).unwrap() else {
        panic!("expected available graph")
    };
    observation
}

fn write_rate(api: &RuntimeAccess, thread: &ThreadContext, expected: &AttackLeechObservation, which: AttackLeechTarget, value: f32) -> Result<AttackLeechObservation, WriteError> {
    let attached = thread.attach().unwrap();
    // SAFETY: owned same-thread fake graph/runtime, exact typed export behavior,
    // explicit single current-field mutation authority; no process operations.
    let result = unsafe { api.set_attack_leech(&attached, raw::FieldWriteApi::test_export(set_rate), expected, which, value) };
    ROOTS.with_borrow(|roots| assert!(roots.is_empty(), "writer roots leaked"));
    result
}

#[test]
fn full_read_store_full_read_path_changes_exactly_one_f32_with_all_roots_retained() {
    for (which, field, value) in [
        (AttackLeechTarget::AttackRate, ATTACK, 1.375_f32),
        (AttackLeechTarget::IncreasedLeechRate, LEECH, -0.375_f32),
    ] {
        let (api, thread) = fixture();
        let before = current_observation(&api, &thread);
        let after = write_rate(&api, &thread, &before, which, value).unwrap();
        assert_eq!(STORES.get(), 1);
        assert_eq!(LAST_STORE.get(), (field, value.to_bits()));
        assert_eq!((after.manager, after.actor, after.stats), (before.manager, before.actor, before.stats));
        if field == ATTACK {
            assert_eq!(after.attack_rate.raw_bits, value.to_bits());
            assert_eq!(after.increased_leech_rate, before.increased_leech_rate);
        } else {
            assert_eq!(after.increased_leech_rate.raw_bits, value.to_bits());
            assert_eq!(after.attack_rate, before.attack_rate);
        }
        assert_eq!(current_observation(&api, &thread), after);
        // The old expectation is stale because this store changed its value bits.
        // This is not a consumed ticket: restoring baseline bits could make an
        // identical request succeed. The primitive never retries or re-applies.
        assert!(matches!(write_rate(&api, &thread, &before, which, value), Err(WriteError::Stale(_))));
        assert_eq!(STORES.get(), 1);
    }
}

#[test]
fn invalid_desired_values_and_exact_noops_refuse_before_any_game_metadata() {
    let (api, thread) = fixture();
    let expected = current_observation(&api, &thread);
    POINTER_READS.set(0);
    RATE_READS.set(0);
    for value in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        f32::from_bits(0x3c23d709),
        f32::from_bits(20_f32.to_bits() + 1),
    ] {
        assert!(matches!(
            write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, value),
            Err(WriteError::Refused(_))
        ));
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.00001, 100.00001] {
        assert!(matches!(
            write_rate(&api, &thread, &expected, AttackLeechTarget::IncreasedLeechRate, value),
            Err(WriteError::Refused(_))
        ));
    }
    assert!(matches!(
        write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, f32::from_bits(expected.attack_rate.raw_bits)),
        Err(WriteError::Refused(_))
    ));
    assert_eq!((POINTER_READS.get(), RATE_READS.get(), STORES.get()), (0, 0, 0));
}

#[test]
fn all_six_expected_tokens_and_both_old_bitpatterns_are_compared_before_store() {
    let (api, thread) = fixture();
    let original = current_observation(&api, &thread);
    for part in 0..8 {
        let mut expected = original;
        let identity = match part {
            0 | 1 => &mut expected.manager,
            2 | 3 => &mut expected.actor,
            4 | 5 => &mut expected.stats,
            _ => &mut expected.manager,
        };
        if part < 6 {
            *identity = ObservedObjectIdentity::new(
                identity.wrapper_token() ^ if part % 2 == 0 { 8 } else { 0 },
                identity.native_token() ^ if part % 2 == 1 { 8 } else { 0 },
            )
            .unwrap();
        } else if part == 6 {
            expected.attack_rate.raw_bits ^= 1;
        } else {
            expected.increased_leech_rate.raw_bits ^= 1;
        }
        assert!(
            matches!(write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, 2.5), Err(WriteError::Stale(_))),
            "part {part}"
        );
        assert_eq!(STORES.get(), 0);
    }
}

#[test]
fn readonly_static_literal_byref_wrong_owner_and_wrong_r4_metadata_never_store() {
    let (api, thread) = fixture();
    let expected = current_observation(&api, &thread);
    for (which, field) in [(AttackLeechTarget::AttackRate, ATTACK), (AttackLeechTarget::IncreasedLeechRate, LEECH)] {
        for failure in [
            Fault::Flags(0x20),
            Fault::Flags(0x10),
            Fault::Flags(0x40),
            Fault::ByRef,
            Fault::Type(WRONG_PRIMITIVE),
            Fault::Parent(OBJECT),
            Fault::ThreadStatic,
            Fault::Missing,
        ] {
            FAULT.set(Some((field, failure)));
            assert!(
                matches!(write_rate(&api, &thread, &expected, which, 2.5), Err(WriteError::Refused(_))),
                "{which:?}/{failure:?}"
            );
            assert_eq!(STORES.get(), 0);
        }
        FAULT.set(None);
        FIELD_CODE_FAULT.set(Some((field, 0x0d)));
        assert!(matches!(write_rate(&api, &thread, &expected, which, 2.5), Err(WriteError::Refused(_))));
        FIELD_CODE_FAULT.set(None);
        assert_eq!(STORES.get(), 0);
    }
}

#[test]
fn prestore_offline_loading_liveness_drift_and_root_failures_are_definite_no_store() {
    let (api, thread) = fixture();
    let expected = current_observation(&api, &thread);
    GRAPH.with_borrow_mut(|graph| graph.online = 1);
    assert!(matches!(write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, 2.5), Err(WriteError::Offline)));
    for object in [MANAGER, ACTOR, STATS] {
        GRAPH.set(Graph::default());
        FAIL_ROOT.set(object);
        assert!(matches!(
            write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, 2.5),
            Err(WriteError::NotReady(_))
        ));
    }
    FAIL_ROOT.set(0);
    for case in 0..8 {
        GRAPH.set(Graph::default());
        GRAPH.with_borrow_mut(|graph| match case {
            0 => graph.manager = 0,
            1 => graph.actor = 0,
            2 => graph.stats = 0,
            3 => graph.native[0] = 0,
            4 => graph.native[1] = 0,
            5 => graph.native[2] = 0,
            6 => graph.stats_class = SHADOW_CHARACTER,
            7 => graph.stats_actor = 0,
            _ => unreachable!(),
        });
        assert!(matches!(
            write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, 2.5),
            Err(WriteError::Offline | WriteError::NotReady(_) | WriteError::Stale(_) | WriteError::Refused(_))
        ));
    }
    for drift in [
        Drift::Online,
        Drift::Manager,
        Drift::ReplacedManager,
        Drift::ManagerNative,
        Drift::DeadManager,
        Drift::Actor,
        Drift::Stats,
        Drift::ManagerActor,
        Drift::ActorManager,
        Drift::StatsActor,
        Drift::Native,
        Drift::Dead,
    ] {
        GRAPH.set(Graph::default());
        DRIFT.set(drift);
        assert!(matches!(
            write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, 2.5),
            Err(WriteError::Offline | WriteError::Stale(_) | WriteError::NotReady(_))
        ));
    }
    assert_eq!(STORES.get(), 0);
}

#[test]
fn every_postattempt_fault_is_failed_uncertain_with_one_store_and_no_rollback() {
    for mode in [
        StoreMode::Ignore,
        StoreMode::OtherBits,
        StoreMode::Online,
        StoreMode::MissingManager,
        StoreMode::ReplacedManager,
        StoreMode::ManagerNative,
        StoreMode::DeadManager,
        StoreMode::ActorEdge,
        StoreMode::StatsEdge,
        StoreMode::DeadActor,
        StoreMode::DeadStats,
        StoreMode::LostRootTarget,
        StoreMode::InvalidBoolean,
    ] {
        let (api, thread) = fixture();
        let expected = current_observation(&api, &thread);
        STORE_MODE.set(mode);
        assert!(
            matches!(write_rate(&api, &thread, &expected, AttackLeechTarget::AttackRate, 2.5), Err(WriteError::Failed(_))),
            "{mode:?}"
        );
        assert_eq!(STORES.get(), 1);
        assert_eq!(LAST_STORE.get(), (ATTACK, 2.5_f32.to_bits()));
        GRAPH.with_borrow(|graph| {
            assert_eq!(
                graph.attack,
                if matches!(mode, StoreMode::Ignore) {
                    expected.attack_rate.raw_bits
                } else {
                    2.5_f32.to_bits()
                }
            )
        });
    }
}

#[test]
fn f32_minimum_signed_zero_nan_repair_and_untouched_nan_bits_are_preserved() {
    let (api, thread) = fixture();
    let before = current_observation(&api, &thread);
    let min = write_rate(&api, &thread, &before, AttackLeechTarget::AttackRate, 0.01_f32).unwrap();
    assert_eq!(min.attack_rate.raw_bits, 0x3c23d70a);
    GRAPH.with_borrow_mut(|graph| {
        graph.attack = 0x7fc12345;
        graph.leech = 0;
    });
    let before = current_observation(&api, &thread);
    let signed = write_rate(&api, &thread, &before, AttackLeechTarget::IncreasedLeechRate, -0.0).unwrap();
    assert_eq!(signed.increased_leech_rate.raw_bits, 0x80000000);
    assert_eq!(signed.attack_rate.raw_bits, 0x7fc12345);
    let repaired = write_rate(&api, &thread, &signed, AttackLeechTarget::AttackRate, 1.5).unwrap();
    assert_eq!(repaired.attack_rate.raw_bits, 1.5_f32.to_bits());
    assert_eq!(repaired.increased_leech_rate.raw_bits, 0x80000000);
    assert_eq!(STORES.get(), 3);
}
