//! The feature selects only its own character accessor.
use super::stat::StatFeature;
pub(super) const ATTACK_RATE: StatFeature = StatFeature {
    // SAFETY: bind_stat establishes the attached metadata contract before resolving the selected accessor.
    accessor: |access, player| unsafe { Ok(access.attack_attributes(player)?.attack_rate) },
};
