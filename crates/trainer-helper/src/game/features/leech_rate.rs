//! Leech rate: the character's increased leech rate, on its leech stats, set to a value the game's editor accepts.
use super::stat::StatFeature;
use eg_trainer_wire::fields::ObservedRate;

pub(super) const LEECH_RATE: StatFeature = StatFeature {
    accepts: ObservedRate::leech_editor_value,
    stat: |character| character.leech.increased_leech_rate,
    slot: |observation| &mut observation.increased_leech_rate,
};
