//! Absolute attack rate, reapplied after every game method that recalculates or sets it. Hooking `calculateAttackRate`
//! alone is not enough: `setMainHandAttackRate` and `setOffHandAttackRate` inline the calculation, so all three are
//! targets.
use super::registry::{Behavior, GameplayHook, GameplayHookTarget, HookFeature, MethodSpec, Targets};
use super::stat::StatFeature;
use crate::game::{GameError, RuntimeAccess};
use std::ptr::NonNull;

pub(super) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::FixedAttackRate],
    behavior: Behavior::Scalar(&ATTACK_RATE),
    targets: Targets::Resolve(targets),
};
pub(super) const ATTACK_RATE: StatFeature = StatFeature {
    // SAFETY: bind_stat establishes the attached metadata contract before resolving the selected accessor.
    accessor: |access, player| unsafe { Ok(access.attack_attributes(player)?.attack_rate) },
};

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    let specs = [
        (
            MethodSpec {
                class: c"CharacterStats",
                method: c"calculateAttackRate",
                instance: true,
                params: &[],
                returns: "System.Void",
            },
            false,
        ),
        (
            MethodSpec {
                class: c"CharacterStats",
                method: c"setMainHandAttackRate",
                instance: true,
                params: &["System.Single", "System.Boolean", "System.Boolean"],
                returns: "System.Void",
            },
            true,
        ),
        (
            MethodSpec {
                class: c"CharacterStats",
                method: c"setOffHandAttackRate",
                instance: true,
                params: &["System.Single", "System.Boolean"],
                returns: "System.Void",
            },
            false,
        ),
    ];
    // SAFETY: attached metadata and retained module; these instance void contracts fit the scalar gateway's four
    // register positions and, for the main-hand setter only, one stack position (including MethodInfo).
    specs
        .into_iter()
        .map(|(spec, stack)| unsafe { Ok(access.hook_method(&spec, module, size)?.scalar(stack)) })
        .collect()
}
