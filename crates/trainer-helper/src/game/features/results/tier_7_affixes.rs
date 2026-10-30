//! Advance the game's affix RNG normally, then return internal tier six for newly generated affixes.
use super::*;
use crate::game::gameplay::{Behavior, GameplayHook, HookFeature};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::Tier7Affixes],
    behavior: Behavior::Postfix,
    targets,
};
const SPEC: MethodSpec = MethodSpec {
    class: c"GenerateItems",
    method: c"RollTier",
    instance: true,
    params: &[
        "ItemData",
        "Affix",
        "System.Int32",
        "System.Boolean",
        "System.Int32",
        "System.Single",
        "GenerationContext",
        "ItemRollModifiers",
    ],
    returns: "System.Int32",
};
pub(crate) fn effect(online: NonNull<c_void>) -> PostfixEffect {
    let mut c = Code::new(online);
    c.emit(&[0x41, 0xba, 6, 0, 0, 0]);
    c.finish(&SPEC, 0)
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact ten-position scalar/reference contract, including the stack float and MethodInfo.
    unsafe {
        let images = access.images()?;
        Ok(vec![access.hook_method(&SPEC, module, size)?.postfix(effect(access.hook_online_storage(&images)?))])
    }
}
