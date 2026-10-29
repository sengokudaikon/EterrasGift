//! Bypass the chosen-mastery mismatch for deeper nodes; other mastery and node prerequisites remain.
use super::super::hooks::{Behavior, GroupId, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::PassiveChain,
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Passive mastery lock",
    MethodSpec {
        class: c"LocalTreeData",
        method: c"tryToSpendPassivePoint",
        instance: true,
        params: &["CharacterClassID", "System.Byte"],
        returns: "System.Boolean",
    },
    0x600,
    &[0xf, 0x85, -1, -1, -1, -1, 0x49, 0x8b, -1, -1, -1, -1, -1, 0x80, -1, -1, -1, -1, -1, -1, 0x75],
    0,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
