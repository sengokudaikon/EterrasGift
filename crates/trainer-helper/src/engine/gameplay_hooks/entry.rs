//! One installation unit: an entry detour or an owned interior range. Pure patch groups and detour groups use their
//! respective native transactions; cleanup can contain both, retaining all owners until both restores succeed.
use super::*;
use crate::hooks::patch::{self, CodePatch};

pub(super) enum Entry {
    Detour(RetainedHook),
    Patch(CodePatch),
}
impl Entry {
    pub(super) fn state(&self) -> HookState {
        match self {
            Self::Detour(hook) => hook.state(),
            Self::Patch(patch) => patch.state(),
        }
    }
}
/// # Safety
/// Every owned range/ABI and native transaction's thread contract holds. Current callers activate one homogeneous
/// feature group; clear restores all groups. Failure retains every owner, including after partial cleanup.
pub(super) unsafe fn commit(changes: &[(&Entry, bool)]) -> Result<(), String> {
    let detours: Vec<_> = changes
        .iter()
        .filter_map(|(entry, on)| match entry {
            Entry::Detour(hook) => Some((hook, *on)),
            _ => None,
        })
        .collect();
    let patches: Vec<_> = changes
        .iter()
        .filter_map(|(entry, on)| match entry {
            Entry::Patch(patch) => Some((patch, *on)),
            _ => None,
        })
        .collect();
    // SAFETY: serialized lifecycle owner retains the whole family before publication and on every failure.
    unsafe {
        if !detours.is_empty() {
            hooks::commit(&detours).map_err(|error| error.to_string())?;
        }
        if !patches.is_empty() {
            patch::commit(&patches).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
