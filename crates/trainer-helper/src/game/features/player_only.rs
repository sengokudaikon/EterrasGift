//! Receiver selection for player-only leaves. Read the game's current reference static on each call, optionally
//! follow one metadata-checked component field, and compare with the receiver. No cached component, helper callback,
//! allocation or runtime lookup runs on the gameplay thread. The live actor/component graph must remain readable
//! for that call, as in the existing mana-cost and ability-speed hooks. Effects use volatile RAX/R11/XMM0 only;
//! arguments, the caller's stack and nonvolatile registers stay intact. Original exceptions are never intercepted.
use crate::game::*;
use native_hooks::{
    emit::{self, LeafCode},
    leaf::{FRAME, LeafEffect},
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
        let mut c = LeafCode::for_receiver(self.online);
        c.reference(FRAME.effect[1]);
        if let Some(offset) = self.component {
            c.offset(&[0x4d, 0x8b, 0x9b], offset);
        }
        c.emit(&[0x4c, 0x39, 0xd9]); // receiver == current object/component
        c.exit(0x85);
        emit(&mut c);
        c.finish_with([self.storage.as_ptr() as u64, 0, 0])
    }
}

pub(crate) mod fixed_area;
pub(crate) mod refill_mana;
pub(crate) mod several_primordials;
#[cfg(test)]
mod tests;
