use super::*;
use native_hooks::postfix::PostfixGate;
use std::sync::atomic::AtomicU8;

#[test]
fn pack_size_and_rarity_compose_independently_restore_on_throw_and_preserve_lone_monsters() {
    #[repr(C)]
    struct Spawner {
        values: [f32; 4],
        observed: [f32; 4],
    }
    unsafe extern "C-unwind" fn original(spawner: *mut Spawner, method: usize) {
        // SAFETY: fixture retains exclusive receiver storage throughout the synchronous original call.
        let s = unsafe { &mut *spawner };
        s.observed = s.values;
        if method == 2 {
            panic!("generation threw");
        }
    }
    let online = AtomicU8::new(0);
    let layout = Layout {
        online: NonNull::from(&online).cast(),
        count: 0,
        weights: [4, 8, 12],
    };
    let mut s = Spawner {
        values: [3.0, 10.0, 2.0, 1.0],
        observed: [0.0; 4],
    };
    // SAFETY: four aligned owned floats and exact void(receiver, MethodInfo) ABI.
    unsafe {
        let gate = PostfixGate::prepare_scoped(&layout.effect(), None).unwrap();
        gate.set_original(original as *const ());
        SETTINGS.set(
            &gate,
            GameplayHook::MonsterRarity,
            true,
            GameplayHook::MonsterRarity.value_kind().encode(&[0.0, 2.0, 4.0]).unwrap(),
        );
        SETTINGS.set(&gate, GameplayHook::PackSize, true, 2f64.to_bits().into());
        gate.set_enabled(true);
        let call: unsafe extern "C-unwind" fn(*mut Spawner, usize) = std::mem::transmute(gate.entry());
        for _ in 0..3 {
            call(&raw mut s, 1);
            assert_eq!(s.observed, [6.0, 0.0, 4.0, 4.0]);
            assert_eq!(s.values, [3.0, 10.0, 2.0, 1.0]);
        }
        let pointer = &raw mut s;
        assert!(std::panic::catch_unwind(|| call(pointer, 2)).is_err());
        assert_eq!(s.values, [3.0, 10.0, 2.0, 1.0]);
        s.values[0] = 1.0;
        call(&raw mut s, 1);
        assert_eq!(s.observed, [1.0, 0.0, 4.0, 4.0]);
        SETTINGS.set(&gate, GameplayHook::MonsterRarity, false, GameplayHook::MonsterRarity.default_value());
        s.values[0] = 4.0;
        call(&raw mut s, 1);
        assert_eq!(s.observed, [8.0, 10.0, 2.0, 1.0]);
        SETTINGS.set(&gate, GameplayHook::PackSize, false, GameplayHook::PackSize.default_value());
        SETTINGS.set(
            &gate,
            GameplayHook::MonsterRarity,
            true,
            GameplayHook::MonsterRarity.value_kind().encode(&[1.0, 3.0, 0.0]).unwrap(),
        );
        call(&raw mut s, 1);
        assert_eq!(s.observed, [4.0, 10.0, 6.0, 0.0]);
        gate.set_enabled(false);
        call(&raw mut s, 1);
        assert_eq!(s.observed, s.values);
    }
}
