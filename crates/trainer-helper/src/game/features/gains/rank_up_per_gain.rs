//! Replace a positive reputation gain with the positive remainder to this rank's requirement, even when that reduces
//! the incoming gain. Missing data, an invalid/top rank or an invalid remainder forwards unchanged. The original
//! retains joined-faction checks, rank transitions, notifications and saves. References belong to the live faction
//! graph for the call; array length/layout follows the retained IL2CPP reference-array contract.
use super::*;
use crate::game::registry::{Behavior, HookFeature, MethodSpec, Targets};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::RankUpPerGain],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"Faction",
    method: c"GainReputation",
    instance: true,
    params: &["System.Int32"],
    returns: "System.Void",
};
#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub online: NonNull<c_void>,
    pub rank: u32,
    pub reputation: u32,
    pub data: u32,
    pub requirements: u32,
    pub required: u32,
}
impl Layout {
    pub(super) fn effect(self) -> LeafEffect {
        let mut c = Code::new(self.online);
        c.emit(&[0x85, 0xd2]);
        c.exit(0x8e);
        c.offset(&[0x4c, 0x63, 0x91], self.rank); // signed rank -> r10
        c.emit(&[0x4d, 0x85, 0xd2]);
        c.exit(0x88);
        c.offset(&[0x4c, 0x8b, 0x99], self.data);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4d, 0x8b, 0x9b], self.requirements);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.emit(&[0x4d, 0x3b, 0x53, 0x18]); // rank < array length
        c.exit(0x83);
        c.emit(&[0x4f, 0x8b, 0x5c, 0xd3, 0x20]); // ranks[rank] -> r11
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4d, 0x63, 0x93], self.required);
        c.offset(&[0x4c, 0x63, 0x99], self.reputation);
        c.emit(&[0x4d, 0x29, 0xda, 0x4d, 0x85, 0xd2]); // 64-bit remainder avoids malformed-field overflow
        c.exit(0x8e);
        c.emit(&[0x49, 0x81, 0xfa, 0xff, 0xff, 0xff, 0x7f]);
        c.exit(0x8f);
        c.emit(&[0x44, 0x89, 0xd2]);
        c.finish([0; 3])
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: exact three-position ABI; immutable field descriptors are checked before publishing this layout.
    unsafe {
        let images = access.images()?;
        let faction = access.class(&images, c"LE.Factions", c"Faction")?;
        let data = access.class(&images, c"LE.Factions", c"FactionData")?;
        let rank = access.nested(data, c"RankData")?;
        let requirements = access.field(data, c"reputationReqForNextRanks")?;
        let array = access.field_class(requirements, "faction rank requirements")?;
        if array.is_null() || (access.class_element)(array) != rank || (access.class_rank)(array) != 1 {
            return Err(unsupported("faction rank requirements array"));
        }
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            rank: access.hook_field(faction, c"<Rank>k__BackingField", "System.Int32", 4)?,
            reputation: access.hook_field(faction, c"<Reputation>k__BackingField", "System.Int32", 4)?,
            data: access.hook_field_of(faction, c"<Data>k__BackingField", data, 8)?,
            requirements: access.hook_field_of(data, c"reputationReqForNextRanks", array, 8)?,
            required: access.hook_field(rank, c"reputationRequired", "System.Int32", 4)?,
        };
        Ok(vec![access.hook_method_on(faction, &SPEC, module, size)?.leaf(layout.effect(), None, None)])
    }
}
