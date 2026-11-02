//! Invocation-local four-byte field snapshots, restored on normal return and Windows native unwind. The caller
//! serializes access to each receiver; recursion has its own stack record. No DLL callback runs during cleanup.
use super::*;
use crate::emit::Code;

pub struct ScopedEffect {
    pub effect: PostfixEffect,
    pub fields: Vec<u32>,
}

/// Stack offsets used by a scoped effect's before-code. Call `save` after admission, before its first field write.
pub struct Scope {
    receiver: u32,
    values: u32,
    frame: u8,
}
impl Scope {
    pub fn new(positions: u8, fields: usize) -> Self {
        assert!((1..=4).contains(&positions) && (1..=4).contains(&fields));
        let receiver = 40;
        let values = receiver + 8;
        let frame = ((values + fields as u32 * 4 + 15) & !15) + 8;
        Self {
            receiver,
            values,
            frame: frame as u8,
        }
    }
    /// RAX is gateway data; RCX is the nonnull retained receiver. Scratch R11 only, preserving all arguments.
    pub fn save(&self, code: &mut Code, fields: &[u32]) {
        code.offset(&[0x48, 0x89, 0x8c, 0x24], self.receiver);
        for (index, field) in fields.iter().enumerate() {
            code.offset(&[0x44, 0x8b, 0x99], *field);
            code.offset(&[0x44, 0x89, 0x9c, 0x24], self.values + index as u32 * 4);
        }
    }
    fn restore(&self, fields: &[u32], unwind: bool) -> Vec<u8> {
        let mut c = Code::default();
        c.offset(if unwind { &[0x48, 0x8b, 0x8a] } else { &[0x48, 0x8b, 0x8c, 0x24] }, self.receiver);
        c.nonnull(&[0x48, 0x85, 0xc9]);
        for (index, field) in fields.iter().enumerate() {
            c.offset(if unwind { &[0x44, 0x8b, 0x9a] } else { &[0x44, 0x8b, 0x9c, 0x24] }, self.values + index as u32 * 4);
            c.offset(&[0x44, 0x89, 0x99], *field);
        }
        c.finish()
    }
}

impl PostfixGate {
    /// # Safety
    /// As `prepare`, but one to four native argument positions and one to four valid aligned four-byte instance
    /// fields. Before-code calls `Scope::save` after admission and before writing. The game retains and serializes
    /// the receiver through normal return or unwind. No field reference escapes the original call.
    pub unsafe fn prepare_scoped(scoped: &ScopedEffect, shared: Option<&Self>) -> Result<Self, HookError> {
        let effect = &scoped.effect;
        if !(1..=4).contains(&effect.positions) || !(1..=4).contains(&scoped.fields.len()) {
            return Err(HookError::UnsupportedPrefix);
        }
        let scope = Scope::new(effect.positions, scoped.fields.len());
        // SAFETY: private pages and exact ABI/storage contracts established by the caller.
        unsafe {
            let allocation = page::allocate("allocate scoped gateway")?;
            let data = allocation.add(page::PAGE).cast::<Data>();
            data.write(Data {
                enabled: AtomicU64::new(0),
                original: AtomicU64::new(0),
                value: AtomicU64::new(1),
                switch: shared.map_or(data as u64, |gate| gate.switch as *const AtomicU64 as u64),
                slots: effect.slots.map(AtomicU64::new),
                wide: super::super::settings::Wide::new(),
            });
            let mut c = Code::default();
            c.emit(&[0x48, 0x83, 0xec, scope.frame]);
            c.emit(&[0x48, 0x89, 0x4c, 0x24, 32, 0x48, 0x89, 0x54, 0x24, scope.frame + 16]);
            c.offset(&[0x48, 0xc7, 0x84, 0x24], scope.receiver);
            c.emit(&[0; 4]);
            c.emit(&[0x48, 0xb8]);
            c.emit(&(data as u64).to_le_bytes());
            c.emit(&effect.before);
            c.emit(&[0xff, 0x50, std::mem::offset_of!(Data, original) as u8, 0x49, 0x89, 0xc2]);
            c.emit(&[0x48, 0x8b, 0x4c, 0x24, 32, 0x48, 0x8b, 0x54, 0x24, scope.frame + 16, 0x48, 0xb8]);
            c.emit(&(data as u64).to_le_bytes());
            c.emit(&effect.code);
            c.emit(&scope.restore(&scoped.fields, false));
            c.emit(&[0x4c, 0x89, 0xd0, 0x48, 0x83, 0xc4, scope.frame, 0xc3]);
            let end = c.bytes.len();
            // Windows language handler: RCX=EXCEPTION_RECORD, RDX=EstablisherFrame (our body RSP).
            // Cleanup only during unwind, never during first-pass exception search. Return ContinueSearch (1).
            c.emit(&[0xf7, 0x41, 4, 0x66, 0, 0, 0]);
            let search = c.branch(0x84);
            c.emit(&scope.restore(&scoped.fields, true));
            c.bind(search);
            c.emit(&[0xb8, 1, 0, 0, 0, 0xc3]);
            let code = c.finish();
            if code.len() > page::PAGE {
                VirtualFree(allocation.cast(), 0, MEM_RELEASE);
                return Err(HookError::UnsupportedPrefix);
            }
            std::ptr::copy_nonoverlapping(code.as_ptr(), allocation, code.len());
            let table = page::unwind(allocation, end, scope.frame);
            let info = allocation.add(page::PAGE + page::STATE_END + 16);
            *info = 1 | (2 << 3); // UNW_FLAG_UHANDLER, version 1
            info.add(8).cast::<u32>().write(end as u32);
            page::publish(allocation, Some(table), "publish scoped gateway")?;
            Ok(Self {
                entry: allocation as usize,
                data: &*data,
                switch: &*((*data).switch as *const AtomicU64),
            })
        }
    }
}

#[cfg(test)]
mod tests;
