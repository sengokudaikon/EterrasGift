//! Apply all bonuses of a worn set during stat recalculation; an unworn set still grants nothing.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::FullSetBonus], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Set bonus check",
    MethodSpec {
        class: c"ItemEquipManager",
        method: c"UpdateStats",
        instance: true,
        params: &[
            "ItemDataUnpacked",
            "System.Boolean",
            "System.Boolean",
            "System.Single",
            "System.Single",
            "System.Single",
            "System.Single",
        ],
        returns: "System.Void",
    },
    0x2100,
    &[0xf, 0x8f, -1, -1, -1, -1, 0x83, -1, -1, -1, 0x75, -1, 0xf3],
    0,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
