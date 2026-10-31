//! Refill the current player's mana from its Int32 maximum before each original tick. The game still regenerates,
//! drains and notifies normally, so the post-tick value may be below maximum. Disabling keeps the current mana and
//! restores normal ticks. XMM1's delta time and R8's MethodInfo are forwarded exactly; no original is skipped or retried.
use super::*;
use crate::game::registry::{Behavior, GameplayHook, HookFeature, MethodSpec, Targets};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::RefillMana],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"BaseMana",
    method: c"OnUpdateTick",
    instance: true,
    params: &["System.Single"],
    returns: "System.Void",
};
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub receiver: Receiver,
    pub maximum: u32,
    pub current: u32,
}
impl Layout {
    pub(crate) fn effect(self) -> LeafEffect {
        self.receiver.effect(|c| {
            c.offset(&[0xf3, 0x0f, 0x2a, 0x81], self.maximum); // cvtsi2ss xmm0,dword [rcx+maximum]
            c.offset(&[0xf3, 0x0f, 0x11, 0x81], self.current); // movss [rcx+current],xmm0
        })
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: checked mutable primitive destination and retained actor/mana graph for the exact three-position ABI.
    unsafe {
        let images = access.images()?;
        let actor = access.class(&images, c"", c"Actor")?;
        let mana = access.class(&images, c"", c"BaseMana")?;
        access
            .mutable_field(mana, c"currentMana", c"Single", 0x0c)
            .map_err(|error| unsupported(error.to_string()))?;
        let receiver = Receiver {
            online: access.hook_online_storage(&images)?,
            storage: access.hook_player_storage(&images)?,
            component: Some(access.hook_field_of(actor, c"mana", mana, 8)?),
        };
        let layout = Layout {
            receiver,
            maximum: access.hook_field(mana, c"maxMana", "System.Int32", 4)?,
            current: access.hook_field(mana, c"currentMana", "System.Single", 4)?,
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.leaf(layout.effect(), None, None)])
    }
}
