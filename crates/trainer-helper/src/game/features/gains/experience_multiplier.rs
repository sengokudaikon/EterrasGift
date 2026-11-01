//! XP multiplier and level-up per gain share GainExp. Only the current player's tracker qualifies. Level-up wins
//! over character multiplication; skill multiplication remains independent, and favor XP is forwarded unchanged.
//! The two multipliers occupy one atomic word as two f32 values; load it once per call so pairs cannot tear. Live
//! settings have the catalog's 1..100 range. Arithmetic uses f64 after decoding, with the original's Int32 cap unless
//! the incoming gain is already larger. The original alone applies levels, skill XP and favor.
use super::*;
use crate::game::registry::{Behavior, ControlSetting, HookFeature, MethodSpec, OFF, SharedSettings, Targets};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &SETTINGS.controls::<2>(),
    behavior: Behavior::SharedLeaf(&SETTINGS),
    targets: Targets::Resolve(targets),
};
const SETTINGS: SharedSettings = SharedSettings(&[
    ControlSetting {
        hook: GameplayHook::ExperienceMultiplier,
        slot: None,
    },
    ControlSetting {
        hook: GameplayHook::LevelUpPerGain,
        slot: Some(2),
    },
]);
const SPEC: MethodSpec = MethodSpec {
    class: c"ExperienceTracker",
    method: c"GainExp",
    instance: true,
    params: &["System.Int64", "System.Int64", "System.Int64"],
    returns: "System.Void",
};
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub player: NonNull<c_void>,
    pub actor: u32,
    pub current: u32,
    pub next: u32,
}
impl Layout {
    pub(crate) fn effect(self) -> LeafEffect {
        let mut c = LeafCode::for_receiver(self.online);
        c.reference(FRAME.effect[1]);
        c.offset(&[0x4c, 0x39, 0x99], self.actor);
        c.exit(0x85);
        // One aligned snapshot for both XP multipliers. XMM3 survives our arithmetic, and is not an argument here.
        c.emit(&[0x4c, 0x8b, 0x58, FRAME.answer, 0x66, 0x49, 0x0f, 0x6e, 0xdb]);
        c.emit(&[0x48, 0x83, 0x78, FRAME.effect[2], 0xff]);
        let multiply = c.branch(0x84);
        c.emit(&[0x48, 0x85, 0xd2]);
        let no_gain = c.branch(0x8e);
        c.offset(&[0x4c, 0x8b, 0x91], self.next);
        c.offset(&[0x4c, 0x2b, 0x91], self.current);
        let overflow = c.branch(0x80); // malformed overflowing requirement: keep the original gain
        c.emit(&[0x4c, 0x39, 0xd2, 0x49, 0x0f, 0x4c, 0xd2]); // character = max(character, needed)
        c.bind(no_gain);
        c.bind(overflow);
        let skills = c.jump();
        c.bind(multiply);
        c.emit(&[0x49, 0x83, 0xfb, 0xff]);
        c.exit(0x84);
        c.emit(&[0xf3, 0x0f, 0x5a, 0xcb, 0x49, 0x89, 0xd2]); // character multiplier, gain -> r10
        scale(&mut c, true);
        c.emit(&[0x4c, 0x89, 0xd2]);
        c.bind(skills);
        c.emit(&[0x66, 0x49, 0x0f, 0x7e, 0xdb, 0x49, 0x83, 0xfb, 0xff]);
        c.exit(0x84);
        c.emit(&[0x0f, 0x28, 0xcb, 0x0f, 0xc6, 0xc9, 0x55, 0xf3, 0x0f, 0x5a, 0xc9, 0x4d, 0x89, 0xc2]);
        scale(&mut c, true);
        c.emit(&[0x4d, 0x89, 0xd0]);
        c.finish_with([self.player.as_ptr() as u64, OFF, 0])
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached immutable metadata and process-owned statics; a live tracker retains its actor and scalar
    // fields throughout this before-original leaf. All five native argument positions are forwarded in place.
    unsafe {
        let images = access.images()?;
        let tracker = access.class(&images, c"", c"ExperienceTracker")?;
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            player: access.hook_player_storage(&images)?,
            actor: access.hook_field(tracker, c"actor", "Actor", 8)?,
            current: access.hook_field(tracker, c"<CurrentExperience>k__BackingField", "System.Int64", 8)?,
            next: access.hook_field(tracker, c"<NextLevelExperience>k__BackingField", "System.Int64", 8)?,
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.leaf(layout.effect(), None, None)])
    }
}
