//! Full reader through independently typed in-memory exports; no native runtime is loaded.
use eg_game_access::il2cpp::{
    ThreadContext, raw,
    runtime_thread::{RuntimeInitializationThreadCandidate, RuntimeThreadApi, RuntimeThreadReadError},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::{CStr, c_char, c_void},
    ptr::{NonNull, null_mut},
};

const MAIN: usize = 0x1000;
const INTERNAL: usize = 0x2000;
const CURRENT: usize = 0x3000;
const OBJECT: usize = 1;
const CRITICAL: usize = 2;
const VALUE: usize = 3;
const THREAD: usize = 4;
const INTERNAL_CLASS: usize = 5;
const I64: usize = 6;
const I32: usize = 7;

fn pointer<T>(value: usize) -> *mut T {
    std::ptr::without_provenance_mut(value)
}
fn class(id: usize) -> *mut raw::Class {
    pointer(id)
}

#[derive(Clone, Copy)]
struct ClassDef {
    parent: usize,
    value: bool,
    flags: i32,
    generic: bool,
    inflated: bool,
    is_enum: bool,
    image: usize,
    ready: bool,
}
#[derive(Clone, Copy)]
struct FieldDef {
    owner: usize,
    ty: usize,
    code: i32,
    flags: i32,
    byref: bool,
    offset: usize,
    missing: bool,
    null_type: bool,
}
struct Fake {
    classes: [ClassDef; 8],
    fields: [FieldDef; 4],
    storage: [[u8; 256]; 2],
    null_main: bool,
    null_child: bool,
    null_core: bool,
    null_class: usize,
    object_classes: [usize; 2],
    handles: BTreeMap<usize, usize>,
    pending_unrooted: Option<usize>,
    root_fail: usize,
    get_fail: usize,
    root_attempts: usize,
    root_gets: usize,
    created: usize,
    freed: usize,
    current_calls: usize,
    field_reads: Vec<usize>,
    events: Vec<&'static str>,
}
impl Fake {
    fn new(padded: bool, windows: i64, managed: i32) -> Self {
        let mut classes = [ClassDef {
            parent: OBJECT,
            value: false,
            flags: 0,
            generic: false,
            inflated: false,
            is_enum: false,
            image: 1,
            ready: true,
        }; 8];
        classes[OBJECT].parent = 0;
        for id in [THREAD, INTERNAL_CLASS] {
            classes[id].parent = CRITICAL;
            classes[id].flags = 0x108;
        }
        for id in [I64, I32] {
            classes[id].parent = VALUE;
            classes[id].value = true;
            classes[id].flags = 0x108;
        }
        let offsets = if padded { [80, 96, 120] } else { [16, 24, 40] };
        let fields = [
            FieldDef {
                owner: THREAD,
                ty: INTERNAL_CLASS,
                code: 0x12,
                flags: 1,
                byref: false,
                offset: offsets[0],
                missing: false,
                null_type: false,
            },
            FieldDef {
                owner: INTERNAL_CLASS,
                ty: I64,
                code: 0x0a,
                flags: 3,
                byref: false,
                offset: offsets[1],
                missing: false,
                null_type: false,
            },
            FieldDef {
                owner: INTERNAL_CLASS,
                ty: I32,
                code: 0x08,
                flags: 3,
                byref: false,
                offset: offsets[2],
                missing: false,
                null_type: false,
            },
            FieldDef {
                owner: INTERNAL_CLASS,
                ty: I64,
                code: 0x0a,
                flags: 1,
                byref: false,
                offset: 8,
                missing: false,
                null_type: false,
            },
        ];
        let mut storage = [[0xcd; 256]; 2];
        storage[0][offsets[0]..offsets[0] + size_of::<usize>()].copy_from_slice(&INTERNAL.to_ne_bytes());
        storage[1][offsets[1]..offsets[1] + 8].copy_from_slice(&windows.to_ne_bytes());
        storage[1][offsets[2]..offsets[2] + 4].copy_from_slice(&managed.to_ne_bytes());
        Self {
            classes,
            fields,
            storage,
            null_main: false,
            null_child: false,
            null_core: false,
            null_class: 0,
            object_classes: [THREAD, INTERNAL_CLASS],
            handles: BTreeMap::new(),
            pending_unrooted: None,
            root_fail: 0,
            get_fail: 0,
            root_attempts: 0,
            root_gets: 0,
            created: 0,
            freed: 0,
            current_calls: 0,
            field_reads: Vec::new(),
            events: Vec::new(),
        }
    }
    fn metadata(&mut self) {
        assert!(self.pending_unrooted.is_none(), "a returned object must be rooted before another export");
        assert!(self.handles.values().any(|id| *id == MAIN), "main root spans all metadata calls");
        self.events.push("metadata");
    }
    fn rooted(&mut self, object: usize) {
        self.metadata();
        assert!(self.handles.values().any(|id| *id == object), "no unrooted object read");
    }
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::new(false, 1234, 9)); }

unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
    FAKE.with_borrow_mut(|f| {
        f.current_calls += 1;
        f.events.push("current");
        pointer(CURRENT)
    })
}
unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
    panic!("reader must not obtain a domain/attach")
}
unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
    panic!("reader must not attach")
}
unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
    panic!("borrowed thread must not detach")
}
unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> *mut c_void {
    FAKE.with_borrow_mut(|f| {
        assert!(!pinned);
        assert_eq!(f.pending_unrooted.take(), Some(object.addr()));
        f.root_attempts += 1;
        f.events.push("root");
        if f.root_fail == f.root_attempts {
            return null_mut();
        }
        let handle = (usize::MAX >> 1) + f.root_attempts;
        assert!(f.handles.insert(handle, object.addr()).is_none());
        f.created += 1;
        pointer(handle)
    })
}
unsafe extern "C-unwind" fn free(handle: *mut c_void) {
    FAKE.with_borrow_mut(|f| {
        assert!(f.handles.remove(&handle.addr()).is_some());
        f.freed += 1;
        f.events.push("free");
    });
}
unsafe extern "C-unwind" fn target(handle: *mut c_void) -> *mut raw::Object {
    FAKE.with_borrow_mut(|f| {
        assert!(f.pending_unrooted.is_none());
        f.root_gets += 1;
        let object = *f.handles.get(&handle.addr()).expect("exact pointer-valued live handle");
        if f.get_fail == f.root_gets { null_mut() } else { pointer(object) }
    })
}
unsafe extern "C-unwind" fn main_thread() -> *mut raw::Object {
    FAKE.with_borrow_mut(|f| {
        assert!(f.handles.is_empty());
        f.events.push("main");
        if f.null_main {
            null_mut()
        } else {
            f.pending_unrooted = Some(MAIN);
            pointer(MAIN)
        }
    })
}
unsafe extern "C-unwind" fn corlib() -> *const c_void {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        pointer(if f.null_core { 0 } else { 1 })
    })
}
unsafe extern "C-unwind" fn from_name(image: *const c_void, ns: *const c_char, name: *const c_char) -> *mut raw::Class {
    // SAFETY: reader passes static NUL-terminated namespace/type names with this exact ABI.
    let names = unsafe { (CStr::from_ptr(ns).to_bytes(), CStr::from_ptr(name).to_bytes()) };
    let id = match names {
        (b"System", b"Object") => OBJECT,
        (b"System.Runtime.ConstrainedExecution", b"CriticalFinalizerObject") => CRITICAL,
        (b"System", b"ValueType") => VALUE,
        (b"System.Threading", b"Thread") => THREAD,
        (b"System.Threading", b"InternalThread") => INTERNAL_CLASS,
        (b"System", b"Int64") => I64,
        (b"System", b"Int32") => I32,
        _ => panic!("unexpected class name"),
    };
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        assert_eq!(image.addr(), 1);
        class(if f.null_class == id { 0 } else { id })
    })
}
macro_rules! class_export {
    ($name:ident, $result:ty, $body:expr) => {
        unsafe extern "C-unwind" fn $name(value: *const raw::Class) -> $result {
            FAKE.with_borrow_mut(|f| {
                f.metadata();
                let definition = f.classes[value.addr()];
                ($body)(definition)
            })
        }
    };
}
class_export!(is_value, bool, |d: ClassDef| d.value);
class_export!(is_enum, bool, |d: ClassDef| d.is_enum);
class_export!(generic, bool, |d: ClassDef| d.generic);
class_export!(inflated, bool, |d: ClassDef| d.inflated);
unsafe extern "C-unwind" fn class_image(value: *mut raw::Class) -> *const c_void {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        pointer(f.classes[value.addr()].image)
    })
}
unsafe extern "C-unwind" fn parent(value: *mut raw::Class) -> *mut raw::Class {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        class(f.classes[value.addr()].parent)
    })
}
unsafe extern "C-unwind" fn flags(value: *mut raw::Class) -> i32 {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        f.classes[value.addr()].flags
    })
}
unsafe extern "C-unwind" fn inited(value: *mut raw::Class) -> bool {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        f.classes[value.addr()].ready
    })
}
unsafe extern "C-unwind" fn object_class(value: *mut raw::Object) -> *mut raw::Class {
    FAKE.with_borrow_mut(|f| {
        f.rooted(value.addr());
        class(
            f.object_classes[if value.addr() == MAIN {
                0
            } else {
                assert_eq!(value.addr(), INTERNAL);
                1
            }],
        )
    })
}
unsafe extern "C-unwind" fn field(owner: *mut raw::Class, name: *const c_char) -> *mut raw::FieldInfo {
    // SAFETY: caller passes one static, valid NUL-terminated field name.
    let index = match unsafe { CStr::from_ptr(name).to_bytes() } {
        b"internal_thread" => 0,
        b"thread_id" => 1,
        b"managed_id" => 2,
        b"unrelated" => 3,
        _ => panic!("unexpected field name"),
    };
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        assert_eq!(owner.addr(), if index == 0 { THREAD } else { INTERNAL_CLASS });
        if f.fields[index].missing { null_mut() } else { pointer(index + 1) }
    })
}
macro_rules! field_export {
    ($name:ident, $result:ty, $body:expr) => {
        unsafe extern "C-unwind" fn $name(value: *mut raw::FieldInfo) -> $result {
            FAKE.with_borrow_mut(|f| {
                f.metadata();
                let definition = f.fields[value.addr() - 1];
                ($body)(definition)
            })
        }
    };
}
field_export!(field_parent, *mut raw::Class, |d: FieldDef| class(d.owner));
field_export!(field_flags, i32, |d: FieldDef| d.flags);
field_export!(field_offset, usize, |d: FieldDef| d.offset);
unsafe extern "C-unwind" fn field_type(value: *mut raw::FieldInfo) -> *const raw::Type {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        pointer(if f.fields[value.addr() - 1].null_type { 0 } else { value.addr() })
    })
}
unsafe extern "C-unwind" fn type_code(value: *const raw::Type) -> i32 {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        f.fields[value.addr() - 1].code
    })
}
unsafe extern "C-unwind" fn type_byref(value: *const raw::Type) -> bool {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        f.fields[value.addr() - 1].byref
    })
}
unsafe extern "C-unwind" fn type_class(value: *const raw::Type) -> *mut raw::Class {
    FAKE.with_borrow_mut(|f| {
        f.metadata();
        class(f.fields[value.addr() - 1].ty)
    })
}
unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, field: *mut raw::FieldInfo, out: *mut c_void) {
    FAKE.with_borrow_mut(|f| {
        f.rooted(object.addr());
        let index = field.addr() - 1;
        assert!(index < 3, "unrelated field must never be read");
        assert_eq!(object.addr(), if index == 0 { MAIN } else { INTERNAL });
        let owner = if index == 0 { 0 } else { 1 };
        let width = if index == 0 {
            size_of::<usize>()
        } else if index == 1 {
            8
        } else {
            4
        };
        let offset = f.fields[index].offset;
        let bytes = &f.storage[owner][offset..offset + width];
        // SAFETY: the actual reader's exact named destination is pointer/Int64/Int32 respectively;
        // this in-memory fake has valid initialized source bytes and matching output width.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), width) };
        f.field_reads.push(index);
        if index == 0 {
            if f.null_child {
                // SAFETY: this exact reference field has a pointer-sized initialized output slot.
                unsafe { out.cast::<*mut raw::Object>().write(null_mut()) };
            } else {
                f.pending_unrooted = Some(INTERNAL);
            }
        }
    });
}

fn symbol(name: &CStr) -> Option<NonNull<c_void>> {
    let address = match name.to_bytes() {
        b"il2cpp_domain_get" => domain as *const (),
        b"il2cpp_thread_current" => current as *const (),
        b"il2cpp_thread_attach" => attach as *const (),
        b"il2cpp_thread_detach" => detach as *const (),
        b"il2cpp_gchandle_new" => root as *const (),
        b"il2cpp_gchandle_free" => free as *const (),
        b"il2cpp_gchandle_get_target" => target as *const (),
        b"mono_thread_get_main" => main_thread as *const (),
        b"il2cpp_get_corlib" => corlib as *const (),
        b"il2cpp_class_from_name" => from_name as *const (),
        b"il2cpp_class_get_image" => class_image as *const (),
        b"il2cpp_class_get_parent" => parent as *const (),
        b"il2cpp_class_get_flags" => flags as *const (),
        b"il2cpp_class_is_valuetype" => is_value as *const (),
        b"il2cpp_class_is_enum" => is_enum as *const (),
        b"il2cpp_class_is_generic" => generic as *const (),
        b"il2cpp_class_is_inflated" => inflated as *const (),
        b"il2cpp_class_is_inited" => inited as *const (),
        b"il2cpp_object_get_class" => object_class as *const (),
        b"il2cpp_class_get_field_from_name" => field as *const (),
        b"il2cpp_field_get_parent" => field_parent as *const (),
        b"il2cpp_field_get_flags" => field_flags as *const (),
        b"il2cpp_field_get_type" => field_type as *const (),
        b"il2cpp_field_get_offset" => field_offset as *const (),
        b"il2cpp_type_get_type" => type_code as *const (),
        b"il2cpp_type_is_byref" => type_byref as *const (),
        b"il2cpp_class_from_type" => type_class as *const (),
        b"il2cpp_field_get_value" => field_get as *const (),
        _ => return None,
    };
    NonNull::new(address.cast_mut().cast())
}

struct Counts {
    created: usize,
    freed: usize,
    reads: Vec<usize>,
    gets: usize,
}
fn run(fake: Fake) -> (Result<RuntimeInitializationThreadCandidate, RuntimeThreadReadError>, Counts) {
    FAKE.with_borrow_mut(|f| *f = fake);
    // SAFETY: each resolver entry has exactly its named ABI and static lifetime; no target is loaded.
    let ownership = unsafe { raw::Api::resolve(symbol) }.unwrap();
    // SAFETY: this test owns one thread-local, nonmoving, initialized/published fake with borrowed attachment.
    let context = unsafe { ThreadContext::new(ownership) };
    let attached = context.attach().unwrap();
    // SAFETY: same fake runtime, exact individually typed exports; resolution invokes none.
    let api = unsafe { RuntimeThreadApi::resolve(symbol) }.unwrap();
    // SAFETY: same-runtime borrowed lease; local fake excludes mutation/shutdown/reentrancy. It
    // models metadata failures only, never malformed native code or foreign exceptions.
    let result = unsafe { api.read_candidate(&attached) };
    let counts = FAKE.with_borrow(|f| {
        assert!(f.handles.is_empty());
        assert_eq!(f.created, f.freed);
        assert_eq!(f.current_calls, 1, "reader never reattaches or changes the borrowed thread");
        Counts {
            created: f.created,
            freed: f.freed,
            reads: f.field_reads.clone(),
            gets: f.root_gets,
        }
    });
    drop(attached);
    FAKE.with_borrow(|f| assert_eq!(f.current_calls, 1));
    (result, counts)
}

#[test]
fn resolves_without_calls_and_missing_export_keeps_exact_name() {
    FAKE.with_borrow_mut(|f| *f = Fake::new(false, 1, 1));
    // SAFETY: individually typed fake exports, with this one name deliberately absent.
    let result = unsafe { RuntimeThreadApi::resolve(|name| if name == c"il2cpp_field_get_value" { None } else { symbol(name) }) };
    assert_eq!(result.err().unwrap(), raw::MissingExport(c"il2cpp_field_get_value"));
    FAKE.with_borrow(|f| assert!(f.events.is_empty()));
}

#[test]
fn exact_fields_and_independently_padded_layouts_return_separate_scalar_domains() {
    for (padded, extra) in [(false, 0), (true, usize::MAX)] {
        let mut fake = Fake::new(padded, u32::MAX as i64, i32::MAX);
        fake.fields[3].offset = extra;
        let (result, counts) = run(fake);
        let candidate = result.unwrap();
        assert_eq!(candidate.windows_thread_id().get(), u32::MAX);
        assert_eq!(candidate.managed_thread_id().get(), i32::MAX);
        assert_eq!((counts.created, counts.freed), (2, 2));
        assert_eq!(counts.reads, [0, 1, 2]);
    }
}

#[test]
fn every_exact_field_owner_type_and_storage_failure_precedes_any_value_read() {
    for index in 0..3 {
        for fault in 0..12 {
            let mut fake = Fake::new(false, 1234, 9);
            let field = &mut fake.fields[index];
            match fault {
                0 => field.owner = OBJECT,
                1 => field.ty = OBJECT,
                2 => field.code = 0x0e,
                3 => field.byref = true,
                4 => field.flags |= 0x10,
                5 => field.flags |= 0x20,
                6 => field.flags |= 0x40,
                7 => field.offset = usize::MAX,
                8 => field.offset = usize::MAX - 1,
                9 => field.missing = true,
                10 => field.null_type = true,
                11 => field.flags ^= 1,
                _ => unreachable!(),
            }
            let (result, counts) = run(fake);
            assert!(matches!(result, Err(RuntimeThreadReadError::Contract(_))), "field={index} fault={fault}: {result:?}");
            assert!(counts.reads.is_empty());
            assert_eq!((counts.created, counts.freed), (1, 1));
        }
    }
}

#[test]
fn exact_core_classes_parents_and_closed_layout_shapes_are_required() {
    for id in 1..=7 {
        for fault in 0..7 {
            let mut fake = Fake::new(false, 1234, 9);
            match fault {
                0 => fake.classes[id].image = 2,
                1 => fake.classes[id].generic = true,
                2 => fake.classes[id].inflated = true,
                3 => fake.classes[id].value = !fake.classes[id].value,
                4 => fake.classes[id].is_enum = true,
                5 => fake.classes[id].parent = I32,
                6 => fake.null_class = id,
                _ => unreachable!(),
            }
            let (result, counts) = run(fake);
            assert!(matches!(result, Err(RuntimeThreadReadError::Contract(_))), "class={id} fault={fault}: {result:?}");
            assert!(counts.reads.is_empty());
        }
    }
    for id in [THREAD, INTERNAL_CLASS, I64, I32] {
        for shape in [0, 0x100, 0x110, 0x128, 0x188] {
            let mut fake = Fake::new(false, 1234, 9);
            fake.classes[id].flags = shape;
            let (result, counts) = run(fake);
            assert!(matches!(result, Err(RuntimeThreadReadError::Contract(_))));
            assert!(counts.reads.is_empty());
        }
    }
    for id in [THREAD, INTERNAL_CLASS] {
        let mut fake = Fake::new(false, 1234, 9);
        fake.classes[id].ready = false;
        let (result, counts) = run(fake);
        assert!(matches!(result, Err(RuntimeThreadReadError::Contract(_))));
        assert!(counts.reads.is_empty());
    }
}

#[test]
fn exact_rooted_object_classes_are_checked_before_their_fields_are_read() {
    for index in 0..2 {
        let mut fake = Fake::new(false, 1234, 9);
        fake.object_classes[index] = OBJECT;
        let (result, counts) = run(fake);
        assert!(matches!(result, Err(RuntimeThreadReadError::Contract(_))));
        assert_eq!(counts.reads, if index == 0 { vec![] } else { vec![0] });
        assert_eq!(counts.created, index + 1);
    }
}

#[test]
fn null_graph_root_creation_and_every_target_failure_release_all_acquired_handles() {
    for fault in 0..10 {
        let mut fake = Fake::new(false, 1234, 9);
        match fault {
            0 => fake.null_main = true,
            1 => fake.null_child = true,
            2 => fake.null_core = true,
            3 => fake.root_fail = 1,
            4 => fake.root_fail = 2,
            5..=9 => fake.get_fail = fault - 4,
            _ => unreachable!(),
        }
        let (result, counts) = run(fake);
        assert!(result.is_err(), "fault={fault}");
        if fault >= 5 {
            assert_eq!(result.unwrap_err(), RuntimeThreadReadError::RootTargetUnavailable);
            assert_eq!(counts.gets, fault - 4);
        }
        assert_eq!(counts.created, counts.freed);
    }
}

#[test]
fn windows_dword_bounds_and_managed_positive_int32_are_distinct() {
    for windows in [i64::MIN, -1, 0, u32::MAX as i64 + 1, i64::MAX] {
        let (result, counts) = run(Fake::new(false, windows, 9));
        assert_eq!(result.unwrap_err(), RuntimeThreadReadError::WindowsIdOutOfRange(windows));
        assert_eq!((counts.created, counts.freed), (2, 2));
    }
    for managed in [i32::MIN, -1, 0] {
        let (result, counts) = run(Fake::new(false, 1234, managed));
        assert_eq!(result.unwrap_err(), RuntimeThreadReadError::ManagedIdOutOfRange(managed));
        assert_eq!((counts.created, counts.freed), (2, 2));
    }
    let candidate = run(Fake::new(true, 1, 1)).0.unwrap();
    assert_eq!((candidate.windows_thread_id().get(), candidate.managed_thread_id().get()), (1, 1));
}
