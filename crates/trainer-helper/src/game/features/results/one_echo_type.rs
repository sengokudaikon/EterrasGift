//! Override newly generated echo types after the original selection; echo cycling remains a separate pending feature.
use super::*;
use crate::game::gameplay::{Behavior, GroupId, HookFeature};
use crate::hooks::postfix::FRAME;

pub(crate) const FEATURE: HookFeature = HookFeature {
    group: GroupId::OneEchoType,
    behavior: Behavior::Postfix,
    targets,
};
const SPEC: MethodSpec = MethodSpec {
    class: c"EchoWeb",
    method: c"getIslandTypeForNewIsland",
    instance: true,
    params: &[
        "System.Int32",
        "System.Collections.Generic.HashSet<IslandType>",
        "EchoChainType",
        "ChainStatus",
        "WovenEchoType",
    ],
    returns: "IslandType",
};
pub(crate) const TYPES: [i32; 10] = [0, 1, 2, 3, 4, 5, 7, 8, 10, 11];
pub(super) fn effect(online: NonNull<c_void>) -> PostfixEffect {
    let mut c = Code::new(online);
    c.emit(&[0x4c, 0x8b, 0x58, FRAME.value]);
    for (index, value) in TYPES.iter().enumerate() {
        c.emit(&[0x49, 0x83, 0xfb, index as u8]);
        let next = c.branch(0x85);
        c.emit(&[0x41, 0xba]);
        c.emit(&value.to_le_bytes());
        c.done();
        c.bind(next);
    }
    c.finish(&SPEC, 0)
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact enum sizes and full HashSet overload validated before forwarding seven native positions.
    unsafe {
        let images = access.images()?;
        for name in [c"IslandType", c"EchoChainType", c"ChainStatus"] {
            access.enum_storage(access.class(&images, c"", name)?, c"Int32", 0x08)?;
        }
        access.enum_storage(access.class(&images, c"", c"WovenEchoType")?, c"Byte", 0x05)?;
        Ok(vec![access.hook_method(&SPEC, module, size)?.postfix(effect(access.hook_online_storage(&images)?))])
    }
}
