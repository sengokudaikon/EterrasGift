use super::*;
use native_hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

#[repr(C)]
struct Difficulty {
    header: [usize; 2],
    maximum: i32,
}
#[repr(C)]
struct Array {
    header: [usize; 3],
    length: usize,
    data: [*const Difficulty; 2],
}
#[repr(C)]
struct List {
    header: [usize; 2],
    items: *const Array,
    size: i32,
}
#[repr(C)]
struct Timeline {
    header: [usize; 2],
    difficulties: *const List,
}
#[repr(C)]
pub(crate) struct Run {
    header: [usize; 2],
    pub quests: i32,
    pub stability: i32,
    index: i32,
    timeline: *const Timeline,
    pub calls: usize,
}
impl Run {
    pub(crate) fn new() -> Self {
        Self {
            header: [0; 2],
            quests: 0,
            stability: 0,
            index: 0,
            timeline: std::ptr::null(),
            calls: 0,
        }
    }
}
thread_local! {
    static SEEN: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    static THROW: Cell<bool> = const { Cell::new(false) };
}
pub(crate) type Call = unsafe extern "C-unwind" fn(usize, *mut Run, bool, bool, i32, usize);
pub(crate) unsafe extern "C-unwind" fn original(panel: usize, run: *mut Run, dont_move: bool, woven: bool, placed: i32, method: usize) {
    assert_eq!((panel, dont_move, woven, placed, method), (0x1234, true, false, -1234567, 0xabcdef01));
    // SAFETY: the fixture retains every nonnull run until the call/unwind finishes.
    unsafe {
        if !run.is_null() {
            (*run).calls += 1;
            SEEN.set(((*run).quests, (*run).stability));
        }
    }
    assert!(!THROW.get(), "original threw");
}
pub(crate) unsafe fn invoke(entry: *const (), run: *mut Run) {
    // SAFETY: exact six-position void ABI including both original stack arguments.
    unsafe { std::mem::transmute::<*const (), Call>(entry)(0x1234, run, true, false, -1234567, 0xabcdef01) }
}
pub(crate) fn layout(online: &AtomicU8) -> Layout {
    Layout {
        online: NonNull::from(online).cast(),
        quests: offset_of!(Run, quests) as u32,
        stability: offset_of!(Run, stability) as u32,
        index: offset_of!(Run, index) as u32,
        timeline: offset_of!(Run, timeline) as u32,
        difficulties: offset_of!(Timeline, difficulties) as u32,
        items: offset_of!(List, items) as u32,
        size: offset_of!(List, size) as u32,
        maximum: offset_of!(Difficulty, maximum) as u32,
    }
}
#[test]
fn skip_quest_echoes_raises_before_original_and_checks_both_difficulty_bounds() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    // SAFETY: authored graph/ABI, with the static veto byte retained for process lifetime.
    let gate = unsafe { LeafGate::prepare_override(layout(online).effect(), None, None).unwrap() };
    // SAFETY: exact matching original, installed before publishing settings.
    unsafe { gate.set_original(original as *const ()) };
    gate.set_enabled(true);
    let difficulty = Difficulty { header: [0; 2], maximum: 800 };
    let mut array = Array {
        header: [0; 3],
        length: 2,
        data: [std::ptr::null(), &difficulty],
    };
    let mut list = List {
        header: [0; 2],
        items: &array,
        size: 2,
    };
    let mut timeline = Timeline {
        header: [0; 2],
        difficulties: &list,
    };
    let mut run = Run {
        index: 1,
        timeline: &timeline,
        ..Run::new()
    };
    let call = |run: &mut Run| {
        let calls = run.calls;
        // SAFETY: all graph objects remain live throughout the synchronous call.
        unsafe { invoke(gate.entry(), run) };
        assert_eq!(run.calls, calls + 1);
        assert_eq!(SEEN.get(), (run.quests, run.stability), "changes precede original");
    };
    call(&mut run);
    assert_eq!((run.quests, run.stability), (2, 800));
    run.quests = 4;
    run.stability = 1200;
    call(&mut run);
    assert_eq!((run.quests, run.stability), (4, 1200));
    for (index, count, capacity) in [(-1, 2, 2), (2, 2, 2), (i32::MAX, 2, 2), (1, -1, 2), (1, 2, 1), (0, 2, 2)] {
        run.quests = 0;
        run.stability = 7;
        run.index = index;
        list.size = count;
        array.length = capacity;
        call(&mut run);
        assert_eq!((run.quests, run.stability), (2, 7), "{index}/{count}/{capacity}");
    }
    run.index = 1;
    list.size = 2;
    array.length = 2;
    for missing in 0..4 {
        run.quests = 0;
        run.stability = 7;
        match missing {
            0 => run.timeline = std::ptr::null(),
            1 => timeline.difficulties = std::ptr::null(),
            2 => list.items = std::ptr::null(),
            _ => array.data[1] = std::ptr::null(),
        }
        call(&mut run);
        assert_eq!((run.quests, run.stability), (2, 7));
        run.timeline = &timeline;
        timeline.difficulties = &list;
        list.items = &array;
        array.data[1] = &difficulty;
    }
    gate.set_enabled(false);
    run.quests = 0;
    run.stability = 7;
    call(&mut run);
    assert_eq!((run.quests, run.stability), (0, 7));
    gate.set_enabled(true);
    online.store(1, SeqCst);
    call(&mut run);
    assert_eq!((run.quests, run.stability), (0, 7));
    online.store(0, SeqCst);
    // SAFETY: original explicitly accepts a null run.
    unsafe { invoke(gate.entry(), std::ptr::null_mut()) };
    THROW.set(true);
    let calls = run.calls;
    let thrown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: leaf tail-jumps; the original's unwind crosses no helper code.
        unsafe { invoke(gate.entry(), &mut run) }
    }));
    assert!(thrown.is_err());
    assert_eq!((run.quests, run.stability, run.calls), (2, 800, calls + 1));
    THROW.set(false);
    gate.set_enabled(false);
    assert_eq!((run.quests, run.stability), (2, 800), "off does not rewind the saved run");
}
