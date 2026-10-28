//! Attack rate: the character's attack rate, on its attack attributes, set to a value the game's editor accepts.
use super::stat::StatFeature;
use eg_trainer_wire::fields::ObservedRate;

pub(super) const ATTACK_RATE: StatFeature = StatFeature {
    accepts: ObservedRate::attack_editor_value,
    stat: |character| character.attack.attack_rate,
    slot: |observation| &mut observation.attack_rate,
};
