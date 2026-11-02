use super::*;
use native_hooks::postfix::PostfixGate;
use std::sync::atomic::{AtomicU8, Ordering::SeqCst};

#[test]
fn density_scales_without_compounding_preserves_config_and_restores_on_unwind() {
    #[repr(C)]
    struct Room {
        density: f32,
        observed: f32,
    }
    unsafe extern "C-unwind" fn original(room: *mut Room, config: u16, method: usize) {
        assert_eq!(config, 0x0101);
        // SAFETY: fixture retains its room exclusively through the call.
        let room = unsafe { &mut *room };
        room.observed = room.density;
        if method == 2 {
            panic!("placement threw");
        }
    }
    let online = AtomicU8::new(0);
    let mut room = Room { density: 0.5, observed: 0.0 };
    // SAFETY: aligned f32 field on owned room, exact receiver/two-byte-config/MethodInfo ABI.
    unsafe {
        let gate = PostfixGate::prepare_scoped(&effect(NonNull::from(&online).cast(), 0), None).unwrap();
        gate.set_original(original as *const ());
        gate.set_value(3f64.to_bits());
        gate.set_enabled(true);
        let call: unsafe extern "C-unwind" fn(*mut Room, u16, usize) = std::mem::transmute(gate.entry());
        for _ in 0..3 {
            call(&raw mut room, 0x0101, 1);
            assert_eq!((room.density, room.observed), (0.5, 3.5));
        }
        let pointer = &raw mut room;
        assert!(std::panic::catch_unwind(|| call(pointer, 0x0101, 2)).is_err());
        assert_eq!((room.density, room.observed), (0.5, 3.5));
        online.store(1, SeqCst);
        call(&raw mut room, 0x0101, 1);
        assert_eq!(room.observed, 0.5);
        online.store(0, SeqCst);
        gate.set_enabled(false);
        call(&raw mut room, 0x0101, 1);
        assert_eq!(room.observed, 0.5);
    }
}
