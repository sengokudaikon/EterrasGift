//! Original-first Windows x64 float-result gateway with three stack argument positions. The original call, result
//! adjustment, return path, state and unwind data remain process-owned. Generated code never enters this DLL.
use super::{HookError, failure};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use windows_sys::Win32::System::{
    Diagnostics::Debug::{FlushInstructionCache, IMAGE_RUNTIME_FUNCTION_ENTRY, RtlAddFunctionTable},
    Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect},
    Threading::GetCurrentProcess,
};

#[repr(C)]
struct Data {
    enabled: AtomicU64,
    original: AtomicU64,
    value: AtomicU64,
    switch: u64,
    slots: [u64; 2],
}

/// Byte displacements from RAX for the feature's post-call code.
pub struct Frame {
    pub switch: u8,
    pub value: u8,
    pub slots: [u8; 2],
}
pub const FRAME: Frame = Frame {
    switch: std::mem::offset_of!(Data, switch) as u8,
    value: std::mem::offset_of!(Data, value) as u8,
    slots: [std::mem::offset_of!(Data, slots) as u8, (std::mem::offset_of!(Data, slots) + 8) as u8],
};

/// Code after a normal original return: RAX is Data, RCX is the saved receiver and XMM0 holds the float result.
/// Use only volatile registers, preserve RSP and nonvolatile registers, keep every branch inside this code (or to its
/// end), and fall through to the appended epilogue. There are no helper-code addresses or callback destinations.
pub struct PostfixEffect {
    pub code: Vec<u8>,
    pub slots: [u64; 2],
}

pub struct PostfixGate {
    entry: usize,
    data: &'static Data,
    switch: &'static AtomicU64,
}
impl PostfixGate {
    /// # Safety
    /// Windows x64 instance method returning System.Single, seven ABI positions including receiver and MethodInfo,
    /// and no hidden return buffer. Effect code obeys PostfixEffect's register/branch contract. Its referenced static
    /// storage remains readable for process lifetime; any object graph it reads is valid over the post-call span.
    pub unsafe fn prepare(effect: &PostfixEffect, shared: Option<&Self>) -> Result<Self, HookError> {
        // SAFETY: private pages initialized while unpublished; executable/data/unwind pages are retained until exit.
        unsafe {
            let allocation = VirtualAlloc(std::ptr::null(), 8192, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE).cast::<u8>();
            if allocation.is_null() {
                return Err(failure("allocate postfix gateway"));
            }
            let data = allocation.add(4096).cast::<Data>();
            data.write(Data {
                enabled: AtomicU64::new(0),
                original: AtomicU64::new(0),
                value: AtomicU64::new(1),
                switch: shared.map_or(data as u64, |gate| gate.switch as *const AtomicU64 as u64),
                slots: effect.slots,
            });
            // 32-byte home area, three 8-byte stack arguments, saved RCX at +38, padding: 72-byte aligned call frame.
            let mut code = vec![0x48, 0x83, 0xec, 0x48, 0x48, 0x89, 0x4c, 0x24, 0x38];
            for (source, destination) in [(0x70u32, 0x20u8), (0x78, 0x28), (0x80, 0x30)] {
                code.extend([0x48, 0x8b, 0x84, 0x24]); // mov rax,[rsp+source]
                code.extend(source.to_le_bytes());
                code.extend([0x48, 0x89, 0x44, 0x24, destination]);
            }
            code.extend([0x48, 0xb8]);
            code.extend((data as u64).to_le_bytes());
            code.extend([0xff, 0x50, std::mem::offset_of!(Data, original) as u8]); // call original once
            code.extend([0x48, 0x8b, 0x4c, 0x24, 0x38, 0x48, 0xb8]); // restore RCX; reload Data
            code.extend((data as u64).to_le_bytes());
            code.extend(&effect.code);
            code.extend([0x48, 0x83, 0xc4, 0x48, 0xc3]);
            if code.len() > 4096 {
                VirtualFree(allocation.cast(), 0, MEM_RELEASE);
                return Err(HookError::UnsupportedPrefix);
            }
            std::ptr::copy_nonoverlapping(code.as_ptr(), allocation, code.len());
            let table = allocation.add(4096 + 64).cast::<IMAGE_RUNTIME_FUNCTION_ENTRY>();
            table.write(IMAGE_RUNTIME_FUNCTION_ENTRY {
                BeginAddress: 0,
                EndAddress: code.len() as u32,
                Anonymous: std::mem::zeroed(),
            });
            (*table).Anonymous.UnwindData = 4096 + 80;
            // Version 1, prologue 4, one UWOP_ALLOC_SMALL(72). The original can unwind without helper frames/state.
            std::ptr::copy_nonoverlapping([1u8, 4, 1, 0, 4, 0x82, 0, 0].as_ptr(), allocation.add(4096 + 80), 8);
            let mut old = 0;
            if VirtualProtect(allocation.cast(), 4096, PAGE_EXECUTE_READ, &raw mut old) == 0
                || FlushInstructionCache(GetCurrentProcess(), allocation.cast(), 4096) == 0
                || !RtlAddFunctionTable(table, 1, allocation as u64)
            {
                let error = failure("publish postfix gateway");
                VirtualFree(allocation.cast(), 0, MEM_RELEASE);
                return Err(error);
            }
            Ok(Self {
                entry: allocation as usize,
                data: &*data,
                switch: &*((*data).switch as *const AtomicU64),
            })
        }
    }
    pub fn entry(&self) -> *const () {
        self.entry as *const ()
    }
    /// # Safety
    /// Original has this gateway's exact seven-position ABI and remains process-owned; set once before publication.
    pub unsafe fn set_original(&self, original: *const ()) {
        self.data.original.store(original as u64, SeqCst);
    }
    pub fn enabled(&self) -> bool {
        self.switch.load(SeqCst) != 0
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.switch.store(u64::from(enabled), SeqCst);
    }
    pub fn value(&self) -> u64 {
        self.data.value.load(SeqCst)
    }
    pub fn set_value(&self, value: u64) {
        self.data.value.store(value, SeqCst);
    }
}
