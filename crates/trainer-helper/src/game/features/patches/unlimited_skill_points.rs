//! Bypass the skill-tree total-point budget while retaining each node and connection requirement.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::UnlimitedSkillPoints], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Skill point check",
    MethodSpec {
        class: c"LocalTreeData",
        method: c"tryToSpendSkillPoint",
        instance: true,
        params: &["Ability", "System.Byte"],
        returns: "System.Boolean",
    },
    0x600,
    &[
        0xf, 0x8d, -1, -1, -1, -1, 0x33, -1, 0xe8, -1, -1, -1, -1, 0x48, 0x85, -1, 0xf, 0x84, -1, -1, -1, -1, 0x45, 0x33, -1, 0x49, 0x8b,
    ],
    0,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
