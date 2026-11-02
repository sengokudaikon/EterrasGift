use super::*;
use native_hooks::leaf::LeafGate;
use std::{
    mem::offset_of,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
};

#[repr(C)]
#[derive(Default, Debug)]
pub(crate) struct Drop {
    pub override_item: bool,
    pub item_chance: f32,
    pub item_multiplier: f32,
    pub override_gold: bool,
    pub gold_chance: f32,
    pub gold_multiplier: f32,
    pub extra_rare: bool,
    pub calls: usize,
    pub fail: bool,
}
pub(crate) fn layout(online: &AtomicU8) -> Layout {
    Layout {
        online: NonNull::from(online).cast(),
        override_item: offset_of!(Drop, override_item) as u32,
        item_chance: offset_of!(Drop, item_chance) as u32,
        item_multiplier: offset_of!(Drop, item_multiplier) as u32,
        override_gold: offset_of!(Drop, override_gold) as u32,
        gold_chance: offset_of!(Drop, gold_chance) as u32,
        gold_multiplier: offset_of!(Drop, gold_multiplier) as u32,
        extra_rare: offset_of!(Drop, extra_rare) as u32,
    }
}
impl Drop {
    fn values(&self) -> [f32; 4] {
        [self.item_chance, self.item_multiplier, self.gold_chance, self.gold_multiplier]
    }
}
pub(crate) extern "C-unwind" fn original(drop: *mut Drop, dying: usize, method: usize) {
    assert_eq!((dying, method), (0x1234, 0xabcd));
    // SAFETY: each test passes a live exclusive fixture or null to check original null forwarding.
    if let Some(drop) = unsafe { drop.as_mut() } {
        drop.calls += 1;
        assert!(!drop.fail, "original failed");
    }
}
fn gate(online: &AtomicU8) -> LeafGate {
    // SAFETY: generated three-position void ABI and authored receiver/online byte, retained through all calls.
    let gate = unsafe { LeafGate::prepare_override(layout(online).effect(), None, None).unwrap() };
    // SAFETY: exact original ABI, configured before the gate is called.
    unsafe {
        gate.set_original(original as *const ());
    }
    gate
}
fn call(gate: &LeafGate, drop: *mut Drop) {
    // SAFETY: gate and receiver obey the authored OnDeath ABI.
    let call: extern "C-unwind" fn(*mut Drop, usize, usize) = unsafe { std::mem::transmute(gate.entry()) };
    call(drop, 0x1234, 0xabcd);
}

#[test]
fn rates_and_rare_are_independent_and_persist_for_delayed_drops() {
    let online = AtomicU8::new(0);
    let gate = gate(&online);
    let value = GameplayHook::DropRates.value_kind().encode(&[0.123, 1.234, 0.456, 7.89]).unwrap();
    SETTINGS.set(&gate, GameplayHook::DropRates, false, value);
    SETTINGS.set(&gate, GameplayHook::ExtraRare, false, 1);
    gate.set_enabled(true);
    for rates in [false, true] {
        for rare in [false, true] {
            SETTINGS.set(&gate, GameplayHook::DropRates, rates, value);
            SETTINGS.set(&gate, GameplayHook::ExtraRare, rare, 1);
            let mut drop = Drop::default();
            call(&gate, &mut drop);
            assert_eq!((drop.override_item, drop.override_gold, drop.extra_rare, drop.calls), (rates, rates, rare, 1));
            assert_eq!(drop.values(), if rates { [0.123, 1.234, 0.456, 7.89] } else { [0.0; 4] });
            gate.set_enabled(false);
            call(&gate, &mut drop);
            assert_eq!(drop.calls, 2);
            assert_eq!(
                (drop.override_item, drop.extra_rare),
                (rates, rare),
                "off leaves this dying enemy's settings for delayed work"
            );
            let mut next = Drop::default();
            call(&gate, &mut next);
            assert_eq!(next.values(), [0.0; 4]);
            gate.set_enabled(true);
        }
    }
    online.store(1, SeqCst);
    let mut drop = Drop::default();
    call(&gate, &mut drop);
    assert_eq!(drop.values(), [0.0; 4]);
    assert!(!drop.extra_rare);
    assert_eq!(drop.calls, 1);
    call(&gate, std::ptr::null_mut());
}

#[test]
fn an_original_exception_is_not_retried_and_keeps_the_components_drop_settings() {
    let online = AtomicU8::new(0);
    let gate = gate(&online);
    SETTINGS.set(
        &gate,
        GameplayHook::DropRates,
        true,
        GameplayHook::DropRates.value_kind().encode(&[1.0, 100.0, 0.0, 0.0]).unwrap(),
    );
    gate.set_enabled(true);
    let mut drop = Drop { fail: true, ..Drop::default() };
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(&gate, &mut drop))).is_err());
    assert_eq!(drop.calls, 1);
    assert_eq!(drop.values(), [1.0, 100.0, 0.0, 0.0]);
}

#[test]
fn game_threads_only_observe_whole_rate_snapshots_during_updates_and_close() {
    let online = AtomicU8::new(0);
    let gate = gate(&online);
    let values = [[0.125, 2.0, 0.25, 3.0], [0.75, 40.0, 0.875, 80.0]];
    let packed = values.map(|values| GameplayHook::DropRates.value_kind().encode(&values).unwrap());
    SETTINGS.set(&gate, GameplayHook::DropRates, true, packed[0]);
    gate.set_enabled(true);
    std::thread::scope(|scope| {
        let readers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    for _ in 0..20_000 {
                        let mut drop = Drop::default();
                        call(&gate, &mut drop);
                        assert_eq!(drop.calls, 1);
                        if drop.override_item {
                            assert!(drop.override_gold);
                            assert!(values.iter().any(|values| values.map(|v| v as f32) == drop.values()), "{drop:?}");
                        } else {
                            assert_eq!(drop.values(), [0.0; 4]);
                            assert!(!drop.override_gold);
                        }
                    }
                })
            })
            .collect();
        for index in 0..20_000 {
            gate.set_wide(packed[index % 2]);
        }
        gate.set_enabled(false);
        for reader in readers {
            reader.join().unwrap();
        }
    });
}
