//! One scalar feature selects one accessor, read over the rooted current player. Pinning retains only its managed
//! storage; the hook lifecycle closes/drains and restores it before releasing the pin, including when the character
//! leaves play.
use crate::game::{
    GameError, RuntimeAccess, WriteError,
    accessors::{
        character::Stat,
        player::{Player, PlayerError, PlayerFields},
    },
    target, unsupported,
};
use eg_il2cpp_runtime::{AttachedThread, raw};
use std::{ffi::c_void, ptr::NonNull};
#[cfg(test)]
mod tests;
pub(crate) struct StatFeature {
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

pub struct StatBinding {
    pub receiver: usize,
    pub offset: u32,
    pub baseline: u32,
    pub online: usize,
    handle: NonNull<c_void>,
    free: raw::GcHandleFree,
}
// SAFETY: nonmoving pinned managed storage; ownership is serialized by the hook lifecycle. Release is on an attached thread.
unsafe impl Send for StatBinding {}
// SAFETY: shared handles are read-only; all writes and final release are serialized by the lifecycle owner.
unsafe impl Sync for StatBinding {}
impl Drop for StatBinding {
    fn drop(&mut self) {
        // SAFETY: lifecycle closes/drains every gateway and restores storage before dropping its last binding.
        unsafe { (self.free)(self.handle.as_ptr()) };
    }
}
impl RuntimeAccess {
    /// # Safety
    /// Same attached main-thread graph/lifetime contract as `selected_stat`.
    pub(crate) unsafe fn current_stat(&self, attached: &AttachedThread<'_>, manager: u64, feature: &StatFeature) -> Result<(usize, u32), WriteError> {
        // SAFETY: caller admits the current player's complete synchronous read span.
        let selected = unsafe { self.selected_stat(attached, manager, feature) }?;
        Ok((target(&selected.player.stats).map_err(GameError::from)?.as_ptr() as usize, selected.bits))
    }
    /// # Safety
    /// Attached main thread, initialized game statics and exclusive player read span as `player`. The caller keeps
    /// this binding until all gateways using it close and drain, and drops it on an attached runtime thread.
    pub(crate) unsafe fn bind_stat(&self, attached: &AttachedThread<'_>, ownership: raw::Api, manager: u64, feature: &StatFeature) -> Result<StatBinding, WriteError> {
        // SAFETY: the selected accessor alone is resolved over a rooted, checked current player.
        unsafe {
            let SelectedStat { player, stat, bits: baseline } = self.selected_stat(attached, manager, feature)?;
            let object = target(&player.stats).map_err(GameError::from)?;
            let field = self.mutable_field(stat.owner, stat.name, c"Single", 0x0c)?;
            if field.as_ptr().cast::<c_void>() != stat.field.cast::<c_void>() {
                return Err(unsupported("scalar field identity").into());
            }
            let offset = (self.field_offset)(stat.field);
            if !(16..=16384).contains(&offset) || !offset.is_multiple_of(4) {
                return Err(unsupported("scalar field alignment").into());
            }
            let images = self.images()?;
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
            let online = self.hook_static_storage(environment, online, 1)?.as_ptr() as usize;
            self.player_unchanged(&player)?;
            let handle = NonNull::new((ownership.gchandle_new)(object.as_ptr(), true)).ok_or_else(|| unsupported("pin scalar receiver"))?;
            Ok(StatBinding {
                receiver: object.as_ptr() as usize,
                offset: offset as u32,
                online,
                baseline,
                handle,
                free: ownership.gchandle_free,
            })
        }
    }
}
