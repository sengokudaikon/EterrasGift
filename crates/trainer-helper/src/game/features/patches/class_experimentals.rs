//! Supply true at the class-compatibility load while leaving subsequent experimental-item generation intact.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::ClassExperimentals], SPECS);
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
