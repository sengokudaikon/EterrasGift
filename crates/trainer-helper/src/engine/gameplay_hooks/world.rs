use super::*;
use crate::game::{registry::ResolvedMethod, spawner_group};
use std::sync::atomic::{AtomicU8, AtomicU32};

#[repr(C)]
struct Spawner {
    values: [f32; 4],
    seen: [f32; 4],
}
unsafe extern "C-unwind" fn generate(this: *mut Spawner, method: usize) {
    assert_eq!(method, 0x1234);
    // SAFETY: the fixture retains its exclusive receiver through this synchronous native call.
    unsafe {
        (*this).seen = (*this).values;
    }
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn group_entry(_: *mut Spawner, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym generate);
}

#[test]
fn world_spawner_controls_share_one_entry_readback_and_departure_cleanup() {
    let online = AtomicU8::new(0);
    let size = GameplayHook::PackSize;
    let rarity = GameplayHook::MonsterRarity;
    let value = rarity.value_kind().encode(&[1.0, 2.0, 3.0]).unwrap();
    // SAFETY: padded exact-ABI original, owned floats, process-retained entry and online byte throughout the test.
    let group = unsafe {
        let address = group_entry as *mut u8;
        let target = ResolvedMethod {
            name: "fixture spawner".into(),
            address: NonNull::new(address).unwrap(),
            bytes: address.cast::<[u8; 16]>().read_unaligned(),
        }
        .scoped(
            spawner_group::Layout {
                online: NonNull::from(&online).cast(),
                count: 0,
                weights: [4, 8, 12],
            }
            .effect(),
        );
        Group::prepare(FeatureBinding::fixture(size), &[target]).unwrap()
    };
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    owner.slots[slot_of(size)] = Some(group);
    let mut s = Spawner {
        values: [4.0, 10.0, 2.0, 1.0],
        seen: [0.0; 4],
    };
    let check = |s: &mut Spawner, wanted| {
        // SAFETY: exact fixture signature; black_box prevents optimization across checked entry mutations.
        unsafe { std::hint::black_box(group_entry)(s, 0x1234) };
        assert_eq!(s.seen, wanted);
        assert_eq!(s.values, [4.0, 10.0, 2.0, 1.0]);
    };
    let group = owner.slots[slot_of(size)].as_ref().unwrap();
    change_group(group, size, true, 2f64.to_bits().into()).unwrap();
    assert!(observation(Some(group), size).confirmed(true));
    assert!(observation(Some(group), rarity).confirmed(false));
    check(&mut s, [8.0, 10.0, 2.0, 1.0]);
    change_group(group, rarity, true, value).unwrap();
    check(&mut s, [8.0, 10.0, 4.0, 3.0]);
    assert_eq!(observation(Some(group), rarity).value, value);
    change_group(group, size, false, size.default_value()).unwrap();
    assert_eq!(observation(Some(group), size).patch, PatchState::Shared);
    check(&mut s, [4.0, 10.0, 4.0, 3.0]);
    owner.observe_manager(0).unwrap();
    check(&mut s, [4.0, 10.0, 2.0, 1.0]);
    assert!(observation(owner.slots[slot_of(rarity)].as_ref(), rarity).confirmed(false));
    let group = owner.slots[slot_of(size)].as_ref().unwrap();
    change_group(group, size, true, 3f64.to_bits().into()).unwrap();
    // Reopening a cleared group must not resurrect its former sibling setting.
    check(&mut s, [12.0, 10.0, 2.0, 1.0]);
    assert!(observation(Some(group), rarity).confirmed(false));
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots[slot_of(size)].is_none());
    check(&mut s, [4.0, 10.0, 2.0, 1.0]);
}

#[repr(C)]
struct Camera {
    distance: AtomicU32,
    seen: AtomicU32,
    pins: AtomicU32,
}
unsafe extern "C-unwind" fn zoom(this: *const Camera, _: usize) {
    // SAFETY: fixture's stable camera storage remains retained throughout this call.
    let this = unsafe { &*this };
    this.seen.store(this.distance.load(SeqCst), SeqCst);
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn camera_entry(_: *const Camera, _: usize) {
    core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym zoom);
}
#[test]
fn world_camera_rebinds_through_shared_lifecycle_and_restores_before_pin_release() {
    use il2cpp_runtime::{Exports, raw};
    unsafe extern "C-unwind" fn pin(object: *mut raw::Object, pinned: bool) -> *mut c_void {
        assert!(pinned);
        // SAFETY: fixture's opaque object is a retained Camera, never a real runtime object.
        unsafe { &*object.cast::<Camera>() }.pins.fetch_add(1, SeqCst);
        object.cast()
    }
    unsafe extern "C-unwind" fn free(handle: *mut c_void) {
        // SAFETY: the fake handle is the same retained Camera pointer returned by pin.
        assert_eq!(unsafe { &*handle.cast::<Camera>() }.pins.fetch_sub(1, SeqCst), 1);
    }
    let mut api = Exports::test_stubs();
    api.gchandle_new = pin;
    api.gchandle_free = free;
    let camera = Camera {
        distance: AtomicU32::new((-17.5f32).to_bits()),
        seen: AtomicU32::new(0),
        pins: AtomicU32::new(0),
    };
    let replacement = Camera {
        distance: AtomicU32::new((-19f32).to_bits()),
        seen: AtomicU32::new(0),
        pins: AtomicU32::new(0),
    };
    let online = AtomicU8::new(0);
    let hook = GameplayHook::CameraDistance;
    let binding = |camera: &Camera| {
        // SAFETY: owned aligned stable test storage, fake pin API, and exact field baseline at acquisition.
        let binding = unsafe { crate::game::ScalarBinding::pin(&api, camera as *const _ as usize, 0, camera.distance.load(SeqCst), &online as *const _ as usize) }.unwrap();
        FeatureBinding::fixture_bound(hook, binding)
    };
    let targets = || {
        let address = NonNull::new(camera_entry as *mut u8).unwrap();
        // SAFETY: padded retained fixture method, read only after its preceding entry is restored.
        let bytes = unsafe { address.cast::<[u8; 16]>().read_unaligned() };
        Ok(vec![
            ResolvedMethod {
                name: "fixture camera".into(),
                address,
                bytes,
            }
            .scalar(false),
        ])
    };
    let mut owner = Hooks {
        slots: [const { None }; FEATURES.len()],
        manager: 1,
        closing: false,
    };
    // SAFETY: local serialized owner, pinned field and exact padded void(receiver, MethodInfo) original.
    unsafe { change_slot(&mut owner.slots[slot_of(hook)], hook, true, (-30f64).to_bits().into(), binding(&camera), targets) }.unwrap();
    assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(true));
    assert_eq!(camera.pins.load(SeqCst), 1);
    // SAFETY: exact padded fixture ABI, retained storage throughout the entry transaction.
    unsafe { std::hint::black_box(camera_entry)(&camera, 0) };
    assert_eq!(f32::from_bits(camera.seen.load(SeqCst)), -30.0);
    camera.distance.store((-22f32).to_bits(), SeqCst);
    assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(true));
    // SAFETY: same pinned receiver and signature; adopt this foreign distance as the game baseline.
    unsafe { std::hint::black_box(camera_entry)(&camera, 0) };
    // SAFETY: same transition used by main-thread replacement maintenance: old field/entry restore precede pin release.
    unsafe { change_slot(&mut owner.slots[slot_of(hook)], hook, true, (-30f64).to_bits().into(), binding(&replacement), targets) }.unwrap();
    assert_eq!(camera.pins.load(SeqCst), 0);
    assert_eq!(replacement.pins.load(SeqCst), 1);
    assert_eq!(f32::from_bits(camera.distance.load(SeqCst)), -22.0);
    assert_eq!(f32::from_bits(replacement.distance.load(SeqCst)), -30.0);
    assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(true));
    // SAFETY: replacement is now the retained receiver of this same method entry.
    unsafe { std::hint::black_box(camera_entry)(&replacement, 0) };
    assert_eq!(f32::from_bits(replacement.seen.load(SeqCst)), -30.0);
    owner.closing = true;
    owner.clear().unwrap();
    assert!(owner.slots[slot_of(hook)].is_none());
    assert_eq!(replacement.pins.load(SeqCst), 0);
    assert_eq!(f32::from_bits(replacement.distance.load(SeqCst)), -19.0);
    // SAFETY: restored original; no override survives menu departure/helper cleanup.
    unsafe { std::hint::black_box(camera_entry)(&replacement, 0) };
    assert_eq!(f32::from_bits(replacement.seen.load(SeqCst)), -19.0);
}
