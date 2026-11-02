use super::*;
use crate::game::{
    morditas_timer::{
        STAGES,
        tests::{self, Hunt},
    },
    registry::ResolvedMethod,
};
use std::sync::atomic::AtomicU8;

#[unsafe(naked)]
unsafe extern "C-unwind" fn start(_: *mut Hunt, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym tests::start);
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn tick(_: *mut Hunt, _: f32, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym tests::tick);
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn kill(_: *mut Hunt, _: usize, _: bool, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym tests::kill);
}
fn check(values: [f32; 3]) {
    let mut hunt = Hunt::default();
    for (stage, entry) in STAGES.into_iter().zip([start as *const (), tick as *const (), kill as *const ()]) {
        // SAFETY: exact authored ABI over a retained hunt; black_box prevents folding across target mutations.
        unsafe { tests::invoke(stage, std::hint::black_box(entry), &mut hunt, 4.0) };
    }
    assert_eq!((hunt.length, hunt.elapsed, hunt.calls), (100.0 * values[0] + 3.0 * values[2], 4.0 * values[1], [1; 3]));
}

#[test]
fn morditas_timer_owns_all_three_entries_settings_readback_manager_loss_and_unload() {
    let online = AtomicU8::new(0);
    let hook = GameplayHook::MorditasTimer;
    // SAFETY: padded originals, exact per-stage ABIs and matching receiver/static layouts. No entry is yet published.
    let group = unsafe {
        let targets: Vec<_> = STAGES
            .into_iter()
            .zip([start as *mut u8, tick as *mut u8, kill as *mut u8])
            .map(|(stage, address)| {
                ResolvedMethod {
                    name: format!("fixture hunt {stage:?}"),
                    address: NonNull::new(address).unwrap(),
                    bytes: address.cast::<[u8; 16]>().read_unaligned(),
                }
                .postfix(tests::layout(&online).effect(stage))
            })
            .collect();
        Group::prepare(FeatureBinding::fixture(hook), &targets).unwrap()
    };
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    owner.slots[slot_of(hook)] = Some(group);
    let settings = [2.0, 0.5, 3.0];
    let value = tests::value(settings);
    for on in [true, false, true] {
        let group = owner.slots[slot_of(hook)].as_ref().unwrap();
        change_group(group, hook, on, value).unwrap();
        let observed = observation(Some(group), hook);
        assert!(observed.confirmed(on));
        if on {
            assert_eq!(observed.value, value);
        }
        assert!(group.hooks.iter().all(|entry| entry.state() == if on { HookState::Enabled } else { HookState::Disabled }));
        check(if on { settings.map(|n| n as f32) } else { [1.0; 3] });
    }
    let group = owner.slots[slot_of(hook)].as_ref().unwrap();
    change_group(group, hook, true, tests::value([10.0, 0.0, 10.0])).unwrap();
    assert!(observation(Some(group), hook).confirmed(true));
    check([10.0, 0.0, 10.0]);
    for manager in [2, 0] {
        owner.observe_manager(manager).unwrap();
        assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
        check([1.0; 3]);
        change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, value).unwrap();
    }
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots.iter().all(Option::is_none));
    check([1.0; 3]);
}
