//! Own all five item/label/vacuum/gold/contact sites as one feature; preserve other pickup conditions.
use super::super::hooks::{Behavior, GroupId, HookFeature};
use super::*;
pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::PickupAnywhere,
    behavior: Behavior::Patch,
    targets,
};
const SPECS: &[PatchSpec] = &[
    PatchSpec::new(
        "Item pickup range",
        MethodSpec {
            class: c"GroundItemManager",
            method: c"pickupItem",
            instance: true,
            params: &["GroundItemList", "Actor", "System.UInt32", "StackableItemFlags"],
            returns: "System.Void",
        },
        0x700,
        &[0xf, 0x2f, 0x5, -1, -1, -1, -1, 0xf, 0x87, -1, -1, -1, -1, 0x48, 0x8b],
        7,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    ),
    PatchSpec::new(
        "Item label pickup range",
        MethodSpec {
            class: c"GroundItemLabel",
            method: c"ClickedItem",
            instance: true,
            params: &[],
            returns: "System.Void",
        },
        0x600,
        &[0xf3, 0x44, 0xf, 0x10, 0x5, -1, -1, -1, -1, 0x41, 0xf, 0x2f, 0xc0, 0x77, -1, 0x33, 0xd2],
        13,
        &[0x90, 0x90],
    ),
    PatchSpec::new(
        "Vacuum range",
        MethodSpec {
            class: c"GroundItemManager",
            method: c"vacuumNearbyStackableItems",
            instance: true,
            params: &["Actor", "GroundItemList", "UnityEngine.Vector3", "StackableItemFlags"],
            returns: "System.Void",
        },
        0xc80,
        &[0xe8, -1, -1, -1, -1, 0x84, 0xc0, 0xf, 0x84, -1, -1, -1, -1, 0x48, 0x85, -1, 0xf, 0x84],
        7,
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    ),
    PatchSpec::new(
        "Gold pickup range",
        MethodSpec {
            class: c"GroundItemManager",
            method: c"pickupGold",
            instance: true,
            params: &["GroundItemList", "Actor", "System.UInt32"],
            returns: "System.Boolean",
        },
        0x300,
        &[0xf, 0x2f, 0x5, -1, -1, -1, -1, 0x77, -1, 0x49, 0x8b],
        7,
        &[0x90, 0x90],
    ),
    PatchSpec::new(
        "Ground object touch",
        MethodSpec {
            class: c"PickupGroundObjectInteraction",
            method: c"initialise",
            instance: true,
            params: &["Actor", "System.UInt32"],
            returns: "System.Void",
        },
        0x100,
        &[0xc6, 0x81, -1, -1, -1, -1, 0x0, 0x48, 0x85, 0xc0, 0x74],
        6,
        &[0x1],
    ),
];
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the shared caller retains the attached runtime, module and exclusive code ownership contract.
    unsafe { access.patch_targets(SPECS, module, size) }
}
