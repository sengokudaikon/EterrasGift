//! Process-owned leaf gateway: return true, or tail-call the retained original.
//! It never calls helper code, changes arguments/stack, or creates an unwind frame.
use super::{HookError, failure};
use std::{
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering::SeqCst},
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::FlushInstructionCache,
    Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect},
    Threading::GetCurrentProcess,
};

#[repr(C)]
struct Data {
    enabled: AtomicU64,
    original: AtomicU64,
}

/// Pages intentionally survive helper unload, including calls paused in this leaf.
pub struct BooleanGate {
    entry: usize,
    data: &'static Data,
}
impl BooleanGate {
    pub fn prepare() -> Result<Self, HookError> {
        // SAFETY: private allocation; code and aligned atomic data fit the two pages.
        unsafe {
            let allocation = VirtualAlloc(std::ptr::null(), 8192, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE).cast::<u8>();
            let Some(code) = NonNull::new(allocation) else {
                return Err(failure("allocate Boolean gateway"));
            };
            let data = allocation.add(4096).cast::<Data>();
            data.write(Data {
                enabled: AtomicU64::new(0),
                original: AtomicU64::new(0),
            });
            // mov rax,data; cmp qword [rax],0; je original; mov eax,1; ret;
            // original: jmp qword [rax+8]. Only the volatile return register changes.
            let mut bytes = vec![0x48, 0xb8];
            bytes.extend((data as u64).to_le_bytes());
            bytes.extend([0x48, 0x83, 0x38, 0x00, 0x74, 0x06, 0xb8, 1, 0, 0, 0, 0xc3, 0xff, 0x60, 0x08]);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation, bytes.len());
            let mut old = 0;
            if VirtualProtect(allocation.cast(), 4096, PAGE_EXECUTE_READ, &raw mut old) == 0 || FlushInstructionCache(GetCurrentProcess(), allocation.cast(), 4096) == 0 {
                let error = failure("publish Boolean gateway");
                VirtualFree(allocation.cast(), 0, MEM_RELEASE);
                return Err(error);
            }
            Ok(Self {
                entry: code.as_ptr() as usize,
                data: &*data,
            })
        }
    }
    pub fn entry(&self) -> *const () {
        self.entry as *const ()
    }
    /// # Safety
    /// Original is a process-lifetime Windows x64 Boolean-returning function.
    /// Set it before publishing entry; never change it while callers can enter.
    pub unsafe fn set_original(&self, original: *const ()) {
        self.data.original.store(original as u64, SeqCst);
    }
    pub fn set_enabled(&self, on: bool) {
        self.data.enabled.store(u64::from(on), SeqCst);
    }
    pub fn enabled(&self) -> bool {
        self.data.enabled.load(SeqCst) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    extern "C-unwind" fn original(a: usize, b: usize, c: i32, d: usize, e: usize, f: usize) -> bool {
        assert_eq!((a, b, c, d, e, f), (11, 22, -33, 44, 55, 66));
        CALLS.fetch_add(1, SeqCst);
        false
    }
    #[test]
    fn bypasses_original_on_and_preserves_register_and_stack_arguments_off() {
        let gate = BooleanGate::prepare().unwrap();
        // SAFETY: unpublished leaf and process-lifetime test target with matching ABI.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: leaf preserves this exact ABI when forwarding and returns a Boolean.
        let call: extern "C-unwind" fn(usize, usize, i32, usize, usize, usize) -> bool = unsafe { std::mem::transmute(gate.entry()) };
        assert!(!call(11, 22, -33, 44, 55, 66));
        gate.set_enabled(true);
        assert!(call(0, 0, 0, 0, 0, 0));
        assert_eq!(CALLS.load(SeqCst), 1);
        gate.set_enabled(false);
        assert!(!call(11, 22, -33, 44, 55, 66));
        assert_eq!(CALLS.load(SeqCst), 2);
        // The disabled leaf tail-jumps: an unwind from the original reaches its
        // caller without a generated stack frame or helper panic boundary.
        assert!(std::panic::catch_unwind(|| call(0, 0, 0, 0, 0, 0)).is_err());
    }
}
