use super::*;
use crate::game::{prophecy_uses, registry::ResolvedMethod, skip_quest_echoes};
use std::sync::atomic::AtomicU8;

#[unsafe(naked)]
unsafe extern "C-unwind" fn prophecy_target(_: *mut prophecy_uses::tests::Slot, _: i32, _: bool, _: usize, _: *mut i32, _: usize) -> bool {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym prophecy_uses::tests::original);
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn run_target(_: usize, _: *mut skip_quest_echoes::tests::Run, _: bool, _: bool, _: i32, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym skip_quest_echoes::tests::original);
}

#[test]
fn progress_hooks_use_shared_on_off_readback_manager_loss_and_unload() {
    let online = AtomicU8::new(0);
    for hook in [GameplayHook::ProphecyUses, GameplayHook::SkipQuestEchoes] {
        // SAFETY: these padded targets have the exact validated six-position ABIs, over retained fixture storage.
        let group = unsafe {
            let address = if hook == GameplayHook::ProphecyUses {
                prophecy_target as *mut u8
            } else {
                run_target as *mut u8
            };
            let method = ResolvedMethod {
                name: hook.id().into(),
                address: NonNull::new(address).unwrap(),
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
            };
            let target = if hook == GameplayHook::ProphecyUses {
                method.postfix(prophecy_uses::effect(NonNull::from(&online).cast(), prophecy_uses::tests::charges_offset()))
            } else {
                method.leaf(skip_quest_echoes::tests::layout(&online).effect(), None, None)
            };
            Group::prepare(FeatureBinding::fixture(hook), &[target]).unwrap()
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(hook)] = Some(group);
        let check = |enabled| {
            // SAFETY: black_box avoids folding calls across target patch mutations; objects/output survive the calls.
            unsafe {
                if hook == GameplayHook::ProphecyUses {
                    let mut slot = prophecy_uses::tests::Slot::new(6);
                    let mut consumed = -1;
                    assert!(prophecy_uses::tests::invoke(std::hint::black_box(prophecy_target) as *const (), &mut slot, &mut consumed));
                    assert_eq!((slot.charges, slot.calls, consumed), (if enabled { 6 } else { 2 }, 1, 4));
                } else {
                    let mut run = skip_quest_echoes::tests::Run::new();
                    skip_quest_echoes::tests::invoke(std::hint::black_box(run_target) as *const (), &mut run);
                    assert_eq!((run.quests, run.calls), (if enabled { 2 } else { 0 }, 1));
                }
            }
        };
        for on in [true, false, true] {
            let group = owner.slots[slot_of(hook)].as_ref().unwrap();
            change_group(group, hook, on, 1).unwrap();
            assert!(observation(Some(group), hook).confirmed(on));
            check(on);
        }
        for manager in [2, 0] {
            owner.observe_manager(manager).unwrap();
            assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
            check(false);
            change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, 1).unwrap();
        }
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
        check(false);
    }
}
