//! Bypass both campaign and Monolith failed-roll branches, preserving both original rolls.
use super::super::hooks::{Behavior, GroupId, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::OmenWindows,
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[
    PatchSpec::new(
        "Omen window roll (campaign)",
        MethodSpec {
            class: c"RandomObserverWindowSpawner",
            method: c"ExecuteSpawn",
            instance: true,
            params: &["EncounterPlacementData"],
            returns: "System.Void",
        },
        0x1700,
        &[
            0xf3, 0xf, 0x59, 0x8e, -1, -1, -1, -1, 0x45, 0x33, 0xc0, 0x48, 0x8b, 0xc8, 0xe8, -1, -1, -1, -1, 0x84, 0xc0, 0xf, 0x84, -1, -1, -1, -1, 0x48, 0x83, 0xbe,
        ],
        21,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    ),
    PatchSpec::new(
        "Omen window roll (Monolith)",
        MethodSpec {
            class: c"RandomObserverWindowSpawner",
            method: c"ExecuteSpawn",
            instance: true,
            params: &["EncounterPlacementData"],
            returns: "System.Void",
        },
        0x1700,
        &[
            0xf3, 0xf, 0x58, 0xc8, 0x45, 0x33, 0xc0, 0x49, 0x8b, 0xc9, 0xe8, -1, -1, -1, -1, 0x84, 0xc0, 0xf, 0x84, -1, -1, -1, -1,
        ],
        17,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    ),
];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
