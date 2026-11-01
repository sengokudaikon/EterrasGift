//! Widen each minimap reveal to at least 1000. The 12-byte float3 is passed indirectly in RDX on Windows x64 and
//! remains opaque: its address and contents reach the original unchanged, along with receiver and MethodInfo.
//! The original still reveals the map; switching off does not hide areas already revealed.
use super::{prefix::Code, registry::*};
use crate::{game::*, hooks::leaf::LeafEffect};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::RevealMap],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"Minimap",
    method: c"UpdateFoW_Singular",
    instance: true,
    params: &["Unity.Mathematics.float3", "System.Single"],
    returns: "System.Void",
};
pub(crate) fn effect(online: NonNull<c_void>) -> LeafEffect {
    let mut c = Code::new(online);
    c.emit(&[0x41, 0xbb]);
    c.emit(&1000.0f32.to_bits().to_le_bytes());
    c.emit(&[0x66, 0x41, 0x0f, 0x6e, 0xc3]); // threshold -> xmm0
    // max(threshold, radius), with radius second, preserves the incoming NaN/signed-zero semantics of std::max(radius, threshold).
    c.emit(&[0xf3, 0x0f, 0x5f, 0xc2, 0xf3, 0x0f, 0x10, 0xd0]);
    c.finish()
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: retained module/statics and exact void signature. The indirect vector is never inspected or copied.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"LE.UI.Minimap", SPEC.class)?;
        Ok(vec![access.hook_method_on(owner, &SPEC, module, size)?.leaf(
            effect(access.hook_online_storage(&images)?),
            None,
            None,
        )])
    }
}
