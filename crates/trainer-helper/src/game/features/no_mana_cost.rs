//! Let the game calculate an ability's cost once, then answer zero only for the current player's mana component.
use super::registry::{Behavior, GameplayHook, GameplayHookTarget, HookFeature, MethodSpec, Targets};
use crate::{
    game::*,
    hooks::postfix::{FRAME, PostfixEffect},
};

pub(super) const FEATURE: HookFeature = HookFeature {
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
        let f = FRAME;
        let mut code = vec![0x4c, 0x8b, 0x58, f.switch, 0x49, 0x83, 0x3b, 0, 0x74, 0]; // closed -> keep original XMM0
        let mut skips = vec![code.len() - 1];
        code.extend([0x4c, 0x8b, 0x58, f.slots[1], 0x41, 0x80, 0x3b, 0, 0x75, 0]); // online
        skips.push(code.len() - 1);
        code.extend([0x48, 0x85, 0xc9, 0x74, 0]); // null receiver
        skips.push(code.len() - 1);
        code.extend([0x4c, 0x8b, 0x58, f.slots[0], 0x4d, 0x8b, 0x1b, 0x4d, 0x85, 0xdb, 0x74, 0]); // current player
        skips.push(code.len() - 1);
        code.extend([0x49, 0x3b, 0x8b]); // cmp rcx,[r11+Actor.mana]
        code.extend(self.mana_offset.to_le_bytes());
        code.extend([0x75, 0]);
        skips.push(code.len() - 1);
        code.extend([0x0f, 0x57, 0xc0]); // xorps xmm0,xmm0
        for skip in skips {
            code[skip] = (code.len() - skip - 1) as u8;
        }
        PostfixEffect {
            code,
            slots: [self.player_storage.as_ptr() as u64, self.online_storage.as_ptr() as u64],
            positions: SPEC.positions(),
        }
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
        let field = access.field(actor, c"mana")?;
        let offset = (access.field_offset)(field);
        if (access.class_value)(actor)
            || (access.class_value)(mana)
            || (access.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
            || access.field_class(field, "mana cost")? != mana
            || !(16..=16384).contains(&offset)
            || !offset.is_multiple_of(8)
        {
            return Err(unsupported("player mana reference layout"));
        }
        let finder = access.class(&images, c"", c"PlayerFinder")?;
        let player = access.static_field(finder, c"playerActorComponent", "Actor")?;
        let environment = access.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
        let online = access.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
        let layout = ManaCostLayout {
            player_storage: access.hook_static_storage(finder, player, 8)?,
            online_storage: access.hook_static_storage(environment, online, 1)?,
            mana_offset: offset as u32,
        };
        let method = access.hook_method(&SPEC, module, size)?;
        Ok(vec![method.postfix(layout.effect())])
    }
}

#[cfg(test)]
mod tests;
