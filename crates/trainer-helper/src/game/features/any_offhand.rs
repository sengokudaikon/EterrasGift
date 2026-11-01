//! Offhand compatibility bypasses, scoped to the offhand container and its linked-slot check. Item requirements
//! remain the game's own checks; this feature only changes weapon category and pairing rules.
//!
//! Four targets under one switch. The C++ app replaced only the placement check, which let an item be equipped whose
//! stats then did not apply; here placement and equipped eligibility answer alike, both pairing directions
//! (two-handed and linked slot) pass, and `EnforceWeaponLimitations` is skipped, as it inlines the pairing check and
//! would otherwise move equipment out. Off neither unequips nor recalculates: re-equip to see the change at once.
use super::registry::{Behavior, GameplayHook, GameplayHookTarget, HookFeature, MethodSpec, Targets};
use crate::game::*;
use native_hooks::leaf::LeafEffect;

pub(super) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::AnyOffhand],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached main thread with initialized statics, retained metadata and bounded module.
    unsafe {
        let images = access.images()?;
        let receiver = access.class(&images, c"", c"OffhandItemContainer")?;
        let receiver = NonNull::new(receiver.cast::<c_void>()).ok_or_else(|| unsupported("offhand receiver class"))?;
        let owner = access.class(&images, c"", c"OneSlotItemContainerWithCharacterRequirements")?;
        let linked = access.class(&images, c"", c"LinkedOneSlotItemContainer")?;
        if (access.class_value)(receiver.as_ptr().cast()) || !(access.assignable)(owner, receiver.as_ptr().cast()) || !(access.assignable)(linked, receiver.as_ptr().cast()) {
            return Err(unsupported("offhand receiver inheritance"));
        }
        let offset = access.hook_field_of(linked, c"linkedSlot", linked, 8)?;
        let specs = [
            (
                MethodSpec {
                    class: c"OneSlotItemContainerWithCharacterRequirements",
                    method: c"alternateSufficientBaseTypeConditionFulfilled",
                    instance: true,
                    params: &["System.Int32"],
                    returns: "System.Boolean",
                },
                LeafEffect::Boolean(true),
                Some(receiver),
                None,
            ),
            (
                MethodSpec {
                    class: c"OffhandItemContainer",
                    method: c"IncompatibleDueTo2hWeapon",
                    instance: false,
                    params: &["AdditionalBaseTypeProvider", "ItemData", "ItemData"],
                    returns: "System.Boolean",
                },
                LeafEffect::Boolean(false),
                None,
                None,
            ),
            (
                MethodSpec {
                    class: c"LinkedOneSlotItemContainer",
                    method: c"incompatibleWithItemInLinkedSlot",
                    instance: true,
                    params: &["ItemData"],
                    returns: "System.Boolean",
                },
                LeafEffect::Boolean(false),
                Some(receiver),
                Some(offset),
            ),
            (
                MethodSpec {
                    class: c"OffhandItemContainer",
                    method: c"EnforceWeaponLimitations",
                    instance: true,
                    params: &["System.Boolean", "UnityEngine.Vector2Int"],
                    returns: "System.Void",
                },
                LeafEffect::Void,
                None,
                None,
            ),
        ];
        specs
            .into_iter()
            .map(|(spec, effect, receiver, linked)| Ok(access.hook_method(&spec, module, size)?.leaf(effect, receiver, linked)))
            .collect()
    }
}
