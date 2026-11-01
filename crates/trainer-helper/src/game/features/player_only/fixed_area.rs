//! Override increased area only for Actor.stats of the current player. Skip the original calculation for that
//! receiver; minions, enemies and replaced player components retain their own calculation. The setting is an f64
//! converted to the target's f32 result; tags and MethodInfo reach every forwarded original unchanged.
use super::*;
use crate::game::registry::{Behavior, GameplayHook, HookFeature, MethodSpec, Targets};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::FixedArea],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"Stats",
    method: c"GetIncreasedAreaForAreaSkill",
    instance: true,
    params: &["AT"],
    returns: "System.Single",
};
pub(crate) fn effect(receiver: Receiver) -> LeafEffect {
    receiver.effect(|c| c.emit(&[0xf2, 0x0f, 0x5a, 0x40, FRAME.answer, 0xc3])) // cvtsd2ss xmm0,[rax+answer]; ret
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact three-position scalar ABI; checked actor/stats reference and initialized player static.
    unsafe {
        let images = access.images()?;
        let actor = access.class(&images, c"", c"Actor")?;
        let stats = access.class(&images, c"", c"BaseStats")?;
        access.enum_storage(access.class(&images, c"", c"AT")?, c"Int32", raw::type_code::I4)?;
        let receiver = Receiver {
            online: access.hook_online_storage(&images)?,
            storage: access.hook_player_storage(&images)?,
            component: Some(access.hook_field_of(actor, c"stats", stats, 8)?),
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.leaf(effect(receiver), None, None)])
    }
}
