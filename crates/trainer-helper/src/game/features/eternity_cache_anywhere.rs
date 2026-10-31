//! Admit only the current player's actor and suppress the final local-used mark. Keep the original cache operation,
//! actor bookkeeping and affix selection. The tiny tier-cap function begins with branches our detour backend cannot
//! relocate: its three checked capped returns are patched to its existing fallback (200), the same result as the
//! C++ trainer's INT32_MAX tier. Off restores every immediate and the used mark with the actor-check entry.
use super::{patches::PatchSpec, player_only::Receiver, registry::*};
use crate::{game::*, hooks::leaf::LeafEffect};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::EternityCacheAnywhere],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const CAN_USE: MethodSpec = MethodSpec {
    class: c"EternityCacheZoneManager",
    method: c"ActorCanUseEternityCacheInThisZone",
    instance: false,
    params: &["Actor"],
    returns: "System.Boolean",
};
const TIER: MethodSpec = MethodSpec {
    class: c"Item",
    method: c"getMaxUniqueLevelRequirementForDungeonTier",
    instance: false,
    params: &["System.Int32"],
    returns: "System.Int32",
};
// The complete decision table, including all branch destinations and the existing fallback. A changed table refuses
// support rather than guessing a new cap or lowering the deepest tier to the old cheat table's 127.
const TIER_PATTERN: &[i16] = &[
    0x85, 0xc9, 0x74, 0x1c, 0x83, 0xe9, 1, 0x74, 0x11, 0x83, 0xe9, 1, 0x74, 6, 0xb8, 0xc8, 0, 0, 0, 0xc3, 0xb8, 0x4b, 0, 0, 0, 0xc3, 0xb8, 0x41, 0, 0, 0, 0xc3, 0xb8, 0x32, 0, 0,
    0, 0xc3,
];
const PATCHES: &[PatchSpec] = &[
    PatchSpec::new(
        "Eternity Cache used mark",
        MethodSpec {
            class: c"EternityCacheZoneManager",
            method: c"ActorUsedEternityCache",
            instance: false,
            params: &["Actor", "System.Nullable<System.UInt16>"],
            returns: "System.Void",
        },
        0x500,
        &[0xc7, 0x41, -1, 1, 0, 0, 0, 0x48, 0x83, 0xc4, -1, 0x5f, 0x5d, 0x5b, 0xc3, 0x48, 0x85],
        0,
        &[0x90; 7],
    ),
    PatchSpec::new("Eternity Cache tier 2 cap", TIER, 0x40, TIER_PATTERN, 21, &[0xc8, 0, 0, 0]),
    PatchSpec::new("Eternity Cache tier 1 cap", TIER, 0x40, TIER_PATTERN, 27, &[0xc8, 0, 0, 0]),
    PatchSpec::new("Eternity Cache tier 0 cap", TIER, 0x40, TIER_PATTERN, 33, &[0xc8, 0, 0, 0]),
];
pub(crate) fn effect(receiver: Receiver) -> LeafEffect {
    receiver.effect(|c| c.emit(&[0xb8, 1, 0, 0, 0, 0xc3]))
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact static actor/MethodInfo ABI, retained reference statics, metadata-checked state field and bounded code recipes.
    unsafe {
        let images = access.images()?;
        let manager = access.class(&images, c"", c"EternityCacheZoneManager")?;
        let state = access.hook_field(manager, c"localCacheState", "CacheState", 4)?;
        access.enum_storage(access.class(&images, c"", c"CacheState")?, c"Int32", 0x08)?;
        let receiver = Receiver {
            online: access.hook_online_storage(&images)?,
            storage: access.hook_player_storage(&images)?,
            component: None,
        };
        let mut targets = vec![access.hook_method(&CAN_USE, module, size)?.leaf(effect(receiver), None, None)];
        targets.extend(access.patch_targets(PATCHES, module, size)?);
        let TargetEffect::Patch(change) = &targets[1].effect else { unreachable!() };
        if state > 127 || u32::from(change.expected[2]) != state {
            return Err(unsupported("Eternity Cache used-state displacement"));
        }
        Ok(targets)
    }
}

#[cfg(test)]
mod tests;
