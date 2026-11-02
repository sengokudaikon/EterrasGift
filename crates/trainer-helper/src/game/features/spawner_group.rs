//! Independent pack-size and rarity controls on the same spawner call. The game's main thread serializes each
//! spawner; separate receivers and recursive calls use separate stack snapshots. Restore all original fields on
//! return or native unwind, so repeated generation never compounds. Single monsters retain their count of one.
use super::registry::*;
use crate::game::*;
use native_hooks::{
    emit::Code,
    postfix::{
        FRAME, PostfixEffect,
        scope::{Scope, ScopedEffect},
    },
};

pub(crate) const SETTINGS: SharedSettings = SharedSettings(&[
    ControlSetting {
        hook: GameplayHook::MonsterRarity,
        slot: None,
    },
    ControlSetting {
        hook: GameplayHook::PackSize,
        slot: Some(1),
    },
]);
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &SETTINGS.controls::<2>(),
    behavior: Behavior::SharedPostfix(&SETTINGS),
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"Spawner",
    method: c"GenerateEntitiesInternal",
    instance: true,
    params: &[],
    returns: "System.Void",
};
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub count: u32,
    pub weights: [u32; 3],
}
impl Layout {
    pub(crate) fn effect(self) -> ScopedEffect {
        let fields = vec![self.count, self.weights[0], self.weights[1], self.weights[2]];
        let mut c = Code::default();
        c.gated(FRAME.switch, FRAME.slots[0]);
        c.nonnull(&[0x48, 0x85, 0xc9]);
        c.snapshot(FRAME.wide);
        Scope::new(SPEC.positions(), fields.len()).save(&mut c, &fields);
        c.emit(&[0x48, 0x83, 0x78, FRAME.value, 0xff]);
        let no_rarity = c.branch(0x84);
        for (index, field) in self.weights.into_iter().enumerate() {
            let reg = if index == 2 { 5 } else { 4 };
            if index == 1 {
                c.emit(&[0x66, 0x0f, 0x73, 0xdc, 4]);
            } // shift Magic into low f32
            c.offset(&[0xf3, 0x0f, 0x59, 0x81 | (reg << 3)], field);
            c.offset(&[0xf3, 0x0f, 0x11, 0x81 | (reg << 3)], field);
        }
        c.bind(no_rarity);
        c.emit(&[0x48, 0x83, 0x78, FRAME.slots[1], 0xff]);
        c.exit(0x84);
        c.offset(&[0xf3, 0x0f, 0x10, 0xa1], self.count);
        c.emit(&[0x41, 0xbb]);
        c.emit(&1f32.to_bits().to_le_bytes());
        c.emit(&[0x66, 0x41, 0x0f, 0x6e, 0xeb, 0x0f, 0x2e, 0xe5]);
        c.exit(0x86); // only count > 1
        c.emit(&[0xf2, 0x0f, 0x5a, 0x68, FRAME.slots[1], 0xf3, 0x0f, 0x59, 0xe5]);
        c.offset(&[0xf3, 0x0f, 0x11, 0xa1], self.count);
        ScopedEffect {
            effect: PostfixEffect {
                before: c.finish(),
                code: vec![],
                slots: [self.online.as_ptr() as u64, OFF],
                positions: SPEC.positions(),
            },
            fields,
        }
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata and retained static storage; check each scalar's type/alignment.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"", c"Spawner")?;
        let field = |name| access.hook_field(owner, name, "System.Single", 4);
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            count: field(c"numberToSpawn")?,
            weights: [field(c"commonWeighting")?, field(c"magicWeighting")?, field(c"rareWeighting")?],
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.scoped(layout.effect())])
    }
}

#[cfg(test)]
pub(crate) mod tests;
