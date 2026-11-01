//! Select the next mysterious-rift encounter through the game's Boolean choice.
use super::registry::{Behavior, GameplayHook, GameplayHookTarget, HookFeature, MethodSpec, Targets};
use crate::game::{GameError, RuntimeAccess};
use native_hooks::leaf::LeafEffect;
use std::ptr::NonNull;

pub(super) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::MysteriousRift],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the caller holds the attached metadata and retained module contract.
    let method = unsafe {
        access.hook_method(
            &MethodSpec {
                class: c"TimeBeastTracker",
                method: c"NextEncounterShouldBeEggs",
                instance: true,
                params: &[],
                returns: "System.Boolean",
            },
            module,
            size,
        )
    }?;
    Ok(vec![method.leaf(LeafEffect::SelectableBoolean, None, None)])
}
