//! One scalar feature selects one accessor. Read, pin, apply, readback and restoration are shared by the lifecycle.
use crate::game::{
    GameError, RuntimeAccess,
    accessors::{
        character::Stat,
        player::{Player, PlayerError, PlayerFields},
    },
    target,
};
use eg_il2cpp_runtime::AttachedThread;
#[cfg(test)]
mod tests;
pub(super) struct StatFeature {
    pub accessor: unsafe fn(&RuntimeAccess, &PlayerFields) -> Result<Stat, PlayerError>,
}

pub(super) struct SelectedStat<'a, 'r> {
    pub player: Player<'a, 'r>,
    pub stat: Stat,
    pub bits: u32,
}
impl RuntimeAccess {
    /// Read one selected accessor over the rooted current player. An unrelated stat is never resolved or copied.
    /// # Safety
    /// Same attached main-thread graph/lifetime contract as `player`.
    pub(super) unsafe fn selected_stat<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: u64, feature: &StatFeature) -> Result<SelectedStat<'a, 'r>, PlayerError> {
        // SAFETY: caller excludes graph mutation during this bounded read; all objects stay rooted.
        unsafe {
            let fields = self.player_fields()?;
            let stat = (feature.accessor)(self, &fields)?;
            let player = self.player(attached, fields, manager)?;
            self.check_object(stat.owner, target(&player.stats).map_err(GameError::from)?)?;
            let bits = self.single_bits(&player.stats, stat.field)?;
            self.player_unchanged(&player)?;
            Ok(SelectedStat { player, stat, bits })
        }
    }
}
