//! Scale the player's calculated ability speed; downstream fixed-speed mutators remain the game's decision.
use super::*;
use crate::game::registry::{Behavior, GameplayHook, HookFeature, Targets};
use native_hooks::postfix::FRAME;

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::AbilitySpeed],
    behavior: Behavior::Postfix,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"UsingAbility",
    method: c"getSpeedMultiplierStat",
    instance: true,
    params: &["AbilityInfo", "AT", "System.Single"],
    returns: "System.Single",
};
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub player: NonNull<c_void>,
    pub controller: u32,
    pub actor: u32,
}
impl Layout {
    pub(crate) fn effect(self) -> PostfixEffect {
        let mut c = PostfixCode::new(self.online);
        c.nonnull(&[0x48, 0x85, 0xc9]);
        c.reference(FRAME.slots[1]);
        c.offset(&[0x4c, 0x8b, 0x81], self.controller);
        c.nonnull(&[0x4d, 0x85, 0xc0]);
        c.offset(&[0x4d, 0x3b, 0x98], self.actor);
        c.exit(0x85);
        c.emit(&[0x66, 0x0f, 0xef, 0xc9, 0x66, 0x0f, 0x2e, 0x48, FRAME.value]); // 0 >= multiplier
        c.exit(0x83);
        c.emit(&[0xf3, 0x0f, 0x5a, 0xc0, 0xf2, 0x0f, 0x59, 0x40, FRAME.value, 0xf2, 0x0f, 0x5a, 0xc0]);
        c.finish(SPEC.positions(), self.player.as_ptr() as u64)
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: retained live state/controller/player graph after original; metadata checks precede publishing offsets.
    unsafe {
        let images = access.images()?;
        let state = access.class(&images, c"", c"State")?;
        let controller = access.class(&images, c"", c"StateController")?;
        access.enum_storage(access.class(&images, c"", c"AT")?, c"Int32", raw::type_code::I4)?;
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            player: access.hook_player_storage(&images)?,
            controller: access.hook_field(state, c"controller", "StateController", 8)?,
            actor: access.hook_field(controller, c"actor", "Actor", 8)?,
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.postfix(layout.effect())])
    }
}
