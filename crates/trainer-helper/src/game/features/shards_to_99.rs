//! Raise existing saved shard quantities before the game's count, preserving its original work and result.
use super::{
    materials::{self, Code, FLOOR, MAX_ENTRIES},
    registry::{Behavior, GameplayHook, GameplayHookTarget, HookFeature, MethodSpec, Targets},
};
use crate::{game::*, hooks::leaf::LeafEffect};

pub(super) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::ShardsTo99],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};

#[derive(Clone, Copy)]
pub(crate) struct ShardLayout {
    pub online: NonNull<c_void>,
    pub stash: u32,
    pub shards: u32,
    pub items: u32,
    pub size: u32,
    pub quantity: u32,
}
impl ShardLayout {
    /// The supported call retains a readable tracker/stash/List<SavedShard>/array graph and writable shard Int32s.
    /// Inventory mutation is serialized by the game. Array headers use the supported IL2CPP x64 layout.
    pub(crate) fn effect(self) -> LeafEffect {
        let mut c = Code::new(self.online);
        c.offset(&[0x4c, 0x8b, 0x81], self.stash); // r8 = tracker.stash
        c.emit(&[0x4d, 0x85, 0xc0]);
        c.exit(0x84);
        c.offset(&[0x4d, 0x8b, 0x80], self.shards); // r8 = stash.SavedShards
        c.emit(&[0x4d, 0x85, 0xc0]);
        c.exit(0x84);
        c.offset(&[0x45, 0x8b, 0x88], self.size); // r9d = active size
        c.emit(&[0x45, 0x85, 0xc9]);
        c.exit(0x8e); // empty/negative
        c.offset(&[0x4d, 0x8b, 0x80], self.items); // r8 = backing array
        c.emit(&[0x4d, 0x85, 0xc0]);
        c.exit(0x84);
        c.emit(&[0x49, 0x81, 0x78, 0x18]); // bound array capacity, as the shared collection reader does
        c.emit(&MAX_ENTRIES.to_le_bytes());
        c.exit(0x87);
        c.emit(&[0x4d, 0x3b, 0x48, 0x18, 0x4d, 0x0f, 0x47, 0x48, 0x18]); // min(size,capacity), like OG
        c.emit(&[0x4d, 0x85, 0xc9]);
        c.exit(0x84);
        c.emit(&[0x49, 0x83, 0xc0, 0x20]); // r8 = first element slot
        let top = c.bytes.len();
        c.emit(&[0x4d, 0x8b, 0x10, 0x4d, 0x85, 0xd2]); // r10 = shard
        let null = c.branch(0x84);
        c.offset(&[0x41, 0x83, 0xba], self.quantity);
        c.emit(&[FLOOR]);
        let enough = c.branch(0x8d); // signed quantity >= 99
        c.offset(&[0x41, 0xc7, 0x82], self.quantity);
        c.emit(&i32::from(FLOOR).to_le_bytes());
        c.bind(null);
        c.bind(enough);
        c.emit(&[0x49, 0x83, 0xc0, 8, 0x49, 0xff, 0xc9]);
        let again = c.branch(0x85);
        c.bind_at(again, top);
        c.finish() // tail-call the original exactly once; no helper return address
    }
}

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata, admitted offline character and retained module. No object is read here.
    unsafe {
        let images = access.images()?;
        let tracker = access.class(&images, c"", c"GlobalDataTracker")?;
        let stash = access.class(&images, c"LE.Data", c"Stash")?;
        let shard = access.class(&images, c"LE.Data", c"SavedShard")?;
        let saved = access.field(stash, c"<SavedShards>k__BackingField")?;
        let list = access.field_class(saved, "shards")?;
        access.list_class(list)?;
        let items = access.field(list, c"_items")?;
        let array = access.field_class(items, "shards")?;
        access.array_class(array, shard)?;
        if (access.class_value)(shard) {
            return Err(unsupported("saved shards must be references"));
        }
        let layout = ShardLayout {
            online: access.hook_online_storage(&images)?,
            stash: access.hook_field_of(tracker, c"<stash>k__BackingField", stash, 8)?,
            shards: access.hook_field_of(stash, c"<SavedShards>k__BackingField", list, 8)?,
            items: access.hook_field_of(list, c"_items", array, 8)?,
            size: access.hook_field_of(list, c"_size", access.system_class(c"Int32")?, 4)?,
            quantity: materials::quantity_offset(access, shard, c"<Quantity>k__BackingField")?,
        };
        let method = access.hook_method(
            &MethodSpec {
                class: c"GlobalDataTracker",
                method: c"GetShardCount",
                instance: true,
                params: &[],
                returns: "System.Int32",
            },
            module,
            size,
        )?;
        Ok(vec![method.leaf(layout.effect(), None, None)])
    }
}
