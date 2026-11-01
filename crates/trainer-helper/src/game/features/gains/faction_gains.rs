//! Adjust positive favor before the game applies rank scaling, its favor cap and derived reputation. Respect
//! ignoreMultiplier; forward ignoreRepGain and MethodInfo unchanged. Never multiply the derived reputation twice.
use super::*;
use crate::game::registry::{Behavior, HookFeature, MethodSpec, Targets};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::FactionGains],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"Faction",
    method: c"GainFavor",
    instance: true,
    params: &["System.Int32", "System.Boolean", "System.Boolean"],
    returns: "System.Void",
};
pub(crate) fn effect(online: NonNull<c_void>) -> LeafEffect {
    let mut c = LeafCode::for_receiver(online);
    c.emit(&[0x45, 0x84, 0xc9]); // ignoreMultiplier is R9B
    c.exit(0x85);
    c.emit(&[0xf2, 0x0f, 0x10, 0x48, FRAME.answer, 0x4c, 0x63, 0xd2]);
    scale(&mut c, false);
    c.emit(&[0x44, 0x89, 0xd2]);
    c.finish()
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact five-position ABI, immutable metadata and initialized process-owned online byte.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"LE.Factions", c"Faction")?;
        Ok(vec![access.hook_method_on(owner, &SPEC, module, size)?.leaf(
            effect(access.hook_online_storage(&images)?),
            None,
            None,
        )])
    }
}
