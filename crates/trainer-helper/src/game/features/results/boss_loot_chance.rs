//! Preserve boss/champion loot-group calculation and its arguments, then replace the pre-modifier chance with one.
use super::*;
use crate::game::gameplay::{Behavior, GameplayHook, HookFeature};
use crate::hooks::postfix::FRAME;

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::BossLootChance],
    behavior: Behavior::Postfix,
    targets,
};
const SPEC: MethodSpec = MethodSpec {
    class: c"LootGroup",
    method: c"GetDropChance",
    instance: true,
    params: &["System.Single", "System.Single", "System.Int32", "System.Int32", "System.Int32"],
    returns: "System.Single",
};
pub(super) fn effect(online: NonNull<c_void>) -> PostfixEffect {
    let mut c = Code::new(online);
    c.emit(&[0xf3, 0x0f, 0x10, 0x40, FRAME.slots[1]]);
    c.finish(&SPEC, u64::from(1.0f32.to_bits()))
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: selected exact nested owner, attached metadata and checked seven-position float-return ABI.
    unsafe {
        let images = access.images()?;
        let owner = access.nested(access.class(&images, c"", c"BossLoot")?, c"LootGroup")?;
        Ok(vec![
            access.hook_method_on(owner, &SPEC, module, size)?.postfix(effect(access.hook_online_storage(&images)?)),
        ])
    }
}
