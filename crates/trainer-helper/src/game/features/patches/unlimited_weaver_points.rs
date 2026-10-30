//! Keep preview and actual Weaver spending consistent; at least one earned Weaver point remains required.
use super::super::hooks::{Behavior, GameplayHook, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::UnlimitedWeaverPoints],
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[
    PatchSpec::new(
        "Weaver preview point check",
        MethodSpec {
            class: c"TreeUIManager",
            method: c"MoveNext",
            instance: true,
            params: &[],
            returns: "System.Void",
        },
        0x1200,
        &[
            0xf, 0x8e, -1, -1, -1, -1, 0x48, 0x8b, -1, -1, -1, -1, -1, 0x83, -1, -1, -1, -1, -1, -1, 0x75, -1, 0xe8, -1, -1, -1, -1, 0x33, -1, 0xe8, -1, -1, -1, -1, 0x48, 0x8b,
            -1, 0x33,
        ],
        0,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    )
    .state_machine(c"<TrySimulatePreviewPointSpentWeaverTree>d__"),
    PatchSpec::new(
        "Weaver preview point spend",
        MethodSpec {
            class: c"TreeUIManager",
            method: c"MoveNext",
            instance: true,
            params: &[],
            returns: "System.Void",
        },
        0x1200,
        &[
            0x41, 0xff, -1, -1, -1, -1, -1, 0x48, 0x8b, -1, -1, 0x48, 0x85, -1, 0xf, 0x84, -1, -1, -1, -1, 0x8b, -1, -1, -1, -1, -1, 0xff, -1, 0x45, 0x33, -1, 0xe8, -1, -1, -1,
            -1, 0x48, 0x8b, -1, -1, 0x48, 0x85,
        ],
        0,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    )
    .state_machine(c"<TrySimulatePreviewPointSpentWeaverTree>d__"),
    PatchSpec::new(
        "Weaver point check",
        MethodSpec {
            class: c"LocalTreeData",
            method: c"TryToSpendWeaverPoint",
            instance: true,
            params: &["System.Byte"],
            returns: "System.Boolean",
        },
        0x500,
        &[0x40, 0xf, -1, -1, 0x66, -1, -1, -1, 0xf, 0x83],
        0,
        &[0x31, 0xc0, 0x90, 0x90],
    ),
];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
