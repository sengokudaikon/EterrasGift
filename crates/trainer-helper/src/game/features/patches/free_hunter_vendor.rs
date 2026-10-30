//! Bypass the Hunter vendor's failed payment precheck at its checked branch, as in the original trainer.
use super::super::hooks::{Behavior, GameplayHook, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::FreeHunterVendor],
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Hunter vendor cost check",
    MethodSpec {
        class: c"HunterVendorContainer",
        method: c"CanRemoveItemPreCheck",
        instance: true,
        params: &["ItemContainerEntry", "System.Int32"],
        returns: "System.Boolean",
    },
    0x80,
    &[
        0x74, -1, 0xb0, 0x1, 0x48, 0x8b, -1, -1, -1, 0x48, 0x83, -1, -1, 0x5f, 0xc3, 0x48, 0x85, -1, 0x74, -1, 0x48, 0x8b, -1, -1, 0x48, 0x85, -1, 0x74, -1, 0x33, -1, 0xe8, -1,
        -1, -1, -1, 0x48, 0x8b, -1, 0x48, 0xc7, -1, -1, -1, -1, -1, -1, -1, 0x45, 0x33, -1, 0x41, 0xb0, -1, 0x48, 0x8b, -1, 0xe8, -1, -1, -1, -1, 0x48, 0x8b, -1, -1, -1, 0x32, -1,
        0x48, 0x83, -1, -1, 0x5f, 0xc3, 0xe8, -1, -1, -1, -1,
    ],
    0,
    &[0x90, 0x90],
)];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
