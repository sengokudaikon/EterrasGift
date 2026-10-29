//! Supply true at the class-compatibility load while leaving subsequent experimental-item generation intact.
use super::super::hooks::{Behavior, GroupId, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::ClassExperimentals,
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Experimental items class check",
    MethodSpec {
        class: c"RunePrisonItemDrops",
        method: c"SpawnExperimentalItem",
        instance: true,
        params: &[],
        returns: "System.Void",
    },
    0x1100,
    &[0xf, 0xb6, 0x40, 0x71, 0x88, 0x84, 0x24, -1, -1, -1, -1, 0x48, 0x8b, 0xd],
    0,
    &[0x31, 0xc0, 0xb0, 0x1],
)];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
