use super::*;
use crate::game::{
    death_drops::{
        self,
        tests::{Drop, layout, original},
    },
    registry::ResolvedMethod,
};
use std::sync::atomic::AtomicU8;

#[unsafe(naked)]
unsafe extern "C-unwind" fn target(_: *mut Drop, _: usize, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym original);
}

#[test]
fn shared_death_drop_target_reads_both_words_and_restores_after_last_owner_manager_loss_and_unload() {
    let online = AtomicU8::new(0);
    let rates = GameplayHook::DropRates;
    let rare = GameplayHook::ExtraRare;
    let value = rates.value_kind().encode(&[0.25, 3.0, 0.75, 8.0]).unwrap();
    // SAFETY: padded three-position void target and authored component/static byte, retained through restoration.
    let group = unsafe {
        let method = ResolvedMethod {
            name: "fixture death drop".into(),
            address: NonNull::new(target as *mut u8).unwrap(),
            bytes: (target as *const [u8; 16]).read_unaligned(),
        };
        Group::prepare(FeatureBinding::fixture(rates), &[method.leaf(layout(&online).effect(), None, None)]).unwrap()
    };
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    owner.slots[slot_of(rates)] = Some(group);
    let check = |rates, rare| {
        let mut drop = Drop::default();
        // SAFETY: exact authored signature, black_box prevents folding across patch mutations.
        unsafe {
            std::hint::black_box(target)(&mut drop, 0x1234, 0xabcd);
        }
        assert_eq!((drop.override_item, drop.override_gold, drop.extra_rare, drop.calls), (rates, rates, rare, 1));
        if rates {
            assert_eq!((drop.item_chance, drop.gold_multiplier), (0.25, 8.0));
        }
    };
    for (hook, on) in [(rates, true), (rare, true), (rates, false), (rates, true), (rare, false), (rates, false)] {
        let group = owner.slots[slot_of(rates)].as_ref().unwrap();
        let setting = if hook == rates { value } else { 1 };
        change_group(group, hook, on, setting).unwrap();
        let a = observation(Some(group), rates);
        let b = observation(Some(group), rare);
        assert!(observation(Some(group), hook).confirmed(on));
        if a.enabled {
            assert_eq!(a.value, value);
        }
        check(a.enabled, b.enabled);
        assert_eq!(group.hooks[0].state(), if a.enabled || b.enabled { HookState::Enabled } else { HookState::Disabled });
    }
    for manager in [2, 0] {
        let group = owner.slots[slot_of(rates)].as_ref().unwrap();
        change_group(group, rates, true, value).unwrap();
        change_group(group, rare, true, 1).unwrap();
        owner.observe_manager(manager).unwrap();
        check(false, false);
        for hook in death_drops::FEATURE.controls {
            assert!(observation(owner.slots[slot_of(rates)].as_ref(), *hook).confirmed(false));
        }
    }
    change_group(owner.slots[slot_of(rates)].as_ref().unwrap(), rare, true, 1).unwrap();
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots.iter().all(Option::is_none));
    check(false, false);
}
