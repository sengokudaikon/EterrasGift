use super::*;
use crate::{
    game::{
        any_waypoint,
        registry::{CodeChange, ResolvedMethod, TargetEffect},
    },
    hooks::patch,
};
use std::{
    mem::offset_of,
    sync::atomic::{AtomicU8, AtomicU32},
};

#[repr(C)]
struct Waypoint {
    header: [usize; 2],
    scene: usize,
    no_waypoint: bool,
    active: bool,
}

#[test]
fn mixed_waypoint_lifecycle_restores_all_entries_after_failure_manager_loss_and_unload() {
    #[unsafe(naked)]
    unsafe extern "C-unwind" fn select(_: &AtomicU32, _: usize, _: bool, _: bool, _: &Waypoint, _: bool, _: bool, _: i32, _: usize) {
        core::arch::naked_asm!(
            "nop",
            "nop",
            "nop",
            "nop",
            "nop",
            "movzx eax,r8b",
            "movzx r11d,r9b",
            "shl r11d,1",
            "or eax,r11d",
            "mov [rcx],eax",
            "ret"
        );
    }
    #[unsafe(naked)]
    unsafe extern "C-unwind" fn travel(_: bool, _: bool, _: bool) -> bool {
        core::arch::naked_asm!(
            "xor eax,eax",
            "cmp cl,0",
            ".byte 0x0f,0x85",
            ".long 2f - . - 4",
            "cmp dl,0",
            ".byte 0x0f,0x84",
            ".long 2f - . - 4",
            "test r8b,r8b",
            ".byte 0x0f,0x85",
            ".long 2f - . - 4",
            "mov eax,1",
            "2:",
            "ret"
        );
    }
    let online = AtomicU8::new(0);
    let flags = AtomicU32::new(0);
    let waypoint = Waypoint {
        header: [0; 2],
        scene: 0,
        no_waypoint: false,
        active: false,
    };
    let layout = any_waypoint::Layout {
        online: NonNull::from(&online).cast(),
        scene: offset_of!(Waypoint, scene) as u32,
        no_waypoint: offset_of!(Waypoint, no_waypoint) as u32,
        active: offset_of!(Waypoint, active) as u32,
    };
    // SAFETY: authored padded selector and complete six-byte Jcc ranges, exclusively owned for this synchronous fixture.
    let group = unsafe {
        let method = |address: *mut u8| ResolvedMethod {
            name: "mixed fixture".into(),
            address: NonNull::new(address).unwrap(),
            bytes: address.cast::<[u8; 16]>().read_unaligned(),
        };
        let mut targets = vec![method(select as *mut u8).leaf(layout.effect(), None, None)];
        for at in [5, 14] {
            let address = (travel as *mut u8).add(at);
            assert_eq!(address.read(), 0x0f);
            targets.push(GameplayHookTarget {
                method: method(travel as *mut u8),
                effect: TargetEffect::Patch(CodeChange {
                    address: NonNull::new(address).unwrap(),
                    expected: std::slice::from_raw_parts(address, 6).to_vec(),
                    replacement: vec![0x90; 6],
                }),
            });
        }
        Group::prepare(FeatureBinding::fixture(GameplayHook::AnyWaypoint), &targets).unwrap()
    };
    let hook = GameplayHook::AnyWaypoint;
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    owner.slots[slot_of(hook)] = Some(group);
    let check = |enabled| {
        // SAFETY: fixture storage and exact nine/three-position native ABIs stay live; the owner retains all entries.
        unsafe {
            std::hint::black_box(select)(&flags, 0, false, false, &waypoint, false, false, 0, 0);
            assert_eq!(flags.load(SeqCst), if enabled { 3 } else { 0 });
            assert_eq!(std::hint::black_box(travel)(true, false, false), enabled);
            assert!(!std::hint::black_box(travel)(true, false, true), "the current-location guard still applies");
        }
    };
    let group = owner.slots[slot_of(hook)].as_ref().unwrap();
    check(false);
    let failed = patch::with_write_failure(1, || change_group(group, hook, true, 1));
    assert!(failed.is_err(), "a second travel-site failure must undo the installed selector and first site");
    assert!(observation(Some(group), hook).confirmed(false));
    check(false);
    change_group(group, hook, true, 1).unwrap();
    assert!(observation(Some(group), hook).confirmed(true));
    check(true);
    change_group(group, hook, false, 1).unwrap();
    assert!(observation(Some(group), hook).confirmed(false));
    check(false);
    change_group(group, hook, true, 1).unwrap();
    let failed = patch::with_write_failure(1, || change_group(group, hook, false, 1));
    assert!(failed.is_err());
    let uncertain = observation(Some(group), hook);
    assert_eq!(uncertain.patch, PatchState::Unknown);
    assert!(!uncertain.enabled && !uncertain.confirmed(false) && !uncertain.confirmed(true));
    owner.clear().unwrap();
    assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
    check(false);
    change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, 1).unwrap();
    owner.observe_manager(2).unwrap();
    assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
    check(false);
    change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, 1).unwrap();
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots.iter().all(Option::is_none));
    check(false);
}
