//! Increased leech rate, reapplied after the game assembles the character's external stats.
use super::hooks::{Behavior, GameplayHookTarget, GroupId, HookFeature, MethodSpec};
use super::stat::StatFeature;
use crate::game::{GameError, RuntimeAccess};
use std::ptr::NonNull;

pub(super) const FEATURE: HookFeature = HookFeature {
    group: GroupId::LeechRate,
    behavior: Behavior::Scalar(&LEECH_RATE),
    targets,
};
pub(super) const LEECH_RATE: StatFeature = StatFeature {
    // SAFETY: bind_stat establishes the attached metadata contract before resolving the selected accessor.
    accessor: |access, player| unsafe { Ok(access.leech_stats(player)?.increased_leech_rate) },
};

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached metadata and retained module; void(instance, MethodInfo) uses only register positions.
    let method = unsafe {
        access.hook_method(
            &MethodSpec {
                class: c"BaseStats",
                method: c"ApplyExternalStats",
                instance: true,
                params: &[],
                returns: "System.Void",
            },
            module,
            size,
        )
    }?;
    Ok(vec![method.scalar(false)])
}
