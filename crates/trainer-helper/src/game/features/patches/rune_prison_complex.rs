//! Bypass the Rune Prison Complex failed-roll branch, preserving level/echo eligibility and the roll itself.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::RunePrisonComplex], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Rune Prison Complex roll",
    MethodSpec {
        class: c"RunePrisonItemDrops",
        method: c"SpawnExperimentalItem",
        instance: true,
        params: &[],
        returns: "System.Void",
    },
    0x1100,
    &[
        0xf, 0x84, -1, -1, -1, -1, 0x48, 0x8b, 0xd, -1, -1, -1, -1, 0x83, 0xb9, 0xe4, 0x0, 0x0, 0x0, 0x0, 0x75, 0x5, 0xe8, -1, -1, -1, -1, 0x45, 0x33, 0xc0, 0x48, 0x8d, 0x94,
        0x24, -1, -1, -1, -1, 0x48, 0x8d, 0x8c, 0x24, -1, -1, -1, -1, 0xe8,
    ],
    0,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
