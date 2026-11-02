//! Scale room density only during the game's placement call. Its placement algorithm and 100-spawner cap remain
//! intact. Unity serializes a room's placement; each call (including recursion) owns its restoration record, and
//! native unwind restores it without entering the helper. The two-bool config occupies DX in the supported build.
use super::registry::*;
use crate::game::*;
use native_hooks::{
    emit::Code,
    postfix::{
        FRAME, PostfixEffect,
        scope::{Scope, ScopedEffect},
    },
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::PackCount],
    behavior: Behavior::Postfix,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"SpawnerPlacementRoom",
    method: c"rollSpawners",
    instance: true,
    params: &["SpawnerRuntimeConfig"],
    returns: "System.Void",
};
pub(crate) fn effect(online: NonNull<c_void>, density: u32) -> ScopedEffect {
    let fields = vec![density];
    let mut c = Code::default();
    c.gated(FRAME.switch, FRAME.slots[0]);
    c.nonnull(&[0x48, 0x85, 0xc9]);
    Scope::new(SPEC.positions(), fields.len()).save(&mut c, &fields);
    c.offset(&[0xf3, 0x0f, 0x10, 0xa1], density); // current -> xmm4
    c.emit(&[0x41, 0xbb]);
    c.emit(&1f32.to_bits().to_le_bytes());
    c.emit(&[0x66, 0x41, 0x0f, 0x6e, 0xeb, 0xf3, 0x0f, 0x58, 0xe5]); // +1
    c.emit(&[0xf2, 0x0f, 0x5a, 0x68, FRAME.value, 0xf3, 0x0f, 0x59, 0xe5]);
    c.emit(&[0x66, 0x41, 0x0f, 0x6e, 0xeb, 0xf3, 0x0f, 0x5c, 0xe5]); // -1
    c.offset(&[0xf3, 0x0f, 0x11, 0xa1], density);
    ScopedEffect {
        effect: PostfixEffect {
            before: c.finish(),
            code: vec![],
            slots: [online.as_ptr() as u64, 0],
            positions: SPEC.positions(),
        },
        fields,
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata; validate the exact value aggregate layout and room field before publication.
    unsafe {
        let images = access.images()?;
        let config = access.class(&images, c"", c"SpawnerRuntimeConfig")?;
        if !(access.class_value)(config) {
            return Err(unsupported("spawner config value type"));
        }
        for (index, name) in [c"allowAmortizedSpawn", c"allowPlacementOffNavmesh"].into_iter().enumerate() {
            let field = access.field(config, name)?;
            let ty = (access.field_type)(field);
            if ty.is_null()
                || (access.type_byref)(ty)
                || access.type_name(ty)? != "System.Boolean"
                || (access.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
                || (access.field_offset)(field) != 16 + index
            {
                return Err(unsupported("two-byte spawner config"));
            }
        }
        let owner = access.class(&images, c"", c"SpawnerPlacementRoom")?;
        let density = access.hook_field(owner, c"spawnerDensityModifier", "System.Single", 4)?;
        Ok(vec![access.hook_method(&SPEC, module, size)?.scoped(effect(access.hook_online_storage(&images)?, density))])
    }
}

#[cfg(test)]
pub(crate) mod tests;
