use super::*;
use crate::game::{encounter_counts::tests as fixture, registry::ResolvedMethod};
use std::sync::atomic::AtomicU8;

#[test]
fn encounter_controls_share_one_entry_and_keep_independent_values_through_off_manager_loss_and_clear() {
    #[unsafe(naked)]
    unsafe extern "C-unwind" fn calculate(_: usize, _: *mut fixture::Placement, _: usize, _: usize, _: usize) {
        core::arch::naked_asm!("nop","nop","nop","nop","nop","jmp {}",sym fixture::original);
    }
    let online = AtomicU8::new(0);
    // SAFETY: authored padded five-position static void entry and retained fixture storage/layout.
    let group = unsafe {
        let target = ResolvedMethod {
            name: "encounter fixture".into(),
            address: NonNull::new(calculate as *mut u8).unwrap(),
            bytes: (calculate as *const [u8; 16]).read_unaligned(),
        }
        .postfix(fixture::layout(&online).effect());
        Group::prepare(FeatureBinding::fixture(GameplayHook::LootLizards), &[target]).unwrap()
    };
    let (lizards, wardens) = (GameplayHook::LootLizards, GameplayHook::RuneWardens);
    assert_eq!(slot_of(lizards), slot_of(wardens));
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    owner.slots[slot_of(lizards)] = Some(group);
    let check = |counts| {
        let mut f = fixture::Fixture::new(2.0);
        // SAFETY: exact fixture ABI, independent fresh graph and retained original/gateway code.
        unsafe {
            fixture::invoke(calculate as *const () as usize, &mut *f.placement);
        }
        assert_eq!((f.context.tallies[0].total, f.context.tallies[1].total), counts);
    };
    let group = owner.slots[slot_of(lizards)].as_ref().unwrap();
    change_group(group, lizards, true, 5.0f64.to_bits().into()).unwrap();
    change_group(group, wardens, true, 7.0f64.to_bits().into()).unwrap();
    for (hook, value) in [(lizards, 5.0f64), (wardens, 7.0)] {
        let read = observation(Some(group), hook);
        assert!(read.confirmed(true));
        assert_eq!(read.value, value.to_bits().into());
    }
    check((5, 7));
    change_group(group, lizards, false, 5.0f64.to_bits().into()).unwrap();
    let off = observation(Some(group), lizards);
    assert!(off.confirmed(false));
    assert_eq!(off.patch, PatchState::Shared);
    assert!(observation(Some(group), wardens).confirmed(true));
    check((4, 7));
    change_group(group, lizards, true, 0.0f64.to_bits().into()).unwrap();
    check((0, 7));
    owner.observe_manager(2).unwrap();
    for hook in [lizards, wardens] {
        assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
    }
    check((4, 4));
    let group = owner.slots[slot_of(lizards)].as_ref().unwrap();
    change_group(group, wardens, true, 20.0f64.to_bits().into()).unwrap();
    check((4, 20));
    change_group(group, wardens, false, 20.0f64.to_bits().into()).unwrap();
    check((4, 4));
    assert_eq!(group.hooks[0].state(), HookState::Disabled);
    change_group(group, lizards, true, 10.0f64.to_bits().into()).unwrap();
    check((10, 4));
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots.iter().all(Option::is_none));
    check((4, 4));
}
