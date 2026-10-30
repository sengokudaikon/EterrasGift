//! Bypass the character-versus-item-level refusal; preserve the remaining Forge admission checks.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::ForgeAboveLevel], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Forge level check",
    MethodSpec {
        class: c"CraftingManager",
        method: c"CheckForgeCapability",
        instance: true,
        params: &["System.String&", "System.Boolean&", "System.Boolean&", "System.String&"],
        returns: "System.Boolean",
    },
    0x2000,
    &[
        0x3b, 0xf8, 0xf, 0x8c, -1, -1, -1, -1, 0x48, 0x8b, 0x45, 0xe8, 0x48, 0x8b, 0x48, 0x10, 0x48, 0x85, 0xc9, 0xf, 0x84,
    ],
    2,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
