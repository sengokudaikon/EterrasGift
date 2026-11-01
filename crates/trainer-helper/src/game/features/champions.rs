//! Override only the champion spawner's checked RollRangeLucky call. Other callers and the other overloads keep
//! their RNG behavior; this selected call skips the RNG as the original trainer does. The game still rounds and
//! enforces its placement cap and existing champion count. The leaf preserves the original caller address directly.
use super::registry::*;
use crate::game::*;
use iced_x86::{Code, Decoder, DecoderOptions};
use native_hooks::{
    emit::LeafCode,
    leaf::{FRAME, LeafEffect},
    patch::find_unique,
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::Champions],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPAWN: MethodSpec = MethodSpec {
    class: c"RandomChampionSpawner",
    method: c"ExecuteSpawn",
    instance: true,
    params: &["RandomEncounterTypes.EncounterPlacementData"],
    returns: "System.Void",
};
const ROLL: MethodSpec = MethodSpec {
    class: c"RngElement",
    method: c"RollRangeLucky",
    instance: true,
    params: &["System.Single", "System.Single", "System.Int32"],
    returns: "System.Single",
};
const PATTERN: &[i16] = &[
    0x41, 0x0f, 0x28, 0xd0, 0xe8, -1, -1, -1, -1, 0x33, 0xd2, 0xe8, -1, -1, -1, -1, 0x48, 0x8b, 0x0d, -1, -1, -1, -1, 0x8b, 0xd8,
];
fn caller(code: &[u8], start: u64, original: u64) -> Result<u64, GameError> {
    let at = find_unique(code, PATTERN, 4, &[0]).map_err(|e| unsupported(format!("champion roll: {e}")))?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    while decoder.can_decode() {
        let instruction = decoder.decode();
        if instruction.is_invalid() || instruction.code() == Code::Int3 {
            break;
        }
        if instruction.ip() == start + at as u64 {
            if instruction.code() == Code::Call_rel32_64 && instruction.near_branch_target() == original {
                return Ok(instruction.next_ip());
            }
            break;
        }
    }
    Err(unsupported("champion roll instruction/overload"))
}
pub(crate) fn effect(online: NonNull<c_void>, caller: u64) -> LeafEffect {
    let mut c = LeafCode::new(online);
    c.emit(&[0x4c, 0x8b, 0x1c, 0x24, 0x4c, 0x3b, 0x58, FRAME.effect[1]]);
    c.exit(0x85);
    c.emit(&[0xf2, 0x0f, 0x5a, 0x40, FRAME.answer, 0xc3]);
    c.finish_with([caller, 0, 0])
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact float/int overload on LE.Core.RngElement; one unique decoded direct caller within bounded retained code.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"", c"RandomChampionSpawner")?;
        let spawn = access.hook_method_on(owner, &SPAWN, module, size)?;
        let roll = access.hook_method_on(access.class(&images, c"LE.Core", c"RngElement")?, &ROLL, module, size)?;
        let start = spawn.address.as_ptr() as usize;
        let module_end = (module.as_ptr() as usize).checked_add(size).ok_or_else(|| unsupported("champion module range"))?;
        let end = access.code_end(owner, start, 0x280, module_end)?;
        let code = std::slice::from_raw_parts(spawn.address.as_ptr(), end - start);
        let caller = caller(code, start as u64, roll.address.as_ptr() as u64)?;
        Ok(vec![roll.leaf(effect(access.hook_online_storage(&images)?, caller), None, None)])
    }
}

#[cfg(test)]
pub(crate) mod tests;
