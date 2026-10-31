//! Independent Loot Lizard and Rune Warden counts on the shared encounter calculation. Snapshot the placement's
//! context and each done/enable flag in invocation-local gateway home slots, call the original once, then modify
//! only newly completed calculations on that same context. Previously consumed encounters are never replenished.
//! Nested calls have separate snapshots; original exceptions unwind normally and run no post-call writes. The game
//! retains and serializes its placement/context/config graph for the call, as the original trainer assumes.
use super::{postfix::Code, registry::*};
use crate::{
    game::*,
    hooks::{
        emit,
        postfix::{FRAME, PostfixEffect, snapshots},
    },
};

pub(crate) const SETTINGS: SharedSettings = SharedSettings(&[
    ControlSetting {
        hook: GameplayHook::LootLizards,
        slot: None,
    },
    ControlSetting {
        hook: GameplayHook::RuneWardens,
        slot: Some(1),
    },
]);
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &SETTINGS.controls::<2>(),
    behavior: Behavior::SharedPostfix(&SETTINGS),
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"RandomEncounterPlacer",
    method: c"HandleEncountersCalculations",
    instance: false,
    params: &[
        "LE.PCG.Level",
        "RandomEncounterTypes.EncounterPlacementData",
        "RandomEncounterData",
        "LE.Factions.TheWeaver",
    ],
    returns: "System.Void",
};
#[derive(Clone, Copy)]
pub(crate) struct Counts {
    pub done: u32,
    pub total: u32,
    pub remaining: u32,
    pub multiplier: u32,
}
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub context: u32,
    pub scene: u32,
    pub config: u32,
    pub rate: u32,
    pub counts: [Counts; 2],
}
impl Layout {
    pub(crate) fn effect(self) -> PostfixEffect {
        let [context, flags] = snapshots(SPEC.positions()).map(u32::from);
        let mut before = emit::Code::default();
        before.emit(&[0x45, 0x33, 0xdb]);
        before.offset(&[0x4c, 0x89, 0x9c, 0x24], context); // no snapshot unless admitted below
        before.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
        before.exit(0x84);
        before.emit(&[0x4c, 0x8b, 0x58, FRAME.slots[0], 0x41, 0x80, 0x3b, 0]);
        before.exit(0x85);
        before.nonnull(&[0x48, 0x85, 0xd2]);
        before.offset(&[0x4c, 0x8b, 0x9a], self.context);
        before.nonnull(&[0x4d, 0x85, 0xdb]);
        before.offset(&[0x4c, 0x89, 0x9c, 0x24], context);
        before.offset(&[0x45, 0x0f, 0xb6, 0x93], self.counts[0].done);
        before.offset(&[0x4c, 0x89, 0x94, 0x24], flags);
        before.offset(&[0x45, 0x0f, 0xb6, 0x93], self.counts[1].done);
        before.offset(&[0x44, 0x88, 0x94, 0x24], flags + 1);
        for (index, word) in [FRAME.value, FRAME.slots[1]].into_iter().enumerate() {
            before.emit(&[0x48, 0x83, 0x78, word, 0xff]);
            before.offset(&[0x0f, 0x95, 0x84, 0x24], flags + 2 + index as u32);
        }

        let mut c = Code::new(self.online);
        c.offset(&[0x4c, 0x8b, 0x9c, 0x24], context);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4c, 0x39, 0x9a], self.context); // restored RDX is placement, not the level in RCX
        c.exit(0x85);
        c.offset(&[0x4c, 0x8b, 0x82], self.scene);
        c.nonnull(&[0x4d, 0x85, 0xc0]);
        c.offset(&[0x4d, 0x8b, 0x80], self.config);
        c.nonnull(&[0x4d, 0x85, 0xc0]);
        c.offset(&[0xf3, 0x41, 0x0f, 0x10, 0x90], self.rate);
        c.emit(&[0x0f, 0x57, 0xc9, 0x0f, 0x2e, 0xd1]);
        let unordered = c.branch(0x8a);
        let nonzero = c.branch(0x85);
        c.emit(&[0x41, 0xb8]);
        c.emit(&0.0001f32.to_bits().to_le_bytes());
        c.emit(&[0x66, 0x41, 0x0f, 0x6e, 0xd0]);
        c.bind(unordered);
        c.bind(nonzero);
        for (index, word) in [FRAME.value, FRAME.slots[1]].into_iter().enumerate() {
            c.offset(&[0x80, 0xbc, 0x24], flags + index as u32);
            c.emit(&[0]);
            let already_done = c.branch(0x85);
            c.offset(&[0x80, 0xbc, 0x24], flags + 2 + index as u32);
            c.emit(&[0]);
            let was_off = c.branch(0x84);
            let fields = self.counts[index];
            c.offset(&[0x41, 0x80, 0xbb], fields.done);
            c.emit(&[0]);
            let incomplete = c.branch(0x84);
            c.emit(&[0x48, 0x83, 0x78, word, 0xff]);
            let now_off = c.branch(0x84);
            c.emit(&[0xf2, 0x44, 0x0f, 0x2c, 0x50, word]);
            c.offset(&[0x45, 0x89, 0x93], fields.total);
            c.offset(&[0x45, 0x89, 0x93], fields.remaining);
            c.emit(&[0xf3, 0x41, 0x0f, 0x2a, 0xca, 0xf3, 0x0f, 0x5e, 0xca]);
            c.offset(&[0xf3, 0x41, 0x0f, 0x11, 0x8b], fields.multiplier);
            for branch in [already_done, was_off, incomplete, now_off] {
                c.bind(branch);
            }
        }
        let mut effect = c.finish(&SPEC, OFF);
        effect.before = before.finish();
        effect
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata, retained module and initialized online storage. All graph fields are checked before code is generated.
    unsafe {
        let images = access.images()?;
        let types = access.class(&images, c"", c"RandomEncounterTypes")?;
        let placement = access.nested(types, c"EncounterPlacementData")?;
        let context = access.nested(types, c"Context")?;
        let scene = access.nested(types, c"SceneData")?;
        let config = access.nested(types, c"PlacementConfigData")?;
        let counts = |done, total, remaining, multiplier| -> Result<Counts, GameError> {
            Ok(Counts {
                done: access.hook_field(context, done, "System.Boolean", 1)?,
                total: access.hook_field(context, total, "System.Int32", 4)?,
                remaining: access.hook_field(context, remaining, "System.Int32", 4)?,
                multiplier: access.hook_field(context, multiplier, "System.Single", 4)?,
            })
        };
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            context: access.hook_field_of(placement, c"<Context>k__BackingField", context, 8)?,
            scene: access.hook_field_of(placement, c"<SceneData>k__BackingField", scene, 8)?,
            config: access.hook_field_of(scene, c"<Config>k__BackingField", config, 8)?,
            rate: access.hook_field(config, c"<SpawnRateMultiplier>k__BackingField", "System.Single", 4)?,
            counts: [
                counts(c"lizardCalculationsDone", c"totalLizards", c"remainingLizards", c"lizardChanceMultiplier")?,
                counts(c"runeWardenCalculationsDone", c"totalRuneWardens", c"remainingRuneWardens", c"runeWardenChanceMultiplier")?,
            ],
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.postfix(layout.effect())])
    }
}

#[cfg(test)]
pub(crate) mod tests;
