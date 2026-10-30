//! Keep cache eligibility, placement and the RNG call; bypass only the failed-roll branch.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::EndgameCache], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Endgame cache roll",
    MethodSpec {
        class: c"RandomOneShotCacheSpawner",
        method: c"ExecuteSpawn",
        instance: true,
        params: &["EncounterPlacementData"],
        returns: "System.Void",
    },
    0x260,
    &[
        0x45, 0x33, 0xc0, 0xf, 0x28, 0xce, 0x48, 0x8b, 0xc8, 0xe8, -1, -1, -1, -1, 0x84, 0xc0, 0xf, 0x84, -1, -1, -1, -1, 0x33, 0xd2, 0x48, 0x8b, 0xcf, 0xe8,
    ],
    16,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
