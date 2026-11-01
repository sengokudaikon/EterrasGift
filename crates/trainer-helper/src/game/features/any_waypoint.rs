//! Offer travel for inactive eligible waypoints, then bypass the two matching travel rejection branches. The
//! seven-character/no-waypoint exclusion is the original trainer's rule, not a blanket unlock of every scene.
//! Only the two availability arguments change; the remaining nine-position ABI and original call stay intact.
//! The caller retains its waypoint/string graph for the call. No object state or saved unlock is changed here.
use super::{patches::PatchSpec, registry::*};
use crate::game::*;
use native_hooks::{emit::LeafCode, leaf::LeafEffect};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::AnyWaypoint],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SELECT: MethodSpec = MethodSpec {
    class: c"MapPanel",
    method: c"SelectLocation",
    instance: true,
    params: &[
        "System.String",
        "System.Boolean",
        "System.Boolean",
        "UIWaypoint",
        "System.Boolean",
        "System.Boolean",
        "System.Int32",
    ],
    returns: "System.Void",
};
const TRAVEL: MethodSpec = MethodSpec {
    class: c"UIWaypointStandard",
    method: c"LoadWaypointScene",
    instance: true,
    params: &[],
    returns: "System.Void",
};
const TRAVEL_PATTERN: &[i16] = &[
    0x80, 0x7f, -1, 0, 0x0f, 0x85, -1, -1, -1, -1, 0x80, 0x7f, -1, 0, 0x0f, 0x84, -1, -1, -1, -1, 0x80, 0x7f, -1, 0, 0x0f, 0x85, -1, -1, -1, -1, 0x48, 0x8b,
];
const PATCHES: &[PatchSpec] = &[
    PatchSpec::new("Waypoint presence travel check", TRAVEL, 0x400, TRAVEL_PATTERN, 4, &[0x90; 6]),
    PatchSpec::new("Waypoint unlock travel check", TRAVEL, 0x400, TRAVEL_PATTERN, 14, &[0x90; 6]),
];

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub scene: u32,
    pub no_waypoint: u32,
    pub active: u32,
}
impl Layout {
    pub(crate) fn effect(self) -> LeafEffect {
        let mut c = LeafCode::new(self.online);
        c.emit(&[0x4c, 0x8b, 0x5c, 0x24, 0x28]); // fifth native position: waypoint
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x41, 0x80, 0xbb], self.active);
        c.emit(&[0]);
        c.exit(0x85);
        c.offset(&[0x41, 0x80, 0xbb], self.no_waypoint);
        c.emit(&[0]);
        let eligible = c.branch(0x84);
        c.offset(&[0x4d, 0x8b, 0x9b], self.scene);
        c.emit(&[0x4d, 0x85, 0xdb]);
        let unnamed = c.branch(0x84);
        c.emit(&[0x41, 0x83, 0x7b, 0x10, 7]); // supported IL2CPP String.length
        c.exit(0x84);
        c.bind(eligible);
        c.bind(unnamed);
        c.emit(&[0x41, 0xb8, 1, 0, 0, 0, 0x41, 0xb9, 1, 0, 0, 0]);
        c.finish()
    }
}

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata, initialized online static and retained module; fields/ABI checked before publication.
    unsafe {
        let images = access.images()?;
        let waypoint = access.class(&images, c"", c"UIWaypoint")?;
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            scene: access.hook_field(waypoint, c"sceneName", "System.String", 8)?,
            no_waypoint: access.hook_field(waypoint, c"noWaypointInScene", "System.Boolean", 1)?,
            active: access.hook_field(waypoint, c"isActive", "System.Boolean", 1)?,
        };
        let mut targets = vec![access.hook_method(&SELECT, module, size)?.leaf(layout.effect(), None, None)];
        targets.extend(access.patch_targets(PATCHES, module, size)?);
        // Bind the travel recipe's field displacements to the same discovered waypoint layout.
        let TargetEffect::Patch(change) = &targets[1].effect else { unreachable!() };
        if layout.no_waypoint > 127
            || layout.active > 127
            || change.address.as_ptr().sub(2).read() != layout.no_waypoint as u8
            || change.address.as_ptr().add(8).read() != layout.active as u8
        {
            return Err(unsupported("waypoint travel field displacement"));
        }
        Ok(targets)
    }
}

#[cfg(test)]
mod tests;
