//! Guarantee both experimental-unique rolls: override the chance and bypass the later failure branch. The following
//! item-level test still runs. The class-compatibility and Rune Prison Complex patches own separate ranges in the
//! same method and can be enabled independently. No reward asset or saved item is changed by this feature.
use super::{patches::PatchSpec, registry::*};
use crate::{
    game::*,
    hooks::{
        emit,
        leaf::{FRAME, GeneratedEffect, LeafEffect},
    },
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::ExperimentalUniques],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const CHANCE: MethodSpec = MethodSpec {
    class: c"RunePrisonItemDrops",
    method: c"GetExperimentalUniqueDropChance",
    instance: true,
    params: &["Actor"],
    returns: "System.Single",
};
const PATCHES: &[PatchSpec] = &[PatchSpec::new(
    "Experimental unique fail-to-drop roll",
    MethodSpec {
        class: c"RunePrisonItemDrops",
        method: c"SpawnExperimentalItem",
        instance: true,
        params: &[],
        returns: "System.Void",
    },
    0x1100,
    &[
        0x0f, 0x85, -1, -1, -1, -1, 0x4d, 0x85, 0xff, 0x0f, 0x84, -1, -1, -1, -1, 0x33, 0xd2, 0x49, 0x8b, 0xcf, 0xe8, -1, -1, -1, -1, 0x44, 0x0f, 0xb6,
    ],
    0,
    &[0x90; 6],
)];
pub(crate) fn effect(online: NonNull<c_void>) -> LeafEffect {
    let mut c = emit::Code::default();
    c.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
    c.exit(0x84);
    c.emit(&[0x4c, 0x8b, 0x58, FRAME.effect[0], 0x41, 0x80, 0x3b, 0]);
    c.exit(0x85);
    c.emit(&[0xb8, 0, 0, 0x80, 0x3f, 0x66, 0x0f, 0x6e, 0xc0, 0xc3]); // return 1.0f
    LeafEffect::Generated(GeneratedEffect {
        code: c.finish(),
        slots: [online.as_ptr() as u64, 0, 0, 0],
    })
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata, retained module and process-owned online storage. Both targets resolve before any write.
    unsafe {
        let images = access.images()?;
        let mut targets = vec![access.hook_method(&CHANCE, module, size)?.leaf(effect(access.hook_online_storage(&images)?), None, None)];
        targets.extend(access.patch_targets(PATCHES, module, size)?);
        Ok(targets)
    }
}

#[cfg(test)]
mod tests;
