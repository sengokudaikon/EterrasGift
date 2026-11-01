use super::*;
use native_hooks::leaf::LeafGate;
use std::{
    cell::Cell,
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

#[repr(C)]
struct Scene {
    header: [usize; 2],
    length: i32,
}
#[repr(C)]
struct Waypoint {
    header: [usize; 2],
    scene: *const Scene,
    no_waypoint: bool,
    active: bool,
}
thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static FLAGS: Cell<(bool, bool)> = const { Cell::new((false, false)) };
    static WAYPOINT: Cell<usize> = const { Cell::new(0) };
    static THROW: Cell<bool> = const { Cell::new(false) };
}
extern "C-unwind" fn original(
    panel: usize,
    scene: usize,
    presence: bool,
    unlocked: bool,
    waypoint: *const Waypoint,
    inactive: bool,
    override_level: bool,
    level: i32,
    method: usize,
) {
    CALLS.set(CALLS.get() + 1);
    assert_eq!((panel, scene, waypoint as usize), (0x1234, 0x5678, WAYPOINT.get()));
    assert_eq!((inactive, override_level, level, method), (true, false, -1234567, 0xabcdef01));
    FLAGS.set((presence, unlocked));
    assert!(!THROW.get(), "original threw");
}

#[test]
fn selection_preserves_all_nine_positions_and_only_changes_eligible_availability_flags() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    let layout = Layout {
        online: NonNull::from(&*online).cast(),
        scene: offset_of!(Waypoint, scene) as u32,
        no_waypoint: offset_of!(Waypoint, no_waypoint) as u32,
        active: offset_of!(Waypoint, active) as u32,
    };
    // SAFETY: exact authored ABI and retained online storage; each nonnull waypoint/string stays live through its call.
    let gate = unsafe { LeafGate::prepare_override(layout.effect(), None, None).unwrap() };
    // SAFETY: matching original configured before any call enters the generated leaf.
    unsafe { gate.set_original(original as *const ()) };
    gate.set_enabled(true);
    type Call = extern "C-unwind" fn(usize, usize, bool, bool, *const Waypoint, bool, bool, i32, usize);
    // SAFETY: the leaf tail-forwards all nine original positions with no stack adjustment.
    let call: Call = unsafe { std::mem::transmute(gate.entry()) };
    let invoke = |waypoint: *const Waypoint, input, expected| {
        WAYPOINT.set(waypoint as usize);
        let before = CALLS.get();
        call(0x1234, 0x5678, input, false, waypoint, true, false, -1234567, 0xabcdef01);
        assert_eq!(CALLS.get(), before + 1);
        assert_eq!(FLAGS.get(), expected);
    };
    let seven = Scene { header: [0; 2], length: 7 };
    let other = Scene { header: [0; 2], length: 8 };
    let mut waypoint = Waypoint {
        header: [0; 2],
        scene: &seven,
        no_waypoint: false,
        active: false,
    };
    invoke(&waypoint, false, (true, true));
    waypoint.no_waypoint = true;
    invoke(&waypoint, false, (false, false));
    invoke(&waypoint, true, (true, false));
    waypoint.scene = &other;
    invoke(&waypoint, false, (true, true));
    waypoint.scene = std::ptr::null();
    invoke(&waypoint, false, (true, true));
    waypoint.active = true;
    invoke(&waypoint, true, (true, false));
    invoke(std::ptr::null(), false, (false, false));
    waypoint.active = false;
    online.store(1, SeqCst);
    invoke(&waypoint, false, (false, false));
    online.store(0, SeqCst);
    gate.set_enabled(false);
    invoke(&waypoint, true, (true, false));
    gate.set_enabled(true);
    invoke(&waypoint, false, (true, true));
    assert!(
        waypoint.scene.is_null() && waypoint.no_waypoint && !waypoint.active,
        "the object is never unlocked or mutated"
    );
    THROW.set(true);
    let before = CALLS.get();
    assert!(std::panic::catch_unwind(|| invoke(&waypoint, false, (true, true))).is_err());
    THROW.set(false);
    assert_eq!(CALLS.get(), before + 1, "an original exception is never retried");
}

#[test]
fn travel_recipe_selects_both_rejections_and_leaves_the_current_location_check_intact() {
    use native_hooks::patch::find_unique;
    let code: Vec<u8> = TRAVEL_PATTERN.iter().map(|byte| if *byte < 0 { 0x12 } else { *byte as u8 }).collect();
    assert_eq!(find_unique(&code, TRAVEL_PATTERN, 4, &[0x90; 6]).unwrap(), 4);
    assert_eq!(find_unique(&code, TRAVEL_PATTERN, 14, &[0x90; 6]).unwrap(), 14);
    let mut patched = code.clone();
    patched[4..10].fill(0x90);
    patched[14..20].fill(0x90);
    assert_eq!(patched[20..], code[20..], "player-is-here and the travel body stay unchanged");
    let mut changed = code.clone();
    changed[14] = 0x75;
    assert!(find_unique(&changed, TRAVEL_PATTERN, 4, &[0x90; 6]).is_err());
    assert!(find_unique(&code.repeat(2), TRAVEL_PATTERN, 4, &[0x90; 6]).is_err());
}
