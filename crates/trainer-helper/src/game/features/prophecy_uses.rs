//! Keep completed prophecy charges after a successful reward trigger, before the caller saves the slot. Snapshot
//! only admitted calls, restore only a decrease, and preserve the Boolean result and the actual chargesConsumed out
//! parameter. Like the original trainer, a native exception skips restoration; it propagates without retry. All
//! code and per-invocation snapshot storage belong to the retained gateway, never the helper module.
use super::registry::*;
use crate::game::*;
use native_hooks::{
    emit::{Code, PostfixCode},
    postfix::{FRAME, PostfixEffect, snapshots},
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::ProphecyUses],
    behavior: Behavior::Postfix,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"ProphecySlot",
    method: c"TryTriggerReward",
    instance: true,
    params: &["LE.Factions.ProphecyTargetType", "System.Boolean", "Actor", "System.Int32&"],
    returns: "System.Boolean",
};
pub(crate) fn effect(online: NonNull<c_void>, charges: u32) -> PostfixEffect {
    let [slot, saved] = snapshots(SPEC.positions()).map(u32::from);
    let mut before = Code::default();
    before.emit(&[0x45, 0x33, 0xdb]);
    before.offset(&[0x4c, 0x89, 0x9c, 0x24], slot); // no snapshot for an unadmitted call
    before.gated(FRAME.switch, FRAME.slots[0]);
    before.nonnull(&[0x48, 0x85, 0xc9]);
    before.offset(&[0x48, 0x89, 0x8c, 0x24], slot);
    before.offset(&[0x44, 0x0f, 0xb6, 0x99], charges);
    before.offset(&[0x4c, 0x89, 0x9c, 0x24], saved);

    let mut c = PostfixCode::new(online);
    c.nonnull(&[0x45, 0x84, 0xd2]); // original Boolean result, not chargesConsumed
    c.offset(&[0x4c, 0x8b, 0x9c, 0x24], slot);
    c.nonnull(&[0x4d, 0x85, 0xdb]);
    c.offset(&[0x44, 0x8b, 0x84, 0x24], saved);
    c.offset(&[0x45, 0x38, 0x83], charges); // current byte >= snapshot: preserve it
    c.exit(0x83);
    c.offset(&[0x45, 0x88, 0x83], charges);
    let mut effect = c.finish(SPEC.positions(), 0);
    effect.before = before.finish();
    effect
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: immutable metadata and retained module/statics; the game retains its slot through the original call.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"LE.Factions", SPEC.class)?;
        access.enum_storage(access.class(&images, c"LE.Factions", c"ProphecyTargetType")?, c"Int32", raw::type_code::I4)?;
        let charges = access.hook_field(owner, c"<CompletedCharges>k__BackingField", "System.Byte", 1)?;
        Ok(vec![
            access
                .hook_method_on(owner, &SPEC, module, size)?
                .postfix(effect(access.hook_online_storage(&images)?, charges)),
        ])
    }
}

#[cfg(test)]
pub(crate) mod tests;
