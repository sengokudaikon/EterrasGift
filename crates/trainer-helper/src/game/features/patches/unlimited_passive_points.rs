//! Replace this spend path's virtual unspent-point result with 113; other point reads remain the game's.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::UnlimitedPassivePoints], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Passive point check",
    MethodSpec {
        class: c"LocalTreeData",
        method: c"tryToSpendPassivePoint",
        instance: true,
        params: &["CharacterClassID", "System.Byte"],
        returns: "System.Boolean",
    },
    0x600,
    &[0xff, -1, -1, -1, -1, -1, 0x66, -1, -1, 0xf, 0x84, -1, -1, -1, -1, 0x80, -1, -1, -1, -1, -1, -1, 0x75],
    0,
    &[0x31, 0xc0, 0xb0, 0x71, 0x90, 0x90],
)];
