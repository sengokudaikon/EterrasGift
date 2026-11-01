//! Scale the current player's movement-stat result only at updateSpeed's checked direct query. This build makes
//! that query once, outside any loop; selecting its return address replaces the C++ trainer's consumable TLS mark.
//! Nested updates qualify independently; unlike a single clear-on-exit mark, an unrelated nested update cannot
//! cancel the outer player's query. There is no context to leak on exception or across threads, and no updateSpeed
//! detour or helper callback. The original GetStatValue runs once with all eleven native positions intact.
//!
//! The checked code recipe binds SpeedManager.actor, Actor.stats, SP.Movespeed, default query arguments and the
//! exact GetStatValue destination. Unsupported/ambiguous recipes or a backward branch across the query refuse
//! activation. Player identity, online state and settings are checked after the original; zero retains its result,
//! as in the original trainer. Speed is refreshed by the game's next update (e.g. re-equip boots), not by a new scan.
use super::*;
use crate::game::registry::{Behavior, GameplayHook, HookFeature, Targets};
use iced_x86::{Code as Instruction, Decoder, DecoderOptions, FlowControl};
use native_hooks::{
    patch::find_unique,
    postfix::{FRAME, frame_size},
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::MovementSpeed],
    behavior: Behavior::Postfix,
    targets: Targets::Resolve(targets),
};
const UPDATE: MethodSpec = MethodSpec {
    class: c"SpeedManager",
    method: c"updateSpeed",
    instance: true,
    params: &[],
    returns: "System.Void",
};
pub(super) const SPEC: MethodSpec = MethodSpec {
    class: c"Stats",
    method: c"GetStatValue",
    instance: true,
    params: &[
        "SP",
        "AT",
        "System.Single",
        "System.Single",
        "System.Single",
        "System.Byte",
        "System.Int32",
        "System.Boolean",
        "System.Boolean",
    ],
    returns: "System.Single",
};
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub player: NonNull<c_void>,
    pub stats: u32,
    pub property: u8,
    pub caller: u64,
}
impl Layout {
    pub(crate) fn effect(self) -> PostfixEffect {
        let mut c = PostfixCode::new(self.online);
        c.emit(&[0x80, 0xfa, self.property]); // restored original DL, not the original's scratch RDX
        c.exit(0x85);
        c.emit(&[0x49, 0xbb]);
        c.emit(&self.caller.to_le_bytes());
        c.offset(&[0x4c, 0x39, 0x9c, 0x24], u32::from(frame_size(SPEC.positions())));
        c.exit(0x85);
        c.nonnull(&[0x48, 0x85, 0xc9]);
        c.reference(FRAME.slots[1]);
        c.offset(&[0x49, 0x3b, 0x8b], self.stats);
        c.exit(0x85);
        c.emit(&[0x66, 0x0f, 0xef, 0xc9, 0x66, 0x0f, 0x2e, 0x48, FRAME.value]); // multiplier <= 0 keeps original
        c.exit(0x83);
        c.emit(&[0xf3, 0x0f, 0x5a, 0xc0, 0xf2, 0x0f, 0x59, 0x40, FRAME.value, 0xf2, 0x0f, 0x5a, 0xc0]);
        c.finish(SPEC.positions(), self.player.as_ptr() as u64)
    }
}

const SEARCH: usize = 0x300;
// From the actor load through the direct call. Field offsets and property come from metadata; null-branch and call
// displacements are checked separately. This is deliberately one concrete compiler recipe, not a generic TLS ABI.
const QUERY: &[i16] = &[
    0x48, 0x8b, 0x43, 0, 0x48, 0x89, 0xb4, 0x24, 0x80, 0, 0, 0, 0x0f, 0x29, 0x74, 0x24, 0x60, 0x48, 0x85, 0xc0, 0x0f, 0x84, -1, -1, -1, -1, 0x48, 0x8b, 0x48, 0, 0x48, 0x85, 0xc9,
    0x0f, 0x84, -1, -1, -1, -1, 0x33, 0xf6, 0x0f, 0x57, 0xdb, 0x48, 0x89, 0x74, 0x24, 0x50, 0x45, 0x33, 0xc0, 0x40, 0x88, 0x74, 0x24, 0x48, 0xb2, 0, 0x40, 0x88, 0x74, 0x24, 0x40,
    0x89, 0x74, 0x24, 0x38, 0x40, 0x88, 0x74, 0x24, 0x30, 0xf3, 0x0f, 0x11, 0x5c, 0x24, 0x28, 0xf3, 0x0f, 0x11, 0x5c, 0x24, 0x20, 0xe8, -1, -1, -1, -1,
];
fn caller(code: &[u8], start: u64, original: u64, actor: u32, stats: u32, property: u8) -> Result<u64, GameError> {
    let short = |offset| {
        u8::try_from(offset)
            .ok()
            .filter(|n| *n <= 127)
            .ok_or_else(|| unsupported("movement query field displacement"))
    };
    let mut pattern = QUERY.to_vec();
    pattern[3] = i16::from(short(actor)?);
    pattern[29] = i16::from(short(stats)?);
    pattern[58] = i16::from(property);
    let at = find_unique(code, &pattern, 0, &[0]).map_err(|e| unsupported(format!("movement query: {e}")))?;
    let call = start + (at + pattern.len() - 5) as u64;
    let mut boundary = false;
    let mut found = false;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    while decoder.can_decode() {
        let i = decoder.decode();
        if i.is_invalid() {
            return Err(unsupported("movement query decode"));
        }
        if i.code() == Instruction::Int3 {
            boundary = true;
            break;
        }
        if i.flow_control() == FlowControl::IndirectBranch {
            return Err(unsupported("movement query indirect branch"));
        }
        if i.ip() == call {
            if i.code() != Instruction::Call_rel32_64 || i.near_branch_target() != original {
                return Err(unsupported("movement query destination"));
            }
            found = true;
        }
        if matches!(i.flow_control(), FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch) {
            let target = i.near_branch_target();
            if i.ip() >= call && target <= call {
                return Err(unsupported("movement query repeats"));
            }
            if i.ip() >= start + at as u64 && i.ip() < call && target <= call + 4 {
                return Err(unsupported("movement query guard"));
            }
        }
    }
    if !found || !boundary {
        return Err(unsupported("movement query extent"));
    }
    Ok(call + 5)
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata and bounded retained module. No runtime call executes on a gameplay thread.
    unsafe {
        let images = access.images()?;
        let property_class = access.class(&images, c"", c"SP")?;
        access.enum_storage(property_class, c"Byte", raw::type_code::U1)?;
        access.enum_storage(access.class(&images, c"", c"AT")?, c"Int32", raw::type_code::I4)?;
        let field = access.field(property_class, c"Movespeed")?;
        let ty = (access.field_type)(field);
        if ty.is_null()
            || (access.type_byref)(ty)
            || (access.field_parent)(field) != property_class
            || (access.type_class)(ty) != property_class
            || (access.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL) != (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL)
        {
            return Err(unsupported("Movespeed literal"));
        }
        let mut property = 0u8;
        (access.static_get)(field, (&raw mut property).cast());
        let stats = access.hook_field_of(access.class(&images, c"", c"Actor")?, c"stats", access.class(&images, c"", c"BaseStats")?, 8)?;
        let speed = access.class(&images, c"", c"SpeedManager")?;
        let actor = access.hook_field(speed, c"actor", "Actor", 8)?;
        let update = access.hook_method(&UPDATE, module, size)?;
        let original = access.hook_method(&SPEC, module, size)?;
        let start = update.address.as_ptr() as usize;
        let module_end = (module.as_ptr() as usize).checked_add(size).ok_or_else(|| unsupported("movement module range"))?;
        let end = access.code_end(speed, start, SEARCH, module_end)?;
        let code = std::slice::from_raw_parts(update.address.as_ptr(), end - start);
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            player: access.hook_player_storage(&images)?,
            stats,
            property,
            caller: caller(code, start as u64, original.address.as_ptr() as u64, actor, stats, property)?,
        };
        Ok(vec![original.postfix(layout.effect())])
    }
}

#[cfg(test)]
pub(crate) mod tests;
