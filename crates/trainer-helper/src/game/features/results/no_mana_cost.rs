//! Let the game calculate an ability's cost once, then answer zero only for the current player's mana component.
use super::*;
use crate::game::registry::{Behavior, GameplayHook, HookFeature, Targets};
use crate::hooks::postfix::FRAME;

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::NoManaCost],
    behavior: Behavior::Postfix,
    targets: Targets::Resolve(targets),
};

/// Static player/online storage is process-owned. The supported game retains the actor/mana graph for this call,
/// as the original trainer's post-call owner check does. No cached player component or helper callback is needed.
pub(crate) struct ManaCostLayout {
    pub player_storage: NonNull<c_void>,
    pub online_storage: NonNull<c_void>,
    pub mana_offset: u32,
}
impl ManaCostLayout {
    pub(crate) fn effect(&self) -> PostfixEffect {
        let mut c = Code::new(self.online_storage);
        c.nonnull(&[0x48, 0x85, 0xc9]); // null receiver
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.slots[1], 0x4d, 0x8b, 0x1b]); // r11 = current player
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x49, 0x3b, 0x8b], self.mana_offset); // cmp rcx, [r11 + Actor.mana]
        c.exit(0x85);
        c.emit(&[0x0f, 0x57, 0xc0]); // xorps xmm0, xmm0: no cost
        c.finish(&SPEC, self.player_storage.as_ptr() as u64)
    }
}

const SPEC: MethodSpec = MethodSpec {
    class: c"BaseMana",
    method: c"getManaCost",
    instance: true,
    params: &["Ability", "System.Single", "System.Boolean", "AbilityMutator", "System.Boolean"],
    returns: "System.Single",
};

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached main thread, initialized game statics, retained metadata and bounded module.
    unsafe {
        let images = access.images()?;
        let actor = access.class(&images, c"", c"Actor")?;
        let mana = access.class(&images, c"", c"BaseMana")?;
        if (access.class_value)(mana) {
            return Err(unsupported("player mana reference layout"));
        }
        let offset = access.hook_field_of(actor, c"mana", mana, 8)?;
        let layout = ManaCostLayout {
            player_storage: access.hook_player_storage(&images)?,
            online_storage: access.hook_online_storage(&images)?,
            mana_offset: offset,
        };
        let method = access.hook_method(&SPEC, module, size)?;
        Ok(vec![method.postfix(layout.effect())])
    }
}

#[cfg(test)]
mod tests;
