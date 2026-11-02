//! Duplicate only the local manager's checked direct DropItem call from AttemptItemDropInWorld. This build has one
//! such call, with the held entry's data/quantity and the manager's actor. Selecting its return address keeps failed
//! attempts, other callers, nested calls and other threads independent without a TLS mark to restore on unwind.
//! Ordinary positive amounts double; key/consumable (104) and woven-echo (107) stacks use the floored multiplier.
//! Cap at i32::MAX, preserve every other argument and the game's Boolean result; existing drops survive off.
use super::registry::*;
use crate::game::*;
use iced_x86::{Code as Instruction, Decoder, DecoderOptions, FlowControl};
use native_hooks::{
    emit::LeafCode,
    leaf::{FRAME, LeafEffect},
    patch::find_unique,
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::DuplicateOnDrop],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const ATTEMPT: MethodSpec = MethodSpec {
    class: c"ItemContainersManager",
    method: c"AttemptItemDropInWorld",
    instance: true,
    params: &[],
    returns: "System.Boolean",
};
const DROP: MethodSpec = MethodSpec {
    class: c"ItemContainersManager",
    method: c"DropItem",
    instance: true,
    params: &["Actor", "ItemData", "System.Int32"],
    returns: "System.Boolean",
};

pub(crate) fn effect(online: NonNull<c_void>, manager: NonNull<c_void>, caller: u64, item_type: u32) -> LeafEffect {
    let mut c = LeafCode::for_receiver(online);
    c.reference(FRAME.effect[1]);
    c.emit(&[0x4c, 0x39, 0xd9]); // cmp rcx,r11 (local manager)
    c.exit(0x85);
    c.emit(&[0x4c, 0x8b, 0x58, FRAME.effect[2], 0x4c, 0x39, 0x1c, 0x24]);
    c.exit(0x85);
    c.nonnull(&[0x4d, 0x85, 0xc0]);
    c.emit(&[0x45, 0x85, 0xc9]);
    c.exit(0x8e);
    c.offset(&[0x45, 0x0f, 0xb6, 0x90], item_type);
    c.emit(&[0x41, 0x83, 0xfa, 104]);
    let key = c.branch(0x84);
    c.emit(&[0x41, 0x83, 0xfa, 107]);
    let ordinary = c.branch(0x85);
    c.bind(key);
    c.emit(&[0xf2, 0x4c, 0x0f, 0x2c, 0x50, FRAME.answer]); // floor positive multiplier -> r10
    let scale = c.jump();
    c.bind(ordinary);
    c.emit(&[0x41, 0xba, 2, 0, 0, 0]);
    c.bind(scale);
    c.emit(&[0x45, 0x8b, 0xd9, 0x4d, 0x0f, 0xaf, 0xd3]); // positive amount * factor in 64 bits
    c.emit(&[0x41, 0xbb, 0xff, 0xff, 0xff, 0x7f, 0x4d, 0x3b, 0xd3, 0x4d, 0x0f, 0x4f, 0xd3, 0x45, 0x8b, 0xca]);
    c.finish_with([manager.as_ptr() as u64, caller, 0])
}

fn caller(code: &[u8], start: u64, destination: u64, data: u32, quantity: u32, actor: u32) -> Result<u64, GameError> {
    let short = |n| u8::try_from(n).ok().filter(|n| *n < 128).ok_or_else(|| unsupported("drop entry displacement"));
    let mut pattern: Vec<i16> = vec![
        0x44,
        0x8b,
        0x4f,
        short(quantity)?.into(),
        0x33,
        0xf6,
        0x4c,
        0x8b,
        0x47,
        short(data)?.into(),
        0x48,
        0x8b,
        0xcb,
        0x48,
        0x8b,
        0x93,
    ];
    pattern.extend(actor.to_le_bytes().map(i16::from));
    pattern.extend([0x48, 0x89, 0x74, 0x24, 0x20, 0xe8, -1, -1, -1, -1]);
    let at = find_unique(code, &pattern, 0, &[0]).map_err(|e| unsupported(format!("drop call: {e}")))?;
    let call = start + (at + pattern.len() - 5) as u64;
    let mut found = false;
    for instruction in Decoder::with_ip(64, code, start, DecoderOptions::NONE) {
        if instruction.is_invalid() {
            return Err(unsupported("drop attempt decode"));
        }
        if instruction.code() == Instruction::Int3 {
            break;
        }
        if instruction.ip() == call {
            if instruction.code() != Instruction::Call_rel32_64 || instruction.near_branch_target() != destination {
                return Err(unsupported("drop call destination"));
            }
            found = true;
        } else if instruction.flow_control() == FlowControl::Call && instruction.near_branch_target() == destination {
            return Err(unsupported("multiple player drop calls"));
        }
    }
    if !found {
        return Err(unsupported("drop call boundary"));
    }
    Ok(call + 5)
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: retained immutable metadata/module and initialized player/online statics. The recipe validates all
    // field offsets and the exact direct callee before any code is published.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"", DROP.class)?;
        let entry = access.class(&images, c"", c"ItemContainerEntry")?;
        let item = access.class(&images, c"", c"ItemData")?;
        let actor = access.hook_field(owner, c"actor", "Actor", 8)?;
        let data = access.hook_field_of(entry, c"data", item, 8)?;
        let quantity = access.hook_field(entry, c"quantity", "System.Int32", 4)?;
        let item_type = access.hook_field(item, c"itemType", "System.Byte", 1)?;
        let attempt = access.hook_method(&ATTEMPT, module, size)?;
        let drop = access.hook_method(&DROP, module, size)?;
        let start = attempt.address.as_ptr() as usize;
        let end = access.code_end(owner, start, 0x800, module.as_ptr() as usize + size)?;
        let code = std::slice::from_raw_parts(attempt.address.as_ptr(), end - start);
        let caller = caller(code, start as u64, drop.address.as_ptr() as u64, data, quantity, actor)?;
        Ok(vec![drop.leaf(
            effect(access.hook_online_storage(&images)?, access.hook_manager_storage(&images)?, caller, item_type),
            None,
            None,
        )])
    }
}

#[cfg(test)]
mod tests;
