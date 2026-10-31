//! Report zero equipped primordials only for the current local item manager, skipping the original count there.
//! Other managers retain their actual count. In the inspected build both the equip permission and ItemEquipManager's
//! apply/remove-stat checks call this counter, so the bypass covers eligibility and usable item stats together.
//! It changes no item or requirement; off restores the count, and re-equipping lets the game recheck existing items.
use super::*;
use crate::game::registry::{Behavior, GameplayHook, HookFeature, MethodSpec, Targets};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::SeveralPrimordials],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"ItemContainersManager",
    method: c"NumberOfEquippedPrimordialItems",
    instance: true,
    params: &[],
    returns: "System.Int32",
};
pub(crate) fn effect(receiver: Receiver) -> LeafEffect {
    receiver.effect(|c| c.emit(&[0x31, 0xc0, 0xc3])) // xor eax,eax; ret
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact two-position Int32 ABI, retained metadata and initialized process-owned reference storage.
    unsafe {
        let images = access.images()?;
        let receiver = Receiver {
            online: access.hook_online_storage(&images)?,
            storage: access.hook_manager_storage(&images)?,
            component: None,
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.leaf(effect(receiver), None, None)])
    }
}
