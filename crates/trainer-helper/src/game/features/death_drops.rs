//! Item/gold overrides and extra rares share DeathItemDrop.OnDeath. Write only its passed ItemDrop component, before
//! tail-calling the original once with Dying and MethodInfo unchanged. Settings remain on that dying enemy because
//! delayed drop work reads them later; turning off affects subsequent deaths, without undoing drops already started.
//!
//! Four f32 settings are copied from retained atomic storage in one version-checked attempt. A paused/changed writer
//! skips that call's rate override, never blocks it or applies a partial snapshot. Extra rare has its own atomic
//! setting. The component's lifetime and serialization belong to the game's OnDeath call, as in the original trainer.
use super::registry::*;
use crate::game::*;
use native_hooks::{
    emit::LeafCode,
    leaf::{FRAME, LeafEffect},
};

pub(crate) const SETTINGS: SharedSettings = SharedSettings(&[
    ControlSetting {
        hook: GameplayHook::DropRates,
        slot: None,
    },
    ControlSetting {
        hook: GameplayHook::ExtraRare,
        slot: Some(1),
    },
]);
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &SETTINGS.controls::<2>(),
    behavior: Behavior::SharedLeaf(&SETTINGS),
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"DeathItemDrop",
    method: c"OnDeath",
    instance: true,
    params: &["Dying"],
    returns: "System.Void",
};

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub override_item: u32,
    pub item_chance: u32,
    pub item_multiplier: u32,
    pub override_gold: u32,
    pub gold_chance: u32,
    pub gold_multiplier: u32,
    pub extra_rare: u32,
}
impl Layout {
    pub(crate) fn effect(self) -> LeafEffect {
        let mut c = LeafCode::for_receiver(self.online);
        c.emit(&[0x48, 0x83, 0x78, FRAME.effect[1], 0xff]);
        let no_rare = c.branch(0x84);
        c.offset(&[0xc6, 0x81], self.extra_rare);
        c.emit(&[1]);
        c.bind(no_rare);
        c.emit(&[0x48, 0x83, 0x78, FRAME.answer, 0xff]);
        c.exit(0x84);
        c.snapshot(FRAME.wide);
        for (flag, chance, multiplier, register) in [
            (self.override_item, self.item_chance, self.item_multiplier, 4),
            (self.override_gold, self.gold_chance, self.gold_multiplier, 5),
        ] {
            c.offset(&[0xc6, 0x81], flag);
            c.emit(&[1]);
            c.offset(&[0xf3, 0x0f, 0x11, 0x81 | (register << 3)], chance);
            c.emit(&[0x66, 0x0f, 0x73, 0xd0 | register, 32]); // psrlq xmm4/5,32
            c.offset(&[0xf3, 0x0f, 0x11, 0x81 | (register << 3)], multiplier);
        }
        c.finish_with([OFF, 0, 0])
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata and retained module/static storage. OnDeath supplies the live derived
    // ItemDrop component; field contracts are checked on its declared base, without invoking game methods.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"", c"ItemDrop")?;
        let death = access.class(&images, c"", SPEC.class)?;
        if !(access.assignable)(owner, death) {
            return Err(unsupported("DeathItemDrop must inherit ItemDrop"));
        }
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            override_item: access.hook_field(owner, c"overrideBaseItemDropChance", "System.Boolean", 1)?,
            item_chance: access.hook_field(owner, c"itemDropChance", "System.Single", 4)?,
            item_multiplier: access.hook_field(owner, c"itemMultiplier", "System.Single", 4)?,
            override_gold: access.hook_field(owner, c"overrideBaseGoldDropChance", "System.Boolean", 1)?,
            gold_chance: access.hook_field(owner, c"goldDropChance", "System.Single", 4)?,
            gold_multiplier: access.hook_field(owner, c"goldMultiplier", "System.Single", 4)?,
            extra_rare: access.hook_field(owner, c"guaranteedAdditionalRare", "System.Boolean", 1)?,
        };
        Ok(vec![access.hook_method_on(death, &SPEC, module, size)?.leaf(layout.effect(), None, None)])
    }
}

#[cfg(test)]
pub(crate) mod tests;
