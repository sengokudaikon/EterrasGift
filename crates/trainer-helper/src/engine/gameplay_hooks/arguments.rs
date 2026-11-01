use super::*;
use crate::game::{registry::ResolvedMethod, reveal_objective};
use std::{mem::offset_of, sync::atomic::AtomicU8};

#[repr(C)]
struct Pulse {
    threshold: f32,
    received: f32,
}
extern "C-unwind" fn original(pulse: &mut Pulse, score: f32, last_kill: usize, method: usize) {
    assert_eq!((last_kill, method), (0x1234, 0xabcd));
    pulse.received = score;
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn target(_: &mut Pulse, _: f32, _: usize, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym original);
}
#[test]
fn argument_adjustment_uses_shared_readback_restore_manager_loss_and_terminal_clear() {
    let online = AtomicU8::new(0);
    // SAFETY: padded native four-position void method and retained authored pulse/online layouts.
    let group = unsafe {
        let method = ResolvedMethod {
            name: "fixture pulse".into(),
            address: NonNull::new(target as *mut u8).unwrap(),
            bytes: (target as *const [u8; 16]).read_unaligned(),
        };
        Group::prepare(
            FeatureBinding::fixture(GameplayHook::RevealObjective),
            &[method.leaf(reveal_objective::effect(NonNull::from(&online).cast(), offset_of!(Pulse, threshold) as u32), None, None)],
        )
        .unwrap()
    };
    let hook = GameplayHook::RevealObjective;
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    owner.slots[slot_of(hook)] = Some(group);
    let mut pulse = Pulse { threshold: 60.0, received: 0.0 };
    let mut check = |expected| {
        // SAFETY: exact authored mixed-argument signature, black_box avoids folding calls across entry mutations.
        unsafe {
            std::hint::black_box(target)(&mut pulse, 1.0, 0x1234, 0xabcd);
        }
        assert_eq!(pulse.received, expected);
    };
    for on in [true, false, true] {
        let group = owner.slots[slot_of(hook)].as_ref().unwrap();
        change_group(group, hook, on, 1).unwrap();
        assert!(observation(Some(group), hook).confirmed(on));
        check(if on { 60.0 } else { 1.0 });
    }
    owner.observe_manager(2).unwrap();
    assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
    check(1.0);
    change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, 1).unwrap();
    check(60.0);
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots.iter().all(Option::is_none));
    check(1.0);
}
