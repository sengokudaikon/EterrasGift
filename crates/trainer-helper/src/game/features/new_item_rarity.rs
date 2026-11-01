//! Replace only an unspecified (-1) rarity in GenerateItems.RollBaseItem. All 23 native positions stay in place,
//! including byref outputs, nullable/aggregate payloads, roll modifiers and MethodInfo; only the low Int32 at the
//! ninth native position changes. A tail jump needs no stack copy or maximum-arity gateway. Explicit blessings,
//! uniques and gambler rarities win. The original runs once and owns all generation/output/exception behavior.
use super::{prefix::Code, registry::*};
use crate::{
    game::*,
    hooks::leaf::{FRAME, LeafEffect},
};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::NewItemRarity],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"GenerateItems",
    method: c"RollBaseItem",
    instance: true,
    params: &[
        "ItemDataUnpacked&",
        "System.Boolean&",
        "System.Nullable<PreCorruptItemInfo>&",
        "System.Int32",
        "System.Boolean",
        "System.Int32",
        "System.Int32",
        "System.Int32",
        "ItemLocationTag",
        "System.Int32",
        "System.Int32",
        "GenerateItems.UniqueRerollReplacement",
        "System.Boolean",
        "System.Int32",
        "System.Boolean",
        "UniqueList.LegendaryType",
        "System.Boolean",
        "System.Nullable<System.Int32>",
        "System.Nullable<System.Int32>",
        "System.Nullable<ItemList.AffixEffectiveness>",
        "GenerateItems.ItemRollModifiers",
    ],
    returns: "System.Void",
};
pub(crate) fn effect(online: NonNull<c_void>) -> LeafEffect {
    let mut c = Code::new(online);
    c.emit(&[0x83, 0x7c, 0x24, 0x48, 0xff]); // native position 8: rarity == -1
    c.exit(0x85);
    c.emit(&[0x4c, 0x8b, 0x58, FRAME.answer, 0x44, 0x89, 0x5c, 0x24, 0x48]);
    c.finish()
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact 21-parameter instance void method; byrefs and scalar/aggregate payloads are forwarded in place.
    unsafe {
        let images = access.images()?;
        Ok(vec![access.hook_method(&SPEC, module, size)?.leaf(
            effect(access.hook_online_storage(&images)?),
            None,
            None,
        )])
    }
}
