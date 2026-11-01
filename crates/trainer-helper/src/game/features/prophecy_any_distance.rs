//! Combat prophecy targets count at any distance. This bypasses only IsPlayerInRangeOf, not prophecy target or
//! faction requirements. Its 12-byte Vector3 travels indirectly in RDX; off/online paths tail-call the original
//! without changing or dereferencing it. On returns true without a game call, matching the original trainer.
use super::registry::*;
use crate::game::*;
use native_hooks::{emit::LeafCode, leaf::LeafEffect};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::ProphecyAnyDistance],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"CircleOfFortune",
    method: c"IsPlayerInRangeOf",
    instance: true,
    params: &["UnityEngine.Vector3"],
    returns: "System.Boolean",
};
pub(crate) fn effect(online: NonNull<c_void>) -> LeafEffect {
    let mut c = LeafCode::new(online);
    c.emit(&[0xb8, 1, 0, 0, 0, 0xc3]);
    c.finish()
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact Boolean return signature with an opaque indirect value argument and process-owned online byte.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"LE.Factions", SPEC.class)?;
        Ok(vec![access.hook_method_on(owner, &SPEC, module, size)?.leaf(
            effect(access.hook_online_storage(&images)?),
            None,
            None,
        )])
    }
}
