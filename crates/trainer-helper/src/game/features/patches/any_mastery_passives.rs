//! Bypass the chosen-mastery mismatch for deeper nodes; other mastery and node prerequisites remain.
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature::patches(&[GameplayHook::AnyMasteryPassives], SPECS);
const SPECS: &[PatchSpec] = &[PatchSpec::new(
    "Passive mastery lock",
    MethodSpec {
        class: c"LocalTreeData",
        method: c"tryToSpendPassivePoint",
        instance: true,
        params: &["CharacterClassID", "System.Byte"],
        returns: "System.Boolean",
    },
    0x600,
    &[0xf, 0x85, -1, -1, -1, -1, 0x49, 0x8b, -1, -1, -1, -1, -1, 0x80, -1, -1, -1, -1, -1, -1, 0x75],
    0,
    &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
)];
