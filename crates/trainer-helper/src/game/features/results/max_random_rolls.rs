//! Preserve every RNG draw, then return its maximum byte. Scope intentionally includes non-item RandomByte users.
use super::*;
use crate::game::gameplay::{Behavior, GameplayHook, HookFeature};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::MaxRandomRolls],
    behavior: Behavior::Postfix,
    targets,
};
const SPEC: MethodSpec = MethodSpec {
    class: c"RngElement",
    method: c"RandomByte",
    instance: true,
    params: &[],
    returns: "System.Byte",
};
pub(super) fn effect(online: NonNull<c_void>) -> PostfixEffect {
    let mut c = Code::new(online);
    c.emit(&[0x41, 0xba, 255, 0, 0, 0]); // r10d = byte maximum
    c.finish(&SPEC, 0)
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached retained metadata, admitted offline character and exact instance-byte ABI.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"LE.Core", c"RngElement")?;
        Ok(vec![
            access.hook_method_on(owner, &SPEC, module, size)?.postfix(effect(access.hook_online_storage(&images)?)),
        ])
    }
}
