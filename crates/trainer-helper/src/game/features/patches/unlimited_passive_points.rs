//! Replace this spend path's virtual unspent-point result with 113; other point reads remain the game's.
use super::super::hooks::{Behavior, GroupId, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::UnlimitedPassivePoints,
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Passive point check",
    MethodSpec {
        class: c"LocalTreeData",
        method: c"tryToSpendPassivePoint",
        instance: true,
        params: &["CharacterClassID", "System.Byte"],
        returns: "System.Boolean",
    },
    0x600,
    &[0xff, -1, -1, -1, -1, -1, 0x66, -1, -1, 0xf, 0x84, -1, -1, -1, -1, 0x80, -1, -1, -1, -1, -1, -1, 0x75],
    0,
    &[0x31, 0xc0, 0xb0, 0x71, 0x90, 0x90],
)];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
