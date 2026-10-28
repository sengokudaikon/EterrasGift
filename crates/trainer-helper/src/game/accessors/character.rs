//! The character's stats, as two accessors on the player's stats object that know nothing of each other: its attack
//! attributes, declared on `CharacterStats`, and its leech stats, declared on `BaseStats`.
use super::player::{Player, PlayerError, PlayerFields, unavailable};
use crate::game::{Class, Ptr, RuntimeAccess};
use eg_trainer_wire::fields::{AttackLeechUnavailable as Unavailable, ObservedRate};
use std::ffi::CStr;

/// One `System.Single` stat: the class that declares it, its name and its field.
#[derive(Clone, Copy)]
pub(crate) struct Stat {
    pub(crate) owner: Class,
    pub(crate) name: &'static CStr,
    pub(crate) field: Ptr,
}

/// The character's attack attributes. Their stats object must be exactly a `CharacterStats`: other layouts, even with
/// an inherited field, stay unsupported.
pub(crate) struct AttackAttributes {
    pub(crate) attack_rate: Stat,
}

/// The character's leech stats.
pub(crate) struct LeechStats {
    pub(crate) increased_leech_rate: Stat,
}

impl RuntimeAccess {
    /// # Safety
    /// As [`RuntimeAccess::player_fields`].
    unsafe fn stat(&self, owner: Class, name: &'static CStr, single: Class) -> Result<Stat, PlayerError> {
        // SAFETY: the caller's metadata contract; the field is checked against its declaring owner and System.Single.
        let field = unsafe { self.declared_field(owner, name, single, false) }?;
        Ok(Stat { owner, name, field })
    }

    /// # Safety
    /// As [`RuntimeAccess::player_fields`].
    pub(crate) unsafe fn attack_attributes(&self, player: &PlayerFields) -> Result<AttackAttributes, PlayerError> {
        // SAFETY: the caller's metadata contract.
        unsafe {
            let class = self.class(&player.images, c"", c"CharacterStats")?;
            if (self.class_value)(class) {
                return Err(crate::game::unsupported("character stats reference class").into());
            }
            Ok(AttackAttributes {
                attack_rate: self.stat(class, c"attackRate", player.single)?,
            })
        }
    }

    /// # Safety
    /// As [`RuntimeAccess::player_fields`].
    pub(crate) unsafe fn leech_stats(&self, player: &PlayerFields) -> Result<LeechStats, PlayerError> {
        // SAFETY: the caller's metadata contract.
        unsafe {
            Ok(LeechStats {
                increased_leech_rate: self.stat(player.base_stats, c"increasedLeechRate", player.single)?,
            })
        }
    }

    /// The character's attack rate.
    ///
    /// # Safety
    /// As [`RuntimeAccess::player`], with `player`'s roots still held.
    pub(crate) unsafe fn attack_rate(&self, player: &Player<'_, '_>, attack: &AttackAttributes) -> Result<ObservedRate, PlayerError> {
        // SAFETY: the stats object is rooted; its exact class is checked before its Single field is copied.
        unsafe {
            if (self.object_class)(crate::game::target(&player.stats)?.as_ptr()) != attack.attack_rate.owner {
                return Err(unavailable(Unavailable::UnsupportedStatsClass));
            }
            Ok(ObservedRate {
                raw_bits: self.single_bits(&player.stats, attack.attack_rate.field)?,
            })
        }
    }

    /// The character's increased leech rate.
    ///
    /// # Safety
    /// As [`RuntimeAccess::player`], with `player`'s roots still held.
    pub(crate) unsafe fn increased_leech_rate(&self, player: &Player<'_, '_>, leech: &LeechStats) -> Result<ObservedRate, PlayerError> {
        // SAFETY: the player checked the stats object is a BaseStats, which declares the field.
        unsafe {
            Ok(ObservedRate {
                raw_bits: self.single_bits(&player.stats, leech.increased_leech_rate.field)?,
            })
        }
    }
}
