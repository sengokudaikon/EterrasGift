//! Original-first Windows x64 scalar-result gateway with up to twelve argument positions. The original call, result
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

/// Code after a normal original return: RAX is Data, RCX is the saved first argument, R10 holds the integer result
/// and XMM0 holds the float result. Preserve the relevant result on unchanged paths; R10 is copied back to RAX.
/// Use only volatile registers, preserve RSP and nonvolatile registers, keep every branch inside this code (or to its
/// end), and fall through to the appended epilogue. There are no helper-code addresses or callback destinations.
pub struct PostfixEffect {
    pub code: Vec<u8>,
    pub slots: [u64; 2],
    /// Native positions, including receiver when present and MethodInfo. No hidden return buffer.
    pub positions: u8,
}

pub struct PostfixGate {
    entry: usize,
    data: &'static Data,
    switch: &'static AtomicU64,
}
impl PostfixGate {
    /// # Safety
    /// Windows x64 method returning an integer, byte or float scalar, with `positions` argument positions including
    /// receiver when present and MethodInfo, and no hidden return buffer. Each parameter occupies one native position.
    /// Effect code obeys PostfixEffect's register/branch contract. Its referenced static
    /// storage remains readable for process lifetime; any object graph it reads is valid over the post-call span.
    pub unsafe fn prepare(effect: &PostfixEffect, shared: Option<&Self>) -> Result<Self, HookError> {
        if !(1..=12).contains(&effect.positions) {
            return Err(HookError::UnsupportedPrefix);
        }
        let stack = effect.positions.saturating_sub(4);
        let saved = 32 + stack * 8;
        let frame = ((saved + 8 + 23) & !15) - 8; // enough storage, with RSP aligned before the call
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
            // Home area, the actual stack arguments, then saved RCX. Never read absent caller arguments.
            let mut code = vec![0x48, 0x83, 0xec, frame, 0x48, 0x89, 0x4c, 0x24, saved];
            for index in 0..stack {
                let source = u32::from(frame) + 40 + u32::from(index) * 8;
                let destination = 32 + index * 8;
                code.extend([0x48, 0x8b, 0x84, 0x24]); // mov rax,[rsp+source]
                code.extend(source.to_le_bytes());
                code.extend([0x48, 0x89, 0x44, 0x24, destination]);
            }
            code.extend([0x48, 0xb8]);
            code.extend((data as u64).to_le_bytes());
            code.extend([0xff, 0x50, std::mem::offset_of!(Data, original) as u8]); // call original once
            code.extend([0x49, 0x89, 0xc2]); // integer result -> r10 before reloading Data into rax
            code.extend([0x48, 0x8b, 0x4c, 0x24, saved, 0x48, 0xb8]); // restore RCX; reload Data
            code.extend((data as u64).to_le_bytes());
            code.extend(&effect.code);
            code.extend([0x4c, 0x89, 0xd0, 0x48, 0x83, 0xc4, frame, 0xc3]);
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
            // Version 1, prologue 4, one UWOP_ALLOC_SMALL(frame). No helper frame/state on the unwind path.
            let allocation_code = ((frame - 8) / 8) << 4 | 2;
            std::ptr::copy_nonoverlapping([1u8, 4, 1, 0, 4, allocation_code, 0, 0].as_ptr(), allocation.add(4096 + 80), 8);
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
    /// Original has this gateway's exact declared ABI and remains process-owned; set once before publication.
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
