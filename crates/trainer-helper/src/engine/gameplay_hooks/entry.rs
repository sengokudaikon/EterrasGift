//! One installation unit: an entry detour or an owned interior range. Pure patch groups and detour groups use their
//! respective native transactions. Mixed activation keeps gateways closed, commits detours then patches, and
//! restores prior detour states if the patch transaction fails. Every failure retains the complete group.
use super::*;
use native_hooks::patch::{self, CodePatch};

pub(super) enum Entry {
    Detour(RetainedHook),
    Patch(CodePatch),
}
impl Entry {
    fn range(&self) -> std::ops::Range<usize> {
        match self {
            Self::Detour(hook) => hook.range(),
            Self::Patch(patch) => patch.range(),
        }
    }
    pub(super) fn state(&self) -> HookState {
        match self {
            Self::Detour(hook) => hook.state(),
            Self::Patch(patch) => patch.state(),
        }
    }
}
/// Each native family commits with its existing transaction. For mixed activation the closed detours go first;
/// failure of the later patch transaction restores their prior states and checks the complete group's readback.
/// Interior patches can take effect between those transactions; this is compensation, not one suspended write.
/// Cleanup attempts both families even when one fails. The caller retains all owners and leaves gateways closed
/// on any failure; only a complete success may publish settings or report the feature on.
/// # Safety
/// Every owned range/ABI and native transaction's thread contract holds.
pub(super) unsafe fn commit(changes: &[(&Entry, bool)]) -> Result<(), String> {
    for (index, (entry, _)) in changes.iter().enumerate() {
        let range = entry.range();
        if changes[..index].iter().any(|(previous, _)| {
            let previous = previous.range();
            range.start < previous.end && previous.start < range.end
        }) {
            return Err("Hook entry ranges overlap".into());
        }
    }
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
        let before: Vec<_> = changes.iter().map(|(entry, _)| entry.state()).collect();
        if before.contains(&HookState::Unknown) {
            return Err("Mixed hook bytes no longer agree; nothing was written".into());
        }
        // SAFETY: caller keeps gateways closed until both transactions and independent readbacks succeed.
        unsafe { native_hooks::commit(&detours) }.map_err(|error| error.to_string())?;
        // SAFETY: all interior ranges remain owned and satisfy their existing native transaction contract.
        if let Err(error) = unsafe { patch::commit(&patches) } {
            let restore: Vec<_> = changes
                .iter()
                .zip(&before)
                .filter_map(|((entry, _), before)| match entry {
                    Entry::Detour(hook) => Some((hook, *before == HookState::Enabled)),
                    Entry::Patch(_) => None,
                })
                .collect();
            // SAFETY: same retained closed gateways; the patch transaction already attempted its own recovery.
            let recovered = unsafe { native_hooks::commit(&restore) };
            let matches = changes.iter().zip(before).all(|((entry, _), before)| entry.state() == before);
            return Err(format!("Mixed hook: {error}; detour recovery: {recovered:?}; previous bytes restored: {matches}"));
        }
        return Ok(());
    }
    // SAFETY: serialized lifecycle owner retains the whole family before publication and on every failure.
    let (detoured, patched) = unsafe {
        (
            if detours.is_empty() {
                Ok(())
            } else {
                native_hooks::commit(&detours).map_err(|error| error.to_string())
            },
            if patches.is_empty() {
                Ok(())
            } else {
                patch::commit(&patches).map_err(|error| error.to_string())
            },
        )
    };
    match (detoured, patched) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(first), Err(second)) => Err(format!("{first}; {second}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}
