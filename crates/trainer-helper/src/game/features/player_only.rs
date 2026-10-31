//! Receiver selection for player-only leaves. Read the game's current reference static on each call, optionally
//! follow one metadata-checked component field, and compare with the receiver. No cached component, helper callback,
//! allocation or runtime lookup runs on the gameplay thread. The live actor/component graph must remain readable
//! for that call, as in the existing mana-cost and ability-speed hooks. Effects use volatile RAX/R11/XMM0 only;
//! arguments, the caller's stack and nonvolatile registers stay intact. Original exceptions are never intercepted.
use crate::{
    game::*,
    hooks::{
        emit,
        leaf::{FRAME, GeneratedEffect, LeafEffect},
    },
};

#[derive(Clone, Copy)]
pub(crate) struct Receiver {
    pub online: NonNull<c_void>,
    /// Address of the process-owned reference static, not the current object itself.
    pub storage: NonNull<c_void>,
    /// None selects that object; Some selects its component at the validated offset.
    pub component: Option<u32>,
}
impl Receiver {
    pub(super) fn effect(self, emit: impl FnOnce(&mut emit::Code)) -> LeafEffect {
        let mut c = emit::Code::default();
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
        c.exit(0x84);
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.effect[0], 0x41, 0x80, 0x3b, 0]);
        c.exit(0x85);
        c.nonnull(&[0x48, 0x85, 0xc9]);
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.effect[1], 0x4d, 0x8b, 0x1b]);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        if let Some(offset) = self.component {
            c.offset(&[0x4d, 0x8b, 0x9b], offset);
        }
        c.emit(&[0x4c, 0x39, 0xd9]); // receiver == current object/component
        c.exit(0x85);
        emit(&mut c);
        LeafEffect::Generated(GeneratedEffect {
            code: c.finish(),
            slots: [self.online.as_ptr() as u64, self.storage.as_ptr() as u64, 0, 0],
        })
    }
}

pub(crate) mod fixed_area;
pub(crate) mod refill_mana;
pub(crate) mod several_primordials;
#[cfg(test)]
mod tests;
