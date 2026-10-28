//! Stat features: one `System.Single` stat on a character accessor, set to a value the game's own editor accepts.
//! Attack rate and leech rate are two; this is what they share: the observation the wire carries (both stats with
//! the player's identities), the read, and the write through `apply`.
use super::{attack_rate::ATTACK_RATE, leech_rate::LEECH_RATE};
use crate::game::{
    GameError, RuntimeAccess,
    accessors::{
        character::{AttackAttributes, LeechStats, Stat},
        player::{Player, PlayerError},
    },
    apply::{WriteError, stored},
    target,
};
use eg_il2cpp_runtime::{AttachedThread, raw};
use eg_trainer_wire::fields::{AttackLeechObservation, AttackLeechRead, AttackLeechTarget, ObservedRate, RateUnavailable};

#[cfg(test)]
mod tests;

/// A stat feature: the stat it sets and the values it accepts.
pub(super) struct StatFeature {
    /// The values the game's own editor accepts.
    pub(super) accepts: fn(ObservedRate) -> Result<f32, RateUnavailable>,
    /// The stat on the character's accessors.
    pub(super) stat: fn(&Character<'_, '_>) -> Stat,
    /// The stat in the observation.
    pub(super) slot: fn(&mut AttackLeechObservation) -> &mut ObservedRate,
}

/// The character's accessors for one call, over the rooted player.
pub(super) struct Character<'a, 'r> {
    player: Player<'a, 'r>,
    pub(super) attack: AttackAttributes,
    pub(super) leech: LeechStats,
}

impl StatFeature {
    /// `expected` with this stat set to `value`, if the editor accepts the value and it changes the stat.
    fn project(&self, expected: &AttackLeechObservation, value: f32) -> Result<AttackLeechObservation, WriteError> {
        let value = ObservedRate { raw_bits: value.to_bits() };
        if (self.accepts)(value).is_err() {
            return Err(WriteError::Refused("New rate must be finite and inside its source editor domain".into()));
        }
        let mut projected = *expected;
        *(self.slot)(&mut projected) = value;
        if projected == *expected {
            return Err(WriteError::Refused("Rate already has these exact bits; no store is needed".into()));
        }
        Ok(projected)
    }
}

impl RuntimeAccess {
    /// # Safety
    /// As [`RuntimeAccess::read_attack_leech`].
    unsafe fn character<'a, 'r>(&self, attached: &'a AttachedThread<'r>, expected_manager: u64) -> Result<Character<'a, 'r>, PlayerError> {
        // SAFETY: the caller's complete graph contract.
        unsafe {
            let fields = self.player_fields()?;
            let attack = self.attack_attributes(&fields)?;
            let leech = self.leech_stats(&fields)?;
            Ok(Character {
                player: self.player(attached, fields, expected_manager)?,
                attack,
                leech,
            })
        }
    }

    /// Both stats with the player's identities, checked unchanged before and after the copy. The checks catch drift;
    /// they do not make the copy atomic against other writers.
    ///
    /// # Safety
    /// As [`RuntimeAccess::read_attack_leech`], with `character`'s roots still held.
    unsafe fn observe(&self, character: &Character<'_, '_>) -> Result<AttackLeechObservation, PlayerError> {
        // SAFETY: the rooted player retains the exact scalar storage throughout this synchronous copy.
        unsafe {
            let player = &character.player;
            self.player_unchanged(player)?;
            let attack_rate = self.attack_rate(player, &character.attack)?;
            let increased_leech_rate = self.increased_leech_rate(player, &character.leech)?;
            self.player_unchanged(player)?;
            Ok(AttackLeechObservation {
                manager: player.manager_identity,
                actor: player.actor_identity,
                stats: player.stats_identity,
                attack_rate,
                increased_leech_rate,
            })
        }
    }

    /// Copy the character's attack and leech rates from populated fields only. No getPlayerActor, component lookup,
    /// managed cctor, scalar getter, stat refresh, setter or save call is invoked.
    ///
    /// # Safety
    /// As [`RuntimeAccess::player`]: the exclusive main-thread read span over the player's statics, references, scalar
    /// fields and native lifetimes. `expected_manager` is an identity comparison token, never a dereferenceable
    /// pointer; the host's process and generation must bind the returned observation.
    pub unsafe fn read_attack_leech(&self, attached: &AttachedThread<'_>, expected_manager: u64) -> Result<AttackLeechRead, GameError> {
        // SAFETY: the caller establishes the documented complete graph contract.
        let observation = unsafe { self.character(attached, expected_manager).and_then(|character| self.observe(&character)) };
        match observation {
            Ok(observation) => Ok(AttackLeechRead::Available(observation)),
            Err(PlayerError::Unavailable(reason)) => Ok(AttackLeechRead::Unavailable(reason)),
            Err(PlayerError::Contract(error)) => Err(error),
        }
    }

    /// Set one rate through its stat feature: a conditional store into the current player's stats, read back whole.
    /// No getter, constructor, refresh, save, callback or reapplication is requested.
    ///
    /// # Safety
    /// As [`RuntimeAccess::read_attack_leech`], now with exclusive WRITE permission over the whole rooted player and
    /// its statics through store and readback. The attachment and both export tables belong to the same initialized
    /// runtime. The caller authorizes this one mutation of the current player and excludes every other writer, graph or
    /// native lifetime change, reentrancy, shutdown and callback. `Field::SetValue` is the direct R4 assignment with no
    /// conversion, managed body or retained argument. Expected tokens are compared only: roots taken by this call keep
    /// managed wrappers, not native objects, alive, and token and bit equality cannot rule out ABA across requests, so
    /// the caller separately establishes that this is still the player it selected. Once the setter is entered every
    /// failure is uncertain; nothing is retried or rolled back.
    pub unsafe fn set_attack_leech(
        &self,
        attached: &AttachedThread<'_>,
        write_api: raw::FieldWriteApi,
        expected: &AttackLeechObservation,
        target: AttackLeechTarget,
        value: f32,
    ) -> Result<AttackLeechObservation, WriteError> {
        let feature = match target {
            AttackLeechTarget::AttackRate => &ATTACK_RATE,
            AttackLeechTarget::IncreasedLeechRate => &LEECH_RATE,
        };
        // SAFETY: the caller's exclusive write span.
        unsafe { self.set_stat(attached, write_api, expected, feature, value) }
    }

    /// # Safety
    /// As [`RuntimeAccess::set_attack_leech`].
    unsafe fn set_stat(
        &self,
        attached: &AttachedThread<'_>,
        write_api: raw::FieldWriteApi,
        expected: &AttackLeechObservation,
        feature: &StatFeature,
        value: f32,
    ) -> Result<AttackLeechObservation, WriteError> {
        let projected = feature.project(expected, value)?;
        // SAFETY: the caller's exclusive main-thread span; the rooted checks precede the single store and its readback.
        unsafe {
            let character = self.character(attached, expected.manager.wrapper_token())?;
            let stat = (feature.stat)(&character);
            let field = self.mutable_field(stat.owner, stat.name, c"Single", 0x0c)?;
            if field.as_ptr().cast::<std::ffi::c_void>() != stat.field.cast::<std::ffi::c_void>() {
                return Err(WriteError::Refused("Rate is not the exact mutable declared System.Single/R4 field".into()));
            }
            let object = target(&character.player.stats).map_err(GameError::from)?;
            // No metadata work or managed body follows this last complete observation before the store.
            if self.observe(&character)? != *expected {
                return Err(WriteError::Stale("Complete current field observation differs from expected".into()));
            }
            stored(
                || {
                    write_api.set_f32(object, field, value);
                    Ok(())
                },
                || {
                    let actual = self.observe(&character)?;
                    if actual != projected {
                        return Err(WriteError::Failed("Complete current graph/readback differs from projected observation".into()));
                    }
                    Ok(actual)
                },
            )
        }
    }
}
