//! Registers the copied prefix and stock near jump using the original function's
//! unwind codes. The stock jump-back is replaced with the old adapter's two
//! conditional near jumps, preserving flags/registers without epilogue detection.
//! Relocated instruction offsets must remain unchanged. Handler/
//! chained records are refused rather than fabricating exception-handler coverage.
use super::{HookError, failure};
use iced_x86::{Decoder, DecoderOptions};
use windows_sys::Win32::System::{
    Diagnostics::Debug::{FlushInstructionCache, IMAGE_RUNTIME_FUNCTION_ENTRY, RtlAddFunctionTable, RtlLookupFunctionEntry},
    Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect},
    Threading::GetCurrentProcess,
};

pub(super) unsafe fn register(target: *const u8, trampoline: *const u8, expected: &[u8], copied: usize) -> Result<(), HookError> {
    // SAFETY: caller owns a retained detour; pinned x64 backend reserves at least copied+5 executable bytes and emits a near jump after straight-line instructions.
    unsafe {
        let generated = std::slice::from_raw_parts(trampoline, copied + 5);
        let mut original = Decoder::with_ip(64, &expected[..copied], target as u64, DecoderOptions::NONE);
        let mut relocated = Decoder::with_ip(64, generated, trampoline as u64, DecoderOptions::NONE);
        while original.can_decode() {
            let from = original.decode();
            let to = relocated.decode();
            if from.code() != to.code() || from.len() != to.len() {
                return Err(HookError::UnsupportedUnwind);
            }
        }
        let jump = relocated.decode();
        if jump.code() != iced_x86::Code::Jmp_rel32_64 || jump.near_branch_target() != target as u64 + copied as u64 {
            return Err(HookError::UnsupportedUnwind);
        }
        let mut image_base = 0;
        let entry = RtlLookupFunctionEntry(target as u64, &raw mut image_base, std::ptr::null_mut());
        let mut info = vec![1, 0, 0, 0];
        if !entry.is_null() {
            if image_base + u64::from((*entry).BeginAddress) != target as u64 {
                return Err(HookError::UnsupportedUnwind);
            }
            let address = (image_base + u64::from((*entry).Anonymous.UnwindData)) as *const u8;
            let header = std::slice::from_raw_parts(address, 4);
            if header[0] != 1 || header[2] > 64 {
                return Err(HookError::UnsupportedUnwind);
            }
            info[1] = header[1].min(copied as u8);
            let codes = std::slice::from_raw_parts(address.add(4), usize::from(header[2]) * 2);
            let mut index = 0;
            while index < codes.len() {
                let opcode = codes[index + 1] & 15;
                let operand = codes[index + 1] >> 4;
                let slots = match opcode {
                    0 | 2 | 3 => 1,
                    1 if operand == 0 => 2,
                    1 if operand == 1 => 3,
                    4 | 8 => 2,
                    5 | 9 => 3,
                    _ => return Err(HookError::UnsupportedUnwind),
                };
                let code = codes.get(index..index + slots * 2).ok_or(HookError::UnsupportedUnwind)?;
                if usize::from(code[0]) <= copied {
                    info.extend_from_slice(code);
                    if opcode == 3 {
                        info[3] = header[3];
                    }
                }
                index += slots * 2;
            }
            info[2] = ((info.len() - 4) / 2) as u8;
        }
        if info.len() % 4 != 0 {
            info.extend_from_slice(&[0, 0]);
        }
        // Pinned detour x64 reserves copied + 24 * (instruction count + 1)
        // bytes. Reuse the original adapter's non-epilogue jump-back within that
        // unpublished reservation; instruction offsets and relocation stay stock.
        let mut tail = [0_u8; 12];
        for (offset, opcode) in [(0, 0x84), (6, 0x85)] {
            tail[offset..offset + 2].copy_from_slice(&[0x0f, opcode]);
            let displacement =
                i32::try_from(target as i128 + copied as i128 - (trampoline as i128 + copied as i128 + offset as i128 + 6)).map_err(|_| HookError::UnsupportedUnwind)?;
            tail[offset + 2..offset + 6].copy_from_slice(&displacement.to_le_bytes());
        }
        let destination = trampoline.add(copied).cast_mut();
        let mut protection = 0;
        if VirtualProtect(destination.cast(), 12, PAGE_EXECUTE_READWRITE, &raw mut protection) == 0 {
            return Err(failure("trampoline tail protection"));
        }
        std::ptr::copy_nonoverlapping(tail.as_ptr(), destination, 12);
        let flushed = FlushInstructionCache(GetCurrentProcess(), destination.cast(), 12);
        let mut ignored = 0;
        let restored = VirtualProtect(destination.cast(), 12, protection, &raw mut ignored);
        if flushed == 0 || restored == 0 {
            return Err(failure("trampoline tail cache/protection"));
        }
        // Metadata is data only. It must be within a DWORD RVA of both code and
        // the registered table base; the detour still owns all executable bytes.
        let aligned = trampoline as usize & !0xffff;
        for index in 1..32768usize {
            for address in [aligned.checked_add(index * 0x10000), aligned.checked_sub(index * 0x10000)].into_iter().flatten() {
                let allocation = VirtualAlloc(address as *const _, 4096, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).cast::<u8>();
                if allocation.is_null() {
                    continue;
                }
                let base = (allocation as usize).min(trampoline as usize);
                let table = allocation.cast::<IMAGE_RUNTIME_FUNCTION_ENTRY>();
                table.write(IMAGE_RUNTIME_FUNCTION_ENTRY {
                    BeginAddress: (trampoline as usize - base) as u32,
                    EndAddress: (trampoline as usize - base + copied + 12) as u32,
                    Anonymous: std::mem::zeroed(),
                });
                (*table).Anonymous.UnwindData = (allocation as usize + 16 - base) as u32;
                std::ptr::copy_nonoverlapping(info.as_ptr(), allocation.add(16), info.len());
                if !RtlAddFunctionTable(table, 1, base as u64) {
                    let error = failure("register trampoline unwind");
                    VirtualFree(allocation.cast(), 0, MEM_RELEASE);
                    return Err(error);
                }
                // Both function table and referenced unwind bytes stay registered
                // until process exit. RetainedHook deliberately has no cleanup.
                return Ok(());
            }
        }
        Err(failure("allocate trampoline unwind data"))
    }
}
