//! Retained x64 ExecuteTasks gateway. Original execution and the return path live
//! outside the owning module's image. Closing the destination fences all future module
//! entries; the count covers every call that could have read the old destination.
//!
//! Native unwinding out of the post-call callback deliberately strands its count:
//! unloading then refuses, rather than guessing whether DLL frames remain alive.
use super::{HookError, HookState, RetainedHook, commit, page};
use std::{
    ptr::NonNull,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering::SeqCst},
    time::{Duration, Instant},
};

#[repr(C)]
struct Data {
    destination: AtomicPtr<()>,
    active: AtomicU64,
    original: AtomicPtr<()>,
}
const _: () = assert!(std::mem::offset_of!(Data, active) == 8 && std::mem::offset_of!(Data, original) == 16);
const _: () = assert!(std::mem::size_of::<Data>() <= page::STATE_END);

/// Intentionally process-owned executable/data pages and unwind table. No Drop
/// path may reclaim them: a delayed original call can return here after unload.
pub struct CallbackGate {
    code: NonNull<u8>,
    data: &'static Data,
}
impl CallbackGate {
    /// Build an unpublished gateway for `void ExecuteTasks(MethodInfo*)`.
    ///
    /// # Safety
    /// Callback uses exactly that Windows x64 ABI, returns normally, and remains
    /// loaded until close() followed by active()==0. Original is set before any
    /// entry is published and must remain callable for the process lifetime.
    pub unsafe fn prepare(callback: *const ()) -> Result<Self, HookError> {
        // SAFETY: fresh private allocation, layout below fits two pages. The
        // function table is registered only after code and data are initialized.
        unsafe {
            let allocation = page::allocate("allocate callback gateway")?;
            let code = NonNull::new(allocation).expect("allocated");
            let data = allocation.add(4096).cast::<Data>();
            data.write(Data {
                destination: AtomicPtr::new(callback.cast_mut()),
                active: AtomicU64::new(0),
                original: AtomicPtr::new(std::ptr::null_mut()),
            });
            let address = data as u64;
            // sub rsp,38; save RCX; call original; restore RCX; increment before
            // reading destination; call it; decrement only back in retained code.
            let mut bytes = vec![0x48, 0x83, 0xec, 0x38, 0x48, 0x89, 0x4c, 0x24, 0x20, 0x48, 0xb8];
            bytes.extend(address.to_le_bytes());
            bytes.extend([0xff, 0x50, 0x10, 0x48, 0x8b, 0x4c, 0x24, 0x20, 0x48, 0xb8]);
            bytes.extend(address.to_le_bytes());
            bytes.extend([0xf0, 0x48, 0xff, 0x40, 0x08, 0xff, 0x10, 0x48, 0xb8]);
            bytes.extend(address.to_le_bytes());
            bytes.extend([0xf0, 0x48, 0xff, 0x48, 0x08, 0x48, 0x83, 0xc4, 0x38, 0xc3]);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation, bytes.len());
            allocation.add(128).write(0xc3); // closed destination: retained no-op ret
            let table = page::unwind(allocation, bytes.len(), 0x38);
            page::publish(allocation, Some(table), "publish callback gateway code/unwind")?;
            Ok(Self { code, data: &*data })
        }
    }
    /// A gateway for `callback` and the hook that leads `target` into it, the gateway's original already the hook's
    /// trampoline. Nothing is published: the entry changes when the caller commits the hook. A gateway whose hook could
    /// not be prepared stays retained, as every gateway does.
    ///
    /// # Safety
    /// As [`Self::prepare`] for `callback` and [`RetainedHook::prepare`] for `target` and `expected`.
    pub unsafe fn prepare_hook(callback: *const (), target: NonNull<u8>, expected: &[u8]) -> Result<(Self, RetainedHook), HookError> {
        // SAFETY: the caller's contracts; the original is set while nothing can reach the gateway.
        unsafe {
            let gate = Self::prepare(callback)?;
            let hook = RetainedHook::prepare(target, gate.entry(), expected)?;
            gate.set_original(hook.trampoline());
            Ok((gate, hook))
        }
    }
    pub fn entry(&self) -> *const () {
        self.code.as_ptr().cast()
    }
    /// Set the retained trampoline before enabling the target hook.
    /// # Safety
    /// No gateway entry can run until this pointer has been published.
    pub unsafe fn set_original(&self, original: *const ()) {
        self.data.original.store(original.cast_mut(), SeqCst);
    }
    pub fn close(&self) {
        // SAFETY: prepare allocated a retained no-op instruction at offset 128.
        self.data.destination.store(unsafe { self.code.as_ptr().add(128) }.cast(), SeqCst);
    }
    pub fn active(&self) -> u64 {
        self.data.active.load(SeqCst)
    }
}

/// Closes the gate, restores the entry and waits for calls already past the gate. Ok means nothing can reach the callback
/// any more. Either can be absent, for an owner that never got as far as making it; failure leaves both retained.
///
/// # Safety
/// The caller owns the target and all generated code is retained, as [`commit`] requires.
pub unsafe fn restore_and_drain(hook: Option<&RetainedHook>, gate: Option<&CallbackGate>) -> Result<(), String> {
    if let Some(gate) = gate {
        gate.close();
    }
    if let Some(hook) = hook
        && hook.state() != HookState::Disabled
    {
        // SAFETY: the caller owns the target; closing the gate prevents new callback calls during restoration.
        unsafe { commit(&[(hook, false)]) }.map_err(|error| error.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while gate.is_some_and(|gate| gate.active() != 0) {
        if Instant::now() >= deadline {
            return Err("Helper callback did not drain; module remains loaded".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};

    #[derive(Default)]
    struct Call {
        entered: AtomicBool,
        release: AtomicBool,
        called: AtomicBool,
    }
    unsafe extern "C-unwind" fn original(call: *const Call) {
        // SAFETY: the test retains its Arc until this thread has joined.
        let call = unsafe { &*call };
        call.entered.store(true, SeqCst);
        while !call.release.load(SeqCst) {
            std::thread::yield_now();
        }
    }
    unsafe extern "C-unwind" fn callback(call: *const Call) {
        // SAFETY: the same live test-owned argument is forwarded by the gateway.
        unsafe { &*call }.called.store(true, SeqCst);
    }

    #[test]
    fn delayed_original_returns_without_entering_a_closed_helper() {
        // SAFETY: both functions have exactly the supported void(pointer) ABI;
        // the test executable retains them and every generated page until exit.
        let gate = unsafe { CallbackGate::prepare(callback as *const ()) }.unwrap();
        // SAFETY: gateway is not published to a calling thread yet.
        unsafe { gate.set_original(original as *const ()) };
        let call = Arc::new(Call::default());
        let entry = gate.entry() as usize;
        let argument = call.clone();
        let thread = std::thread::spawn(move || {
            // SAFETY: prepared gateway implements the same exact ABI, and the Arc
            // keeps its argument alive through the complete generated-code return.
            let entry: unsafe extern "C-unwind" fn(*const Call) = unsafe { std::mem::transmute(entry) };
            // SAFETY: caller owns this live argument and gateway publication.
            unsafe { entry(Arc::as_ptr(&argument)) };
        });
        while !call.entered.load(SeqCst) {
            std::thread::yield_now();
        }
        gate.close();
        assert_eq!(gate.active(), 0);
        call.release.store(true, SeqCst);
        thread.join().unwrap();
        assert!(!call.called.load(SeqCst));
        assert_eq!(gate.active(), 0);
    }

    /// A target of its own, for each test that patches one: straight-line nops the hook can relocate, then a ret. The line
    /// number keeps identical code from being folded into one function by the linker.
    macro_rules! target {
        ($name:ident) => {
            #[unsafe(naked)]
            unsafe extern "C-unwind" fn $name(_: *const Call) {
                core::arch::naked_asm!(".rept 16", "nop", ".endr", "ret", concat!(".long ", line!()));
            }
        };
    }

    #[test]
    fn the_prepared_hook_forwards_to_the_callback_after_the_original_and_restore_ends_that() {
        target!(entry);
        // SAFETY: authored process-lifetime target and callback of the supported void(pointer) ABI; the test owns the entry.
        unsafe {
            let bytes: [u8; 16] = (entry as *const [u8; 16]).read_unaligned();
            let (gate, hook) = CallbackGate::prepare_hook(callback as *const (), NonNull::new(entry as *mut u8).unwrap(), &bytes).unwrap();
            assert_eq!(hook.state(), HookState::Disabled, "preparing publishes nothing");
            let call = Call::default();
            std::hint::black_box(entry)(&call);
            assert!(!call.called.load(SeqCst));
            commit(&[(&hook, true)]).unwrap();
            std::hint::black_box(entry)(&call);
            assert!(call.called.swap(false, SeqCst));
            restore_and_drain(Some(&hook), Some(&gate)).unwrap();
            assert_eq!(hook.state(), HookState::Disabled);
            std::hint::black_box(entry)(&call);
            assert!(!call.called.load(SeqCst));
            restore_and_drain(Some(&hook), Some(&gate)).unwrap();
        }
    }

    #[test]
    fn a_callback_still_running_keeps_the_owner_from_leaving() {
        target!(entry);
        unsafe extern "C-unwind" fn stuck(call: *const Call) {
            // SAFETY: the test retains its Arc until this thread has joined.
            let call = unsafe { &*call };
            call.entered.store(true, SeqCst);
            while !call.release.load(SeqCst) {
                std::thread::yield_now();
            }
        }
        // SAFETY: authored process-lifetime target and callback of the supported void(pointer) ABI; the test owns the entry.
        let (gate, hook) = unsafe {
            let bytes: [u8; 16] = (entry as *const [u8; 16]).read_unaligned();
            CallbackGate::prepare_hook(stuck as *const (), NonNull::new(entry as *mut u8).unwrap(), &bytes).unwrap()
        };
        // SAFETY: the same owned entry, before any caller.
        unsafe { commit(&[(&hook, true)]) }.unwrap();
        let call = Arc::new(Call::default());
        let argument = call.clone();
        // SAFETY: the Arc keeps the argument alive through the call and the entry stays callable, patched or restored.
        let thread = std::thread::spawn(move || unsafe { std::hint::black_box(entry)(Arc::as_ptr(&argument)) });
        while !call.entered.load(SeqCst) {
            std::thread::yield_now();
        }
        // SAFETY: the test owns the target; the caller is parked in the callback, outside the entry's changed bytes.
        let failed = unsafe { restore_and_drain(Some(&hook), Some(&gate)) };
        assert_eq!(failed.unwrap_err(), "Helper callback did not drain; module remains loaded");
        assert_eq!(hook.state(), HookState::Disabled, "the entry was restored before the wait");
        call.release.store(true, SeqCst);
        thread.join().unwrap();
        // SAFETY: as above, with nothing running.
        unsafe { restore_and_drain(Some(&hook), Some(&gate)) }.unwrap();
    }

    #[test]
    fn nothing_to_restore_or_drain_is_success() {
        // SAFETY: nothing to change.
        unsafe { restore_and_drain(None, None) }.unwrap();
    }
}
