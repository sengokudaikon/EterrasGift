//! Refill rune/glyph stacks in one-slot containers before original quantity queries. Other item types are unchanged.
use super::{
    materials::{self, Code, FLOOR},
    registry::{Behavior, GameplayHook, GameplayHookTarget, HookFeature, MethodSpec, Targets},
};
use crate::game::*;
use native_hooks::leaf::LeafEffect;

pub(super) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::RunesGlyphsTo99],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};

#[derive(Clone, Copy)]
pub(crate) struct SlotLayout {
    pub online: NonNull<c_void>,
    pub content: u32,
    pub data: u32,
    pub item_type: u32,
    pub quantity: u32,
}
impl SlotLayout {
    /// A live one-slot receiver retains readable content/data and an aligned writable quantity during the call.
    /// ItemData.itemType is the metadata-validated byte; 102/103 are the game's rune/glyph categories.
    pub(crate) fn effect(self) -> LeafEffect {
        let mut c = Code::new(self.online);
        c.offset(&[0x4c, 0x8b, 0x81], self.content); // r8 = entry
        c.emit(&[0x4d, 0x85, 0xc0]);
        c.exit(0x84);
        c.offset(&[0x4d, 0x8b, 0x88], self.data); // r9 = item
        c.emit(&[0x4d, 0x85, 0xc9]);
        c.exit(0x84);
        c.offset(&[0x41, 0x80, 0xb9], self.item_type);
        c.emit(&[102]);
        let rune = c.branch(0x84);
        c.offset(&[0x41, 0x80, 0xb9], self.item_type);
        c.emit(&[103]);
        c.exit(0x85);
        c.bind(rune);
        c.offset(&[0x41, 0x83, 0xb8], self.quantity);
        c.emit(&[FLOOR]);
        c.exit(0x8d);
        c.offset(&[0x41, 0xc7, 0x80], self.quantity);
        c.emit(&i32::from(FLOOR).to_le_bytes());
        c.finish()
    }
}

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata, admitted offline character and retained module; no current inventory read.
    unsafe {
        let images = access.images()?;
        let slot = access.class(&images, c"", c"OneSlotItemContainer")?;
        let entry = access.class(&images, c"", c"ItemContainerEntry")?;
        let item = access.class(&images, c"", c"ItemData")?;
        let layout = SlotLayout {
            online: access.hook_online_storage(&images)?,
            content: access.hook_field_of(slot, c"content", entry, 8)?,
            data: access.hook_field_of(entry, c"data", item, 8)?,
            item_type: access.hook_field_of(item, c"itemType", access.system_class(c"Byte")?, 1)?,
            quantity: materials::quantity_offset(access, entry, c"quantity")?,
        };
        let method = access.hook_method(
            &MethodSpec {
                class: c"OneSlotItemContainer",
                method: c"GetQuantity",
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
