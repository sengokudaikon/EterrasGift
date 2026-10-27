//! Process-owned leaf gateway: return a constant, or tail-call the retained original.
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
    receiver_class: u64,
    switch: u64,
    answer: AtomicU64,
}

#[derive(Clone, Copy)]
pub enum LeafReturn {
    Boolean(bool),
    SelectableBoolean,
    Void,
}

/// Pages intentionally survive helper unload, including calls paused in this leaf.
pub struct ConstantGate {
    entry: usize,
    data: &'static Data,
    switch: &'static AtomicU64,
}
impl ConstantGate {
    pub fn prepare() -> Result<Self, HookError> {
        // SAFETY: unscoped gate never reads a receiver.
        unsafe { Self::prepare_override(LeafReturn::Boolean(true), None, None) }
    }
    /// Override only calls on this exact IL2CPP class. Other receivers forward.
    ///
    /// # Safety
    /// The class descriptor is immutable and process-lifetime. Every non-null
    /// receiver passed in RCX has a readable object header whose first word is
    /// its class pointer. No other object fields are read by the gateway.
    pub unsafe fn prepare_for_class(class: NonNull<std::ffi::c_void>) -> Result<Self, HookError> {
        // SAFETY: caller supplies the retained metadata and object-header contract.
        unsafe { Self::prepare_override(LeafReturn::Boolean(true), Some(class), None) }
    }
    /// # Safety
    /// The target's return type must match `answer`. With a class guard, every
    /// non-null receiver has a readable IL2CPP header. If `linked_offset` is set,
    /// it names a metadata-validated reference field on that receiver; a non-null
    /// linked object also has a readable header. Class metadata is process-lifetime.
    /// The linked guard admits either the selected class or a slot linked to it.
    pub unsafe fn prepare_override(answer: LeafReturn, class: Option<NonNull<std::ffi::c_void>>, linked_offset: Option<u32>) -> Result<Self, HookError> {
        // SAFETY: forwarded target/receiver contract, with a new private switch.
        unsafe { Self::prepare_shared(answer, class, linked_offset, None) }
    }
    /// Prepare another leaf using an existing process-owned enable switch.
    ///
    /// # Safety
    /// Same target and receiver contract as `prepare_override`. Neither leaf may
    /// be published until every original has been set and all entries verified.
    pub unsafe fn prepare_shared(answer: LeafReturn, class: Option<NonNull<std::ffi::c_void>>, linked_offset: Option<u32>, shared: Option<&Self>) -> Result<Self, HookError> {
        // SAFETY: private allocation; code and aligned atomic data fit the two pages.
        unsafe {
            let allocation = VirtualAlloc(std::ptr::null(), 8192, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE).cast::<u8>();
            let Some(code) = NonNull::new(allocation) else {
                return Err(failure("allocate constant gateway"));
            };
            let data = allocation.add(4096).cast::<Data>();
            data.write(Data {
                enabled: AtomicU64::new(0),
                original: AtomicU64::new(0),
                answer: AtomicU64::new(1),
                receiver_class: class.map_or(0, |class| class.as_ptr() as u64),
                switch: shared.map_or(data as u64, |gate| gate.switch as *const AtomicU64 as u64),
            });
            // Only volatile RAX/R11 and flags change; all arguments stay intact.
            let mut bytes = vec![0x48, 0xb8];
            bytes.extend((data as u64).to_le_bytes());
            bytes.extend([0x4c, 0x8b, 0x58, 0x18, 0x49, 0x83, 0x3b, 0, 0x74, 0]); // mov r11,[rax+24]; cmp [r11],0; je original
            let mut branches = vec![bytes.len() - 1];
            if class.is_some() {
                bytes.extend([0x48, 0x85, 0xc9, 0x74, 0]); // test rcx,rcx; je original
                branches.push(bytes.len() - 1);
                bytes.extend([0x4c, 0x8b, 0x19, 0x4c, 0x3b, 0x58, 0x10]); // mov r11,[rcx]; cmp r11,[rax+16]
                if let Some(offset) = linked_offset {
                    bytes.extend([0x74, 21]); // matching receiver: jump to constant return
                    bytes.extend([0x4c, 0x8b, 0x99]); // mov r11,[rcx+offset]
                    bytes.extend(offset.to_le_bytes());
                    bytes.extend([0x4d, 0x85, 0xdb, 0x74, 0]); // test r11,r11; je original
                    branches.push(bytes.len() - 1);
                    bytes.extend([0x4d, 0x8b, 0x1b, 0x4c, 0x3b, 0x58, 0x10]); // mov r11,[r11]; cmp r11,[rax+16]
                }
                bytes.extend([0x75, 0]); // jne original
                branches.push(bytes.len() - 1);
            }
            if matches!(answer, LeafReturn::SelectableBoolean) {
                bytes.extend([0x48, 0x8b, 0x40, 0x20, 0xc3]); // mov rax,[rax+32]; ret
            } else {
                let answer = matches!(answer, LeafReturn::Boolean(true));
                bytes.extend([0xb8, u8::from(answer), 0, 0, 0, 0xc3]); // mov eax,answer; ret (also valid for void)
            }
            for branch in branches {
                bytes[branch] = (bytes.len() - branch - 1) as u8;
            }
            bytes.extend([0xff, 0x60, 0x08]); // original: jmp [rax+8]
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation, bytes.len());
            let mut old = 0;
            if VirtualProtect(allocation.cast(), 4096, PAGE_EXECUTE_READ, &raw mut old) == 0 || FlushInstructionCache(GetCurrentProcess(), allocation.cast(), 4096) == 0 {
                let error = failure("publish constant gateway");
                VirtualFree(allocation.cast(), 0, MEM_RELEASE);
                return Err(error);
            }
            Ok(Self {
                entry: code.as_ptr() as usize,
                data: &*data,
                switch: &*((*data).switch as *const AtomicU64),
            })
        }
    }
    pub fn entry(&self) -> *const () {
        self.entry as *const ()
    }
    /// # Safety
    /// Original is a process-lifetime Windows x64 function with the selected return type.
    /// Set it before publishing entry; never change it while callers can enter.
    pub unsafe fn set_original(&self, original: *const ()) {
        self.data.original.store(original as u64, SeqCst);
    }
    /// Used by a selectable Boolean leaf; fixed leaves retain their authored answer.
    pub fn set_answer(&self, answer: bool) {
        self.data.answer.store(u64::from(answer), SeqCst);
    }
    pub fn answer(&self) -> bool {
        self.data.answer.load(SeqCst) != 0
    }
    pub fn set_enabled(&self, on: bool) {
        self.switch.store(u64::from(on), SeqCst);
    }
    pub fn enabled(&self) -> bool {
        self.switch.load(SeqCst) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    #[test]
    fn selectable_answer_changes_without_replacing_code_and_disabled_always_forwards() {
        extern "C-unwind" fn original(calls: &AtomicUsize, answer: bool) -> bool {
            calls.fetch_add(1, SeqCst);
            answer
        }
        // SAFETY: unscoped Boolean leaf with a matching process-lifetime original.
        let gate = unsafe { ConstantGate::prepare_override(LeafReturn::SelectableBoolean, None, None).unwrap() };
        // SAFETY: configured before publication.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: exact original ABI; retained leaf preserves the arguments.
        let call: extern "C-unwind" fn(&AtomicUsize, bool) -> bool = unsafe { std::mem::transmute(gate.entry()) };
        let calls = AtomicUsize::new(0);
        for answer in [true, false, true, false] {
            gate.set_answer(answer);
            gate.set_enabled(true);
            assert_eq!(call(&calls, !answer), answer);
            assert_eq!(calls.load(SeqCst), 0, "false must override rather than call the original");
        }
        gate.set_enabled(false);
        assert!(call(&calls, true));
        assert!(!call(&calls, false));
        assert_eq!(calls.load(SeqCst), 2);
    }
    #[test]
    fn one_switch_controls_all_leaves_after_the_owner_is_dropped() {
        extern "C-unwind" fn original() -> bool {
            false
        }
        let (a, b, second) = {
            let first = ConstantGate::prepare().unwrap();
            // SAFETY: same Boolean ABI; shared switch and executable pages are process-owned.
            let second = unsafe { ConstantGate::prepare_shared(LeafReturn::Boolean(true), None, None, Some(&first)).unwrap() };
            // SAFETY: matching originals, configured before either entry is used.
            unsafe {
                first.set_original(original as *const ());
                second.set_original(original as *const ());
            }
            // SAFETY: first leaf preserves this Boolean ABI.
            let a: extern "C-unwind" fn() -> bool = unsafe { std::mem::transmute(first.entry()) };
            // SAFETY: second leaf preserves this Boolean ABI.
            let b: extern "C-unwind" fn() -> bool = unsafe { std::mem::transmute(second.entry()) };
            first.set_enabled(true);
            assert!(a() && b());
            first.set_enabled(false);
            (a, b, second)
        };
        assert!(!a() && !b());
        second.set_enabled(true);
        assert!(a() && b());
    }
    #[test]
    fn false_override_covers_both_link_directions_but_not_unrelated_slots() {
        #[repr(C)]
        struct Slot {
            class: usize,
            linked: *const Slot,
        }
        extern "C-unwind" fn original(_: *const Slot, item: usize) -> bool {
            assert_eq!(item, 42);
            true
        }
        static CLASS: u8 = 0;
        let class = NonNull::from(&CLASS).cast::<std::ffi::c_void>();
        let offhand = Slot {
            class: class.as_ptr() as usize,
            linked: std::ptr::null(),
        };
        let mainhand = Slot { class: 0, linked: &offhand };
        let unrelated = Slot { class: 0, linked: &mainhand };
        let unlinked = Slot {
            class: 0,
            linked: std::ptr::null(),
        };
        // SAFETY: authored readable headers/references; exact field offset and process-lifetime class.
        let gate = unsafe { ConstantGate::prepare_override(LeafReturn::Boolean(false), Some(class), Some(8)).unwrap() };
        // SAFETY: matching process-lifetime Boolean target.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: leaf forwards this ABI exactly.
        let call: extern "C-unwind" fn(*const Slot, usize) -> bool = unsafe { std::mem::transmute(gate.entry()) };
        gate.set_enabled(true);
        assert!(!call(&offhand, 0));
        assert!(!call(&mainhand, 0));
        assert!(call(&unrelated, 42));
        assert!(call(&unlinked, 42));
        assert!(call(std::ptr::null(), 42));
        gate.set_enabled(false);
        assert!(call(&offhand, 42));
        assert!(call(&mainhand, 42));
    }

    #[test]
    fn void_override_skips_work_and_restores_arguments_and_unwind() {
        extern "C-unwind" fn original(count: &AtomicUsize, a: usize, b: usize, c: usize, d: usize) {
            assert_eq!((a, b, c, d), (1, 2, 3, 4));
            count.fetch_add(1, SeqCst);
        }
        // SAFETY: no receiver guard; matching void target below.
        let gate = unsafe { ConstantGate::prepare_override(LeafReturn::Void, None, None).unwrap() };
        // SAFETY: process-lifetime original, configured before publication.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: exact target ABI, including a stack argument.
        let call: extern "C-unwind" fn(&AtomicUsize, usize, usize, usize, usize) = unsafe { std::mem::transmute(gate.entry()) };
        let count = AtomicUsize::new(0);
        gate.set_enabled(true);
        call(&count, 0, 0, 0, 0);
        assert_eq!(count.load(SeqCst), 0);
        gate.set_enabled(false);
        call(&count, 1, 2, 3, 4);
        assert_eq!(count.load(SeqCst), 1);
        assert!(std::panic::catch_unwind(|| call(&count, 0, 0, 0, 0)).is_err());
    }
    extern "C-unwind" fn original(a: usize, b: usize, c: i32, d: usize, e: usize, f: usize) -> bool {
        assert_eq!((a, b, c, d, e, f), (11, 22, -33, 44, 55, 66));
        CALLS.fetch_add(1, SeqCst);
        false
    }
    #[test]
    fn bypasses_original_on_and_preserves_register_and_stack_arguments_off() {
        let gate = ConstantGate::prepare().unwrap();
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

    #[test]
    fn scoped_override_preserves_other_receivers_arguments_and_unwinds() {
        #[repr(C)]
        struct Object {
            class: usize,
        }
        extern "C-unwind" fn original(this: *const Object, kind: i32, c: usize, d: usize, e: usize, f: usize) -> bool {
            assert_eq!((kind, c, d, e, f), (-17, 22, 33, 44, 55));
            this.is_null()
        }
        static CLASS: u8 = 0;
        let class = (&CLASS as *const u8).cast_mut().cast::<std::ffi::c_void>();
        let selected = Object { class: class as usize };
        let other = Object { class: 0 };
        // SAFETY: class identity is compared only; all non-null test receivers
        // have this readable header and outlive calls. The gate outlives the test.
        let gate = unsafe { ConstantGate::prepare_for_class(NonNull::new(class).unwrap()).unwrap() };
        // SAFETY: process-lifetime original with the same ABI as the leaf.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: the leaf preserves register/stack arguments and returns bool.
        let call: extern "C-unwind" fn(*const Object, i32, usize, usize, usize, usize) -> bool = unsafe { std::mem::transmute(gate.entry()) };
        assert!(!call(&selected, -17, 22, 33, 44, 55));
        gate.set_enabled(true);
        assert!(call(&selected, 0, 0, 0, 0, 0));
        assert!(!call(&other, -17, 22, 33, 44, 55));
        assert!(call(std::ptr::null(), -17, 22, 33, 44, 55));
        assert!(std::panic::catch_unwind(|| call(&other, 0, 0, 0, 0, 0)).is_err());
        gate.set_enabled(false);
        assert!(!call(&selected, -17, 22, 33, 44, 55));
    }
}
