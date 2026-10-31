//! Keep Nemesis eligibility, placement and the RNG call; bypass only the failed-roll branch.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::Nemesis], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Nemesis roll",
    MethodSpec {
        class: c"RandomNemesisSpawner",
        method: c"ExecuteSpawn",
        instance: true,
        params: &["RandomEncounterTypes.EncounterPlacementData"],
        returns: "System.Void",
    },
    0x640,
    &[
        0x45, 0x33, 0xc0, 0xf, 0x28, 0xce, 0x48, 0x8b, 0xc8, 0xe8, -1, -1, -1, -1, 0x84, 0xc0, 0xf, 0x84, -1, -1, -1, -1, 0x48, 0x8b, 0x47, 0x18, 0x48, 0x85, 0xc0, 0xf, 0x84,
    ],
    16,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
