//! Keep Weaver/zone eligibility, then replace the final RNG tail-call with true.
use super::super::hooks::{Behavior, GameplayHook, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::FavorCache],
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Favor cache roll",
    MethodSpec {
        class: c"RandomFavourCacheSpawner",
        method: c"CanSpawn",
        instance: true,
        params: &["EncounterPlacementData"],
        returns: "System.Boolean",
    },
    0x120,
    &[
        0x45, 0x33, 0xc0, 0x48, 0x8b, 0xc8, 0xf, 0x28, 0xce, 0xf, 0x28, 0x74, 0x24, 0x20, 0x48, 0x8b, 0x5c, 0x24, 0x40, 0x48, 0x83, 0xc4, 0x30, 0x5f, 0xe9, -1, -1, -1, -1,
    ],
    24,
    &[0xb0, 0x1, 0xc3, 0x90, 0x90],
)];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
