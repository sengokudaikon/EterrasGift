use super::*;
use crate::game::features::{runes_glyphs_to_99::SlotLayout, shards_to_99::ShardLayout};
use native_hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering::SeqCst},
};

#[repr(C)]
struct Shard {
    header: [usize; 2],
    quantity: Cell<i32>,
}
#[repr(C)]
struct Array<const N: usize> {
    header: [usize; 2],
    bounds: usize,
    length: usize,
    items: [*const Shard; N],
}
#[repr(C)]
struct List {
    header: [usize; 2],
    items: *const Array<4>,
    size: i32,
    version: i32,
}
#[repr(C)]
struct Stash {
    header: [usize; 2],
    shards: *const List,
}
#[repr(C)]
struct Tracker {
    header: [usize; 2],
    stash: *const Stash,
}
#[repr(C)]
struct Item {
    header: [usize; 2],
    kind: u8,
}
#[repr(C)]
struct Entry {
    header: [usize; 2],
    data: *const Item,
    quantity: Cell<i32>,
}
#[repr(C)]
struct Slot {
    header: [usize; 2],
    content: *const Entry,
}

#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    throws: AtomicBool,
}
fn entered(probe: *const Probe) {
    // SAFETY: each authored call retains this exact method-position probe until return/unwind.
    let probe = unsafe { &*probe };
    probe.calls.fetch_add(1, SeqCst);
    assert!(!probe.throws.load(SeqCst), "original quantity failed");
}
unsafe extern "C-unwind" fn shard_count(tracker: *const Tracker, probe: *const Probe) -> i32 {
    entered(probe);
    // SAFETY: authored fixture graphs remain live; malformed counts are clamped to the four allocated slots.
    unsafe {
        let Some(tracker) = tracker.as_ref() else { return 0 };
        let Some(stash) = tracker.stash.as_ref() else { return 0 };
        let Some(list) = stash.shards.as_ref() else { return 0 };
        let Some(array) = list.items.as_ref() else { return 0 };
        let count = usize::try_from(list.size).unwrap_or(0).min(array.length).min(4);
        array.items[..count].iter().filter_map(|shard| shard.as_ref()).map(|shard| shard.quantity.get()).sum()
    }
}
unsafe extern "C-unwind" fn slot_quantity(slot: *const Slot, probe: *const Probe) -> i32 {
    entered(probe);
    // SAFETY: live authored slot graph, including deliberately absent references.
    unsafe { slot.as_ref().and_then(|slot| slot.content.as_ref()).map_or(0, |entry| entry.quantity.get()) }
}
type Count<T> = unsafe extern "C-unwind" fn(*const T, *const Probe) -> i32;
fn gate<T>(effect: LeafEffect, original: Count<T>) -> (LeafGate, Count<T>) {
    // SAFETY: generated leaf contract is i32(instance, MethodInfo); fixture graphs/online byte outlive calls.
    unsafe {
        let gate = LeafGate::prepare_override(effect, None, None).unwrap();
        gate.set_original(original as *const ());
        let call = std::mem::transmute::<*const (), Count<T>>(gate.entry());
        (gate, call)
    }
}
fn shard_effect(online: &AtomicU8) -> LeafEffect {
    ShardLayout {
        online: NonNull::from(online).cast(),
        stash: std::mem::offset_of!(Tracker, stash) as u32,
        shards: std::mem::offset_of!(Stash, shards) as u32,
        items: std::mem::offset_of!(List, items) as u32,
        size: std::mem::offset_of!(List, size) as u32,
        quantity: std::mem::offset_of!(Shard, quantity) as u32,
    }
    .effect()
}
fn slot_effect(online: &AtomicU8) -> LeafEffect {
    SlotLayout {
        online: NonNull::from(online).cast(),
        content: std::mem::offset_of!(Slot, content) as u32,
        data: std::mem::offset_of!(Entry, data) as u32,
        item_type: std::mem::offset_of!(Item, kind) as u32,
        quantity: std::mem::offset_of!(Entry, quantity) as u32,
    }
    .effect()
}
fn call<T>(function: Count<T>, object: *const T, probe: &Probe) -> i32 {
    let before = probe.calls.load(SeqCst);
    // SAFETY: all fixture graphs and their method-position probe are alive; the two arguments must survive the leaf.
    let result = unsafe { function(object, probe) };
    assert_eq!(probe.calls.load(SeqCst), before + 1);
    result
}

#[test]
fn shards_raise_stored_active_entries_before_one_original_count_and_never_lower_or_undo_them() {
    let low = Shard {
        header: [0; 2],
        quantity: Cell::new(1),
    };
    let high = Shard {
        header: [0; 2],
        quantity: Cell::new(140),
    };
    let unused = Shard {
        header: [0; 2],
        quantity: Cell::new(7),
    };
    let array = Array {
        header: [0; 2],
        bounds: 0,
        length: 4,
        items: [&low, std::ptr::null(), &high, &unused],
    };
    let list = List {
        header: [0; 2],
        items: &array,
        size: 3,
        version: 123,
    };
    let stash = Stash { header: [0; 2], shards: &list };
    let tracker = Tracker { header: [0; 2], stash: &stash };
    let online = AtomicU8::new(0);
    let probe = Probe::default();
    let (gate, query) = gate(shard_effect(&online), shard_count);
    assert_eq!(call(query, &tracker, &probe), 141);
    gate.set_enabled(true);
    assert_eq!(call(query, &tracker, &probe), 239);
    assert_eq!(low.quantity.get(), 99);
    assert_eq!(high.quantity.get(), 140);
    assert_eq!(unused.quantity.get(), 7);
    assert_eq!(list.version, 123, "only primitive quantities change");
    low.quantity.set(-3);
    online.store(1, SeqCst);
    assert_eq!(call(query, &tracker, &probe), 137);
    assert_eq!(low.quantity.get(), -3);
    online.store(0, SeqCst);
    assert_eq!(call(query, &tracker, &probe), 239);
    gate.set_enabled(false);
    assert_eq!(call(query, &tracker, &probe), 239, "off keeps granted inventory");
    low.quantity.set(2);
    assert_eq!(call(query, &tracker, &probe), 142, "off stops replenishment");
}

#[test]
fn shards_bound_the_list_walk_and_forward_absent_graphs_and_original_failures_without_retry() {
    let shard = Shard {
        header: [0; 2],
        quantity: Cell::new(2),
    };
    let mut array = Array {
        header: [0; 2],
        bounds: 0,
        length: 1,
        items: [&shard, std::ptr::null(), std::ptr::null(), std::ptr::null()],
    };
    let mut list = List {
        header: [0; 2],
        items: &array,
        size: i32::MAX,
        version: 0,
    };
    let mut stash = Stash { header: [0; 2], shards: &list };
    let mut tracker = Tracker { header: [0; 2], stash: &stash };
    let online = AtomicU8::new(0);
    let probe = Probe::default();
    let (gate, query) = gate(shard_effect(&online), shard_count);
    gate.set_enabled(true);
    assert_eq!(call(query, &tracker, &probe), 99, "OG count is min(active size, backing capacity)");
    shard.quantity.set(2);
    list.size = -1;
    assert_eq!(call(query, &tracker, &probe), 0);
    assert_eq!(shard.quantity.get(), 2);
    list.size = 1;
    array.length = usize::try_from(MAX_ENTRIES).unwrap() + 1;
    assert_eq!(call(query, &tracker, &probe), 2, "unsupported capacity forwards without mutation");
    array.length = 0;
    assert_eq!(call(query, &tracker, &probe), 0);
    array.length = 1;
    list.items = std::ptr::null();
    assert_eq!(call(query, &tracker, &probe), 0);
    list.items = &array;
    stash.shards = std::ptr::null();
    assert_eq!(call(query, &tracker, &probe), 0);
    stash.shards = &list;
    tracker.stash = std::ptr::null();
    assert_eq!(call(query, &tracker, &probe), 0);
    assert_eq!(call(query, std::ptr::null(), &probe), 0);
    tracker.stash = &stash;
    let before = probe.calls.load(SeqCst);
    probe.throws.store(true, SeqCst);
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: authored call; the outer test catches its Rust panic, with no catch/retry in the retained leaf.
        unsafe { query(&tracker, &probe) }
    }));
    assert!(failed.is_err());
    assert_eq!(probe.calls.load(SeqCst), before + 1);
    assert_eq!(shard.quantity.get(), 99, "before-original grants persist even when the original fails");
}

#[test]
fn rune_and_glyph_categories_only_are_refilled_and_gates_survive_owner_drop() {
    let mut item = Item { header: [0; 2], kind: 102 };
    let mut entry = Entry {
        header: [0; 2],
        data: &item,
        quantity: Cell::new(1),
    };
    let mut slot = Slot { header: [0; 2], content: &entry };
    let online = AtomicU8::new(0);
    let probe = Probe::default();
    let query = {
        let (gate, query) = gate(slot_effect(&online), slot_quantity);
        assert_eq!(call(query, &slot, &probe), 1);
        gate.set_enabled(true);
        for kind in [0, 101, 102, 103, 104, 255] {
            item.kind = kind;
            entry.quantity.set(1);
            let expected = if matches!(kind, 102 | 103) { 99 } else { 1 };
            assert_eq!(call(query, &slot, &probe), expected);
            assert_eq!(entry.quantity.get(), expected);
        }
        item.kind = 102;
        entry.quantity.set(140);
        assert_eq!(call(query, &slot, &probe), 140);
        entry.quantity.set(-7);
        assert_eq!(call(query, &slot, &probe), 99);
        online.store(1, SeqCst);
        entry.quantity.set(3);
        assert_eq!(call(query, &slot, &probe), 3);
        online.store(0, SeqCst);
        entry.data = std::ptr::null();
        assert_eq!(call(query, &slot, &probe), 3);
        entry.data = &item;
        slot.content = std::ptr::null();
        assert_eq!(call(query, &slot, &probe), 0);
        assert_eq!(call(query, std::ptr::null(), &probe), 0);
        slot.content = &entry;
        assert_eq!(call(query, &slot, &probe), 99);
        gate.set_enabled(false);
        query
    };
    assert_eq!(call(query, &slot, &probe), 99);
    entry.quantity.set(4);
    assert_eq!(call(query, &slot, &probe), 4, "retained closed path does not replenish after owner departure");
}

#[test]
fn material_reference_contract_refuses_wrong_type_byref_static_value_owner_and_unaligned_offsets() {
    use crate::game::{Type, test_fakes::token};
    use std::ffi::c_char;
    thread_local! {
        static KIND: Cell<usize> = const { Cell::new(2) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static VALUE: Cell<bool> = const { Cell::new(false) };
        static OFFSET: Cell<usize> = const { Cell::new(16) };
    }
    unsafe extern "C-unwind" fn field(_: Class, _: *const c_char) -> Ptr {
        token(3)
    }
    unsafe extern "C-unwind" fn ty(_: Ptr) -> Type {
        token(4)
    }
    unsafe extern "C-unwind" fn class(_: Type) -> Class {
        token(KIND.get())
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn value(_: *const raw::Class) -> bool {
        VALUE.get()
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        OFFSET.get()
    }
    let mut access = RuntimeAccess::test_stubs();
    access.class_field = field;
    access.field_type = ty;
    access.type_class = class;
    access.type_byref = byref;
    access.field_flags = flags;
    access.class_value = value;
    access.field_offset = offset;
    let resolve = || {
        // SAFETY: immutable authored class/field/type tokens served by the matching fake metadata exports.
        unsafe { access.hook_field_of(token(1), c"content", token(2), 8) }
    };
    assert_eq!(resolve().unwrap(), 16);
    KIND.set(5);
    assert!(resolve().is_err());
    KIND.set(2);
    BYREF.set(true);
    assert!(resolve().is_err());
    BYREF.set(false);
    FLAGS.set(raw::FIELD_ATTRIBUTE_STATIC);
    assert!(resolve().is_err());
    FLAGS.set(0);
    VALUE.set(true);
    assert!(resolve().is_err());
    VALUE.set(false);
    for offset in [0, 8, 17, 16392] {
        OFFSET.set(offset);
        assert!(resolve().is_err());
    }
}
