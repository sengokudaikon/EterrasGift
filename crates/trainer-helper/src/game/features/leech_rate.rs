//! The feature selects only its own character accessor.
use super::stat::StatFeature;
pub(super) const LEECH_RATE: StatFeature = StatFeature {
    // SAFETY: bind_stat establishes the attached metadata contract before resolving the selected accessor.
    accessor: |access, player| unsafe { Ok(access.leech_stats(player)?.increased_leech_rate) },
};
