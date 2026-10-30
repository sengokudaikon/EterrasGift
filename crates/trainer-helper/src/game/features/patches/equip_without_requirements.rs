//! Bypass equipment class/faction/level refusals, preserving slot compatibility and the primordial limit.
use super::super::hooks::{Behavior, GameplayHook, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::EquipWithoutRequirements],
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[
    PatchSpec::new(
        "Equip class check",
        MethodSpec {
            class: c"OneSlotItemContainerWithCharacterRequirements",
            method: c"CanReceiveItem",
            instance: true,
            params: &["ItemData", "System.Int32"],
            returns: "System.Boolean",
        },
        0x400,
        &[0xf, 0x84, -1, -1, -1, -1, 0x45, 0x84, -1, 0x74, -1, 0x85, -1, 0x74, -1, 0x8b],
        0,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    ),
    PatchSpec::new(
        "Equip faction check",
        MethodSpec {
            class: c"OneSlotItemContainerWithCharacterRequirements",
            method: c"CanReceiveItem",
            instance: true,
            params: &["ItemData", "System.Int32"],
            returns: "System.Boolean",
        },
        0x400,
        &[0xf, 0x84, -1, -1, -1, -1, 0x45, 0x84, -1, 0x74, -1, 0x85, -1, 0x74, -1, 0x8b],
        9,
        &[0x90, 0x90],
    ),
    PatchSpec::new(
        "Equip level check",
        MethodSpec {
            class: c"OneSlotItemContainerWithCharacterRequirements",
            method: c"CanReceiveItem",
            instance: true,
            params: &["ItemData", "System.Int32"],
            returns: "System.Boolean",
        },
        0x400,
        &[0x7c, -1, 0x33, -1, 0x48, 0x89, -1, -1, -1, -1, -1, 0xb0],
        0,
        &[0x90, 0x90],
    ),
];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
