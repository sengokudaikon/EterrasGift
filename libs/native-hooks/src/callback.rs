//! Retained x64 ExecuteTasks gateway. Original execution and the return path live
//! outside the owning module's image. Closing the destination fences all future module
//! entries; the count covers every call that could have read the old destination.
//!
//! Native unwinding out of the post-call callback deliberately strands its count:
//! unloading then refuses, rather than guessing whether DLL frames remain alive.
use super::{HookError, page};
use std::{
    ptr::NonNull,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering::SeqCst},
};

#[repr(C)]
struct Data {
    destination: AtomicPtr<()>,
    active: AtomicU64,
    original: AtomicPtr<()>,
}
const _: () = assert!(std::mem::offset_of!(Data, active) == 8 && std::mem::offset_of!(Data, original) == 16);

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
}
