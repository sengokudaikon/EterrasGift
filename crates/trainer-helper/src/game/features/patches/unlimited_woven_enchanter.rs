//! Suppress offline local-use increments in both station overloads, preserving the server-account branch.
use super::super::hooks::{Behavior, GameplayHook, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::UnlimitedWovenEnchanter],
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[
    PatchSpec::new(
        "Woven Enchanter use count",
        MethodSpec {
            class: c"ZoneRestrictedCraftingManager",
            method: c"OnUse",
            instance: true,
            params: &["Actor"],
            returns: "System.Void",
        },
        0x200,
        &[
            0xff, -1, -1, 0xe9, -1, -1, -1, -1, 0x48, 0x85, -1, 0xf, 0x84, -1, -1, -1, -1, 0x45, 0x33, -1, 0x48, 0x8d, -1, -1, 0x48, 0x8b, -1, 0xe8,
        ],
        0,
        &[0x90, 0x90, 0x90],
    ),
    PatchSpec::new(
        "Woven Enchanter typed use count",
        MethodSpec {
            class: c"ZoneRestrictedCraftingManager",
            method: c"OnUse",
            instance: false,
            params: &["Actor", "CraftingType"],
            returns: "System.Void",
        },
        0x280,
        &[
            0xff, -1, -1, 0xe9, -1, -1, -1, -1, 0x48, 0x85, -1, 0xf, 0x84, -1, -1, -1, -1, 0x45, 0x33, -1, 0x48, 0x8d, -1, -1, 0x48, 0x8b, -1, 0xe8,
        ],
        0,
        &[0x90, 0x90, 0x90],
    ),
];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
