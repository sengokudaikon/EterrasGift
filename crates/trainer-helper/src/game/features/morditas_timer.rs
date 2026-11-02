//! Scale the original hunt's changes: a newly assigned starting length, positive elapsed time while still hunting,
//! and positive time added by a kill. Never revive an ended hunt or rescale an unchanged secondary hunt. The game
//! retains its config, escalation curve, kill popup and bar-sync timing; adjustments become visible on its next
//! update. Off keeps elapsed progress. Original exceptions propagate without retry or post-call writes, as in C#.
//!
//! Three targets share one lifecycle and the same three-float settings. Each call keeps its receiver and prior time
//! in its own gateway home words. The game retains and serializes that receiver through the original; no helper
//! callback, object pin or shared temporary game-field mutation spans it.
use super::registry::*;
use crate::game::*;
use native_hooks::{
    emit::{Code, PostfixCode},
    postfix::{FRAME, PostfixEffect, snapshots},
};

pub(crate) const SETTINGS: SharedSettings = SharedSettings(&[ControlSetting {
    hook: GameplayHook::MorditasTimer,
    slot: None,
}]);
pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &SETTINGS.controls::<1>(),
    behavior: Behavior::SharedPostfix(&SETTINGS),
    targets: Targets::Resolve(targets),
};
#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    Start,
    Tick,
    Kill,
}
pub(crate) const STAGES: [Stage; 3] = [Stage::Start, Stage::Tick, Stage::Kill];
impl Stage {
    fn spec(self) -> MethodSpec {
        MethodSpec {
            class: c"RageOfMorditasLogic",
            method: match self {
                Self::Start => c"OnEnter_HuntState",
                Self::Tick => c"FixedUpdate_Hunt",
                Self::Kill => c"OnActorDiedDuringHunt",
            },
            instance: true,
            params: match self {
                Self::Start => &[],
                Self::Tick => &["System.Single"],
                Self::Kill => &["Actor", "System.Boolean"],
            },
            returns: "System.Void",
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub elapsed: u32,
    pub length: u32,
    pub state: u32,
    pub hunt: i32,
}
impl Layout {
    pub(crate) fn effect(self, stage: Stage) -> PostfixEffect {
        let spec = stage.spec();
        let [receiver, saved] = snapshots(spec.positions()).map(u32::from);
        let time = if matches!(stage, Stage::Tick) { self.elapsed } else { self.length };
        let mut before = Code::default();
        before.emit(&[0x45, 0x33, 0xdb]);
        before.offset(&[0x4c, 0x89, 0x9c, 0x24], receiver);
        before.gated(FRAME.switch, FRAME.slots[0]);
        before.nonnull(&[0x48, 0x85, 0xc9]);
        before.offset(&[0x48, 0x89, 0x8c, 0x24], receiver);
        before.offset(&[0x44, 0x8b, 0x99], time); // raw f32 bits; preserve all incoming float arguments
        before.offset(&[0x4c, 0x89, 0x9c, 0x24], saved);

        let mut c = PostfixCode::new(self.online);
        c.snapshot(FRAME.wide);
        c.offset(&[0x4c, 0x8b, 0x9c, 0x24], receiver);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0xf3, 0x41, 0x0f, 0x10, 0x8b], time); // xmm1 = original's new time
        c.offset(&[0xf3, 0x0f, 0x10, 0x94, 0x24], saved); // xmm2 = previous time
        match stage {
            Stage::Start => {
                c.emit(&[0x0f, 0x2e, 0xca]); // compare new and old, with C# float inequality semantics
                let unordered = c.branch(0x8a);
                c.exit(0x84);
                c.bind(unordered);
                c.emit(&[0xf3, 0x0f, 0x59, 0xcc]); // new length * starting multiplier
                c.offset(&[0xf3, 0x41, 0x0f, 0x11, 0x8b], time);
            }
            Stage::Tick | Stage::Kill => {
                if matches!(stage, Stage::Tick) {
                    c.offset(&[0x41, 0x81, 0xbb], self.state);
                    c.emit(&self.hunt.to_le_bytes());
                    c.exit(0x85);
                    c.emit(&[0x66, 0x0f, 0x73, 0xd4, 32]); // countdown multiplier -> low f32 of xmm4
                } else {
                    c.emit(&[0xf3, 0x0f, 0x10, 0xe5]); // kill multiplier -> xmm4
                }
                c.emit(&[0x0f, 0x28, 0xd9, 0xf3, 0x0f, 0x5c, 0xda]); // xmm3 = new - old
                c.emit(&[0x0f, 0x57, 0xc0, 0x0f, 0x2e, 0xd8]);
                c.exit(0x86); // only positive deltas; unordered also stays unchanged
                c.emit(&[0xf3, 0x0f, 0x59, 0xdc, 0xf3, 0x0f, 0x58, 0xd3]); // old + delta * multiplier
                c.offset(&[0xf3, 0x41, 0x0f, 0x11, 0x93], time);
            }
        }
        let mut effect = c.finish(spec.positions(), 0);
        effect.before = before.finish();
        effect
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: admitted immutable metadata and retained module/statics; all methods/field widths are checked before any target is published.
    unsafe {
        let images = access.images()?;
        let owner = access.class(&images, c"", c"RageOfMorditasLogic")?;
        let state = access.nested(owner, c"State")?;
        access.enum_storage(state, c"Int32", raw::type_code::I4)?;
        let literal = access.field(state, c"Hunt")?;
        let ty = (access.field_type)(literal);
        if ty.is_null()
            || (access.type_byref)(ty)
            || (access.type_class)(ty) != state
            || (access.field_parent)(literal) != state
            || (access.field_flags)(literal) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL) != (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL)
        {
            return Err(unsupported("Morditas Hunt enum literal"));
        }
        let mut hunt = 0i32;
        (access.static_get)(literal, (&raw mut hunt).cast());
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            elapsed: access.hook_field(owner, c"_huntElapsedTime", "System.Single", 4)?,
            length: access.hook_field(owner, c"_huntFinalTime", "System.Single", 4)?,
            state: access.hook_field_of(owner, c"_encounterState", state, 4)?,
            hunt,
        };
        STAGES
            .into_iter()
            .map(|stage| Ok(access.hook_method(&stage.spec(), module, size)?.postfix(layout.effect(stage))))
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests;
