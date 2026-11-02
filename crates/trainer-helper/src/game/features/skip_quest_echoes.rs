//! Raise the displayed Monolith run's quest completion to two and stability to its selected difficulty's maximum
//! before the panel reads them. Preserve higher values. Invalid/missing difficulty data leaves stability alone,
//! while quest progress still advances, as in the original trainer. These are saved-run changes, not temporary
//! overrides: disabling affects later calls and never rewinds progress. The game retains this graph for the call.
use super::registry::*;
use crate::game::*;
use native_hooks::{emit::LeafCode, leaf::LeafEffect};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::SkipQuestEchoes],
    behavior: Behavior::Leaf,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"MonolithTimelinePanelManager",
    method: c"SetRunInfo",
    instance: true,
    params: &["MonolithRun", "System.Boolean", "System.Boolean", "System.Int32"],
    returns: "System.Void",
};
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub online: NonNull<c_void>,
    pub quests: u32,
    pub stability: u32,
    pub index: u32,
    pub timeline: u32,
    pub difficulties: u32,
    pub items: u32,
    pub size: u32,
    pub maximum: u32,
}
impl Layout {
    pub(crate) fn effect(self) -> LeafEffect {
        let mut c = LeafCode::new(self.online);
        c.nonnull(&[0x48, 0x85, 0xd2]); // RDX is the run; preserve panel, both bools and stack arguments
        c.offset(&[0x83, 0xba], self.quests);
        c.emit(&[2]);
        let enough = c.branch(0x8d);
        c.offset(&[0xc7, 0x82], self.quests);
        c.emit(&2i32.to_le_bytes());
        c.bind(enough);
        c.offset(&[0x4c, 0x8b, 0x9a], self.timeline);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4d, 0x8b, 0x9b], self.difficulties);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4c, 0x63, 0x92], self.index);
        c.emit(&[0x4d, 0x85, 0xd2]);
        c.exit(0x88); // negative difficulty index
        c.offset(&[0x45, 0x3b, 0x93], self.size);
        c.exit(0x8d); // index >= active count, including a negative count
        c.offset(&[0x4d, 0x8b, 0x9b], self.items);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.emit(&[0x4d, 0x3b, 0x53, 0x18]);
        c.exit(0x83); // index >= array capacity
        c.emit(&[0x4f, 0x8b, 0x5c, 0xd3, 0x20]);
        c.nonnull(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x45, 0x8b, 0x93], self.maximum);
        c.offset(&[0x44, 0x39, 0x92], self.stability);
        c.exit(0x8d);
        c.offset(&[0x44, 0x89, 0x92], self.stability);
        c.finish() // original runs exactly once, with its complete six-position ABI unchanged
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: metadata checks the whole reference graph before publication; array headers use retained IL2CPP x64 layout.
    unsafe {
        let images = access.images()?;
        let run = access.class(&images, c"", c"MonolithRun")?;
        let timeline = access.class(&images, c"", c"MonolithTimeline")?;
        let difficulty = access.nested(timeline, c"Difficulty")?;
        let list = access.field_class(access.field(timeline, c"difficulties")?, "timeline difficulties")?;
        access.list_class(list)?;
        let array = access.field_class(access.field(list, c"_items")?, "timeline difficulties")?;
        access.array_class(array, difficulty)?;
        if (access.class_value)(difficulty) {
            return Err(unsupported("timeline difficulties must be references"));
        }
        let layout = Layout {
            online: access.hook_online_storage(&images)?,
            quests: access.hook_field(run, c"questCompletion", "System.Int32", 4)?,
            stability: access.hook_field(run, c"stability", "System.Int32", 4)?,
            index: access.hook_field(run, c"difficultyIndex", "System.Int32", 4)?,
            timeline: access.hook_field_of(run, c"timeline", timeline, 8)?,
            difficulties: access.hook_field_of(timeline, c"difficulties", list, 8)?,
            items: access.hook_field_of(list, c"_items", array, 8)?,
            size: access.hook_field(list, c"_size", "System.Int32", 4)?,
            maximum: access.hook_field(difficulty, c"maxStability", "System.Int32", 4)?,
        };
        Ok(vec![access.hook_method(&SPEC, module, size)?.leaf(layout.effect(), None, None)])
    }
}

#[cfg(test)]
pub(crate) mod tests;
