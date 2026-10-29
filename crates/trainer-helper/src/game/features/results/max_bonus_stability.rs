//! Preserve the original bonus calculation, then raise it to the echo's maximum without lowering a higher result.
use super::*;
use crate::game::gameplay::{Behavior, GroupId, HookFeature};

pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::MaxBonusStability,
    behavior: Behavior::Postfix,
    targets,
};
const SPEC: MethodSpec = MethodSpec {
    class: c"MonolithZoneManager",
    method: c"getBonusStabilityInRegularEcho",
    instance: true,
    params: &[],
    returns: "System.Int32",
};
pub(super) fn effect(online: NonNull<c_void>, maximum: u32) -> PostfixEffect {
    let mut c = Code::new(online);
    c.nonnull(&[0x48, 0x85, 0xc9]);
    c.offset(&[0x44, 0x3b, 0x91], maximum); // cmp r10d,[rcx+maximum]
    c.offset(&[0x44, 0x0f, 0x4c, 0x91], maximum); // cmovl r10d,[rcx+maximum]
    c.finish(&SPEC, 0)
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: readable live zone after original; exact Int32 field and method contract checked from metadata.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"", c"MonolithZoneManager")?;
        let maximum = access.hook_field(owner, c"maxBonusStablity", "System.Int32", 4)?;
        Ok(vec![
            access
                .hook_method_on(owner, &SPEC, module, size)?
                .postfix(effect(access.hook_online_storage(&images)?, maximum)),
        ])
    }
}
