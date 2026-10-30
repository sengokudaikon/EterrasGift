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
/// Activation commits one feature's group, whose entries are all of one kind, as one native transaction; a change that
/// turns entries on and mixes kinds is refused before anything is written. Cleanup (every change off) may span both
/// kinds: each kind's restore is attempted, and the first failure is reported. Failure retains every owner.
/// # Safety
/// Every owned range/ABI and native transaction's thread contract holds.
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
    if !detours.is_empty() && !patches.is_empty() && changes.iter().any(|(_, on)| *on) {
        return Err("a hook change mixes entry detours and code patches".into());
    }
    // SAFETY: serialized lifecycle owner retains the whole family before publication and on every failure.
    let (detoured, patched) = unsafe {
        (
            if detours.is_empty() {
                Ok(())
            } else {
                hooks::commit(&detours).map_err(|error| error.to_string())
            },
            if patches.is_empty() {
                Ok(())
            } else {
                patch::commit(&patches).map_err(|error| error.to_string())
            },
        )
    };
    detoured.and(patched)
}
