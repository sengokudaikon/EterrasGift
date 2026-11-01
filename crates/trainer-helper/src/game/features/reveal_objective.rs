//! Raise the score passed to the existing Monolith pulse event to at least its threshold. The game keeps control
//! of the score-event/first-kill trigger and objective visibility. Read only the pulse's metadata-checked field;
//! lastKill and MethodInfo are forwarded unchanged, and already revealed objectives stay revealed after off.
use super::registry::*;
use crate::game::*;
use native_hooks::{emit::LeafCode, leaf::LeafEffect};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::RevealObjective],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"MonolithPulse",
    method: c"OnScoreChanged",
    instance: true,
    params: &["System.Single", "Dying"],
    returns: "System.Void",
};
pub(crate) fn effect(online: NonNull<c_void>, threshold: u32) -> LeafEffect {
    let mut c = LeafCode::new(online);
    c.nonnull(&[0x48, 0x85, 0xc9]);
    c.offset(&[0xf3, 0x0f, 0x10, 0x81], threshold);
    c.emit(&[0xf3, 0x0f, 0x5f, 0xc1, 0xf3, 0x0f, 0x10, 0xc8]);
    c.finish()
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: the live pulse retains its threshold during this leaf; exact field type/alignment and native signature are checked.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"LE.Gameplay.Monolith.Frontend", SPEC.class)?;
        let threshold = access.hook_field(owner, c"scoreThreshold", "System.Single", 4)?;
        Ok(vec![access.hook_method_on(owner, &SPEC, module, size)?.leaf(
            effect(access.hook_online_storage(&images)?, threshold),
            None,
            None,
        )])
    }
}
