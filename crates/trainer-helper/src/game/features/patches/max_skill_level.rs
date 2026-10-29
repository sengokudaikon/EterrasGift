//! Set specialized skills to their level-20 XP cap when the game next applies skill experience.
use super::super::hooks::{Behavior, GroupId, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::MaxSkillLevel,
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Skill experience cap",
    MethodSpec {
        class: c"LocalTreeData",
        method: c"ApplyAbilityXp",
        instance: true,
        params: &["System.Int32", "System.Boolean"],
        returns: "System.Void",
    },
    0x380,
    &[0x7f, -1, 0x44, 0x1, -1, -1, 0x39, -1, -1, 0x7e, -1, 0x89, -1, -1, 0x33, -1, 0xf, 0x1f],
    9,
    &[0x90, 0x90],
)];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
