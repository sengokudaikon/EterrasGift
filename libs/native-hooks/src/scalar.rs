//! Retained scalar gateways for void instance methods. Original-first: after the game recalculates a scalar, remember
//! that result and apply the selected bits. No generated path enters this DLL. The owner pins the receiver until
//! closing and draining the shared state, then conditionally restores the latest game value before releasing it.
use super::{HookError, page};
use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst},
    time::{Duration, Instant},
};

#[repr(C)]
struct State {
    enabled: AtomicU64,
    active: AtomicU64,
    receiver: usize,
    online: usize,
    desired: AtomicU32,
    baseline: AtomicU32,
}

const _: () = assert!(std::mem::offset_of!(State, desired) == 32 && std::mem::offset_of!(State, baseline) == 36);
const _: () = assert!(std::mem::size_of::<State>() <= 48 && 48 + std::mem::size_of::<AtomicU64>() <= page::STATE_END);

pub struct ScalarGate {
    entry: usize,
    original: &'static AtomicU64,
    state: &'static State,
    field: usize,
    value: AtomicU64,
}
impl ScalarGate {
    /// # Safety
    /// Windows x64 void instance method, at most five ABI argument positions, with no hidden return buffer.
    /// `stack_argument` means exactly one argument follows the four register positions. Receiver is pinned,
    /// nonmoving, and its aligned four-byte field stays readable/writable until close/drain/restore. `online`
    /// is a process-lifetime byte. The supported game serializes recalculation of this receiver on its main thread.
    pub unsafe fn prepare(receiver: usize, offset: u32, online: usize, baseline: u32, stack_argument: bool, shared: Option<&Self>) -> Result<Self, HookError> {
        // SAFETY: caller supplies the pinned field and exact original-first ABI.
        unsafe { Self::build(receiver, offset, online, baseline, stack_argument, shared, false) }
    }
    /// A before-original scalar: preserve a foreign field change as the new baseline, then write the override.
    /// # Safety
    /// As `prepare`; the original is void(instance, MethodInfo), serialized on the main thread. It reads rather
    /// than recalculates this field. The owner closes/drains/restores before releasing the pinned receiver.
    pub unsafe fn prepare_before(receiver: usize, offset: u32, online: usize, baseline: u32) -> Result<Self, HookError> {
        // SAFETY: caller supplies this before-original ABI and pinned storage.
        unsafe { Self::build(receiver, offset, online, baseline, false, None, true) }
    }
    unsafe fn build(receiver: usize, offset: u32, online: usize, baseline: u32, stack_argument: bool, shared: Option<&Self>, before: bool) -> Result<Self, HookError> {
        // SAFETY: private pages, initialized before publication; state/pages survive until process exit.
        unsafe {
            let allocation = page::allocate("allocate scalar gateway")?;
            let field = receiver + offset as usize;
            let state = shared.map_or_else(
                || {
                    let state = allocation.add(4096).cast::<State>();
                    state.write(State {
                        enabled: AtomicU64::new(0),
                        active: AtomicU64::new(0),
                        receiver,
                        online,
                        desired: AtomicU32::new(0),
                        baseline: AtomicU32::new(baseline),
                    });
                    &*state
                },
                |gate| gate.state,
            );
            let original = allocation.add(4096 + 48).cast::<AtomicU64>();
            original.write(AtomicU64::new(0));
            // 32-byte home area, optional fifth argument at +20, saved receiver at +28; 56-byte aligned frame.
            let mut code = vec![0x48, 0x83, 0xec, 0x38, 0x48, 0x89, 0x4c, 0x24, 0x28];
            if stack_argument {
                code.extend([0x48, 0x8b, 0x44, 0x24, 0x60, 0x48, 0x89, 0x44, 0x24, 0x20]);
            }
            code.extend([0x48, 0xb8]);
            code.extend((original as u64).to_le_bytes());
            if !before {
                code.extend([0xff, 0x10]);
            }
            code.extend([0x48, 0x8b, 0x4c, 0x24, 0x28, 0x48, 0xb8]);
            code.extend((state as *const State as u64).to_le_bytes());
            code.extend([0xf0, 0x48, 0xff, 0x40, 0x08]); // active++ before checking admission
            let mut skips = Vec::new();
            code.extend([0x48, 0x83, 0x38, 0, 0x74, 0]);
            skips.push(code.len() - 1);
            code.extend([0x48, 0x3b, 0x48, 0x10, 0x75, 0]);
            skips.push(code.len() - 1); // exact pinned receiver
            code.extend([0x4c, 0x8b, 0x58, 0x18, 0x41, 0x80, 0x3b, 0, 0x75, 0]);
            skips.push(code.len() - 1);
            if before {
                // RDX is the original MethodInfo: use R10/R11, preserving both incoming argument registers.
                code.extend([0x44, 0x8b, 0x91]);
                code.extend(offset.to_le_bytes());
                code.extend([0x44, 0x3b, 0x50, 0x20, 0x74, 4, 0x44, 0x89, 0x50, 0x24]);
                code.extend([0x44, 0x8b, 0x50, 0x20, 0x44, 0x89, 0x91]);
                code.extend(offset.to_le_bytes());
            } else {
                code.extend([0x8b, 0x91]);
                code.extend(offset.to_le_bytes()); // calculated bits
                code.extend([0x89, 0x50, 0x24, 0x8b, 0x50, 0x20]); // baseline = calculated; desired -> edx
                code.extend([0x89, 0x91]);
                code.extend(offset.to_le_bytes());
            }
            for skip in skips {
                code[skip] = (code.len() - skip - 1) as u8;
            }
            code.extend([0xf0, 0x48, 0xff, 0x48, 0x08]);
            if before {
                code.extend([0x48, 0xb8]);
                code.extend((original as u64).to_le_bytes());
                code.extend([0xff, 0x10]);
            }
            code.extend([0x48, 0x83, 0xc4, 0x38, 0xc3]);
            std::ptr::copy_nonoverlapping(code.as_ptr(), allocation, code.len());
            let table = page::unwind(allocation, code.len(), 0x38);
            page::publish(allocation, Some(table), "publish scalar gateway")?;
            Ok(Self {
                entry: allocation as usize,
                original: &*original,
                state,
                field,
                value: AtomicU64::new(0),
            })
        }
    }
    pub fn entry(&self) -> *const () {
        self.entry as *const ()
    }
    /// # Safety
    /// Set once before publication to a retained original with the prepared ABI.
    pub unsafe fn set_original(&self, original: *const ()) {
        self.original.store(original as u64, SeqCst);
    }
    pub fn enabled(&self) -> bool {
        self.state.enabled.load(SeqCst) != 0
    }
    pub fn close(&self) {
        self.state.enabled.store(0, SeqCst);
    }
    pub fn value(&self) -> u64 {
        self.value.load(SeqCst)
    }
    pub fn drain(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.state.active.load(SeqCst) != 0 {
            if Instant::now() >= deadline {
                return Err("Scalar override did not drain".into());
            }
            std::thread::yield_now();
        }
        Ok(())
    }
    /// # Safety
    /// The pinned field remains valid. Called by the serialized owner on the main thread after closing/draining.
    pub unsafe fn enable(&self, value: u64) {
        // SAFETY: the owner holds the pin and excludes recalculation during this main-thread operation.
        let field = unsafe { &*(self.field as *const AtomicU32) };
        self.state.baseline.store(field.load(SeqCst), SeqCst);
        let bits = (f64::from_bits(value) as f32).to_bits();
        self.value.store(value, SeqCst);
        self.state.desired.store(bits, SeqCst);
        field.store(bits, SeqCst);
        self.state.enabled.store(1, SeqCst);
    }
    /// # Safety
    /// Close and drain first; keep the pin until this returns. The atomic conditional store does not replace a
    /// newer game write. No Unity/native object is accessed, only the pinned managed scalar storage.
    pub unsafe fn restore(&self) {
        // SAFETY: the pin retains the managed scalar storage, including during shutdown on the attached control thread.
        let field = unsafe { &*(self.field as *const AtomicU32) };
        let _ = field.compare_exchange(self.state.desired.load(SeqCst), self.state.baseline.load(SeqCst), SeqCst, SeqCst);
    }
    /// # Safety
    /// The pin is held and reads of the scalar are permitted.
    pub unsafe fn matches(&self) -> bool {
        // SAFETY: pinned aligned scalar; no object graph traversal.
        unsafe { &*(self.field as *const AtomicU32) }.load(SeqCst) == self.state.desired.load(SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    };
    #[test]
    fn before_original_tracks_foreign_changes_keeps_method_info_and_restores_each_bound_camera() {
        #[repr(C)]
        struct Camera {
            distance: AtomicU32,
            seen: AtomicU32,
        }
        unsafe extern "C-unwind" fn zoom(camera: *const Camera, method: usize) {
            assert_eq!(method, 0x12345678);
            // SAFETY: the fixture retains aligned camera storage through the call.
            let camera = unsafe { &*camera };
            camera.seen.store(camera.distance.load(SeqCst), SeqCst);
        }
        let online = AtomicU8::new(0);
        let camera = Camera {
            distance: AtomicU32::new((-17.5f32).to_bits()),
            seen: AtomicU32::new(0),
        };
        let other = Camera {
            distance: AtomicU32::new((-20f32).to_bits()),
            seen: AtomicU32::new(0),
        };
        // SAFETY: live aligned pinned test storage, exact void(receiver, MethodInfo) ABI, serialized calls.
        unsafe {
            let gate = ScalarGate::prepare_before(&camera as *const _ as usize, 0, &online as *const _ as usize, (-17.5f32).to_bits()).unwrap();
            gate.set_original(zoom as *const ());
            gate.enable((-30f64).to_bits());
            let call: unsafe extern "C-unwind" fn(*const Camera, usize) = std::mem::transmute(gate.entry());
            call(&camera, 0x12345678);
            assert_eq!(f32::from_bits(camera.seen.load(SeqCst)), -30.0);
            call(&other, 0x12345678);
            assert_eq!(f32::from_bits(other.seen.load(SeqCst)), -20.0);
            camera.distance.store((-22f32).to_bits(), SeqCst);
            call(&camera, 0x12345678);
            assert_eq!(f32::from_bits(camera.seen.load(SeqCst)), -30.0);
            gate.close();
            gate.drain().unwrap();
            gate.restore();
            assert_eq!(f32::from_bits(camera.distance.load(SeqCst)), -22.0);
            gate.enable((-40f64).to_bits());
            camera.distance.store((-24f32).to_bits(), SeqCst);
            gate.close();
            gate.drain().unwrap();
            gate.restore();
            assert_eq!(f32::from_bits(camera.distance.load(SeqCst)), -24.0);
            let replacement = ScalarGate::prepare_before(&other as *const _ as usize, 0, &online as *const _ as usize, (-20f32).to_bits()).unwrap();
            replacement.set_original(zoom as *const ());
            replacement.enable((-30f64).to_bits());
            let call: unsafe extern "C-unwind" fn(*const Camera, usize) = std::mem::transmute(replacement.entry());
            call(&other, 0x12345678);
            assert_eq!(f32::from_bits(other.seen.load(SeqCst)), -30.0);
            replacement.close();
            replacement.drain().unwrap();
            replacement.restore();
            assert_eq!(f32::from_bits(other.distance.load(SeqCst)), -20.0);
        }
    }
    #[repr(C)]
    struct Stats {
        attack: AtomicU32,
        leech: AtomicU32,
        observed_method: AtomicU64,
    }
    unsafe extern "C-unwind" fn weapon(this: *const Stats, rate: f32, main: u64, off: u64, method: u64) {
        // SAFETY: fixture owns this receiver through the call.
        let stats = unsafe { &*this };
        stats.attack.store((rate + main as f32 + off as f32).to_bits(), SeqCst);
        stats.observed_method.store(method, SeqCst);
    }
    unsafe extern "C-unwind" fn recalc_leech(this: *const Stats, rate: f32) {
        // SAFETY: fixture owns this receiver through the call.
        unsafe { &*this }.leech.store(rate.to_bits(), SeqCst);
    }
    #[test]
    fn independent_overrides_follow_recalculations_and_restore_the_latest_values() {
        let stats = Stats {
            attack: AtomicU32::new(1f32.to_bits()),
            leech: AtomicU32::new(0),
            observed_method: AtomicU64::new(0),
        };
        let other = Stats {
            attack: AtomicU32::new(0),
            leech: AtomicU32::new(0),
            observed_method: AtomicU64::new(0),
        };
        let online = AtomicU8::new(0);
        // SAFETY: owned aligned scalar storage and matching retained void ABIs, with the fifth weapon argument on the stack.
        unsafe {
            let attack = ScalarGate::prepare(&stats as *const _ as usize, 0, &online as *const _ as usize, 1f32.to_bits(), true, None).unwrap();
            let leech = ScalarGate::prepare(&stats as *const _ as usize, 4, &online as *const _ as usize, 0, false, None).unwrap();
            attack.set_original(weapon as *const ());
            leech.set_original(recalc_leech as *const ());
            let hit: unsafe extern "C-unwind" fn(*const Stats, f32, u64, u64, u64) = std::mem::transmute(attack.entry());
            let heal: unsafe extern "C-unwind" fn(*const Stats, f32) = std::mem::transmute(leech.entry());
            attack.enable(8f64.to_bits());
            leech.enable(3f64.to_bits());
            hit(&stats, 2.0, 1, 2, 0x12345678);
            heal(&stats, 0.5);
            assert_eq!(stats.observed_method.load(SeqCst), 0x12345678);
            assert_eq!(f32::from_bits(stats.attack.load(SeqCst)), 8.0);
            assert_eq!(f32::from_bits(stats.leech.load(SeqCst)), 3.0);
            hit(&other, 4.0, 0, 0, 1);
            assert_eq!(f32::from_bits(other.attack.load(SeqCst)), 4.0);
            attack.close();
            attack.drain().unwrap();
            attack.restore();
            assert_eq!(f32::from_bits(stats.attack.load(SeqCst)), 5.0);
            assert!(leech.enabled());
            assert!(leech.matches());
            leech.close();
            leech.drain().unwrap();
            leech.restore();
            assert_eq!(f32::from_bits(stats.leech.load(SeqCst)), 0.5);
            // An independent later game store wins over conditional restoration.
            leech.enable(2f64.to_bits());
            leech.close();
            stats.leech.store(0.75f32.to_bits(), SeqCst);
            leech.restore();
            assert_eq!(f32::from_bits(stats.leech.load(SeqCst)), 0.75);
            leech.enable(4f64.to_bits());
            online.store(1, SeqCst);
            heal(&stats, 1.5);
            assert_eq!(f32::from_bits(stats.leech.load(SeqCst)), 1.5);
            leech.close();
            leech.drain().unwrap();
            leech.restore();
        }
    }
    #[test]
    fn original_paused_across_close_cannot_apply_an_override_after_departure() {
        #[repr(C)]
        struct Waiting {
            value: AtomicU32,
            entered: AtomicBool,
            release: AtomicBool,
        }
        unsafe extern "C-unwind" fn original(this: *const Waiting) {
            // SAFETY: caller holds an Arc until the thread joins.
            let this = unsafe { &*this };
            this.entered.store(true, SeqCst);
            while !this.release.load(SeqCst) {
                std::thread::yield_now();
            }
            this.value.store(7f32.to_bits(), SeqCst);
        }
        let object = Arc::new(Waiting {
            value: AtomicU32::new(1f32.to_bits()),
            entered: AtomicBool::new(false),
            release: AtomicBool::new(false),
        });
        let online = AtomicU8::new(0);
        // SAFETY: retained test code, pinned Arc storage and exact void(pointer) ABI.
        let gate = unsafe { ScalarGate::prepare(Arc::as_ptr(&object) as usize, 0, &online as *const _ as usize, 1f32.to_bits(), false, None).unwrap() };
        // SAFETY: gateway and original share the exact ABI; Arc keeps the scalar storage alive.
        unsafe {
            gate.set_original(original as *const ());
            gate.enable(3f64.to_bits());
        }
        let entry = gate.entry() as usize;
        let caller = object.clone();
        // SAFETY: the retained entry has this exact signature and caller owns the Arc through return.
        let thread = std::thread::spawn(move || unsafe {
            let call: unsafe extern "C-unwind" fn(*const Waiting) = std::mem::transmute(entry);
            call(Arc::as_ptr(&caller));
        });
        while !object.entered.load(SeqCst) {
            std::thread::yield_now();
        }
        gate.close();
        gate.drain().unwrap();
        // SAFETY: closed and drained; the Arc still pins the field.
        unsafe {
            gate.restore();
        }
        assert_eq!(f32::from_bits(object.value.load(SeqCst)), 1.0);
        let _retired = gate;
        object.release.store(true, SeqCst);
        thread.join().unwrap();
        assert_eq!(f32::from_bits(object.value.load(SeqCst)), 7.0);
    }
}
