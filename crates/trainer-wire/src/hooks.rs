//! Concrete supported gameplay effects; arbitrary signatures never cross the pipe.
use crate::protocol::ProtocolError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GameplayHook {
    MysteriousRift = 0,
    AnyOffhand = 1,
    IncomingDamage = 2,
    EnemyDamage = 3,
    FixedAttackRate = 4,
    FixedLeechRate = 5,
    NoManaCost = 6,
    SkillsToLevel20 = 7,
    EquipWithoutRequirements = 8,
    FullSetBonus = 9,
    UnlimitedSkillPoints = 10,
    UnlimitedPassivePoints = 11,
    AnyMasteryPassives = 12,
    UnlimitedWeaverPoints = 13,
    ForgeAboveLevel = 14,
    UnlimitedWovenEnchanter = 15,
    PickupAnywhere = 16,
    FreeHunterVendor = 17,
    EndgameCache = 18,
    FavorCache = 19,
    Nemesis = 20,
    OmenWindows = 21,
    ClassExperimentals = 22,
    RunePrisonComplex = 23,
    ShardsTo99 = 24,
    RunesGlyphsTo99 = 25,
    MaxRandomRolls = 26,
    BossLootChance = 27,
    Tier7Affixes = 28,
    MaxBonusStability = 29,
    AbilitySpeed = 30,
    OneEchoType = 31,
}

impl GameplayHook {
    pub const ALL: [Self; 32] = [
        Self::MysteriousRift,
        Self::AnyOffhand,
        Self::IncomingDamage,
        Self::EnemyDamage,
        Self::FixedAttackRate,
        Self::FixedLeechRate,
        Self::NoManaCost,
        Self::SkillsToLevel20,
        Self::EquipWithoutRequirements,
        Self::FullSetBonus,
        Self::UnlimitedSkillPoints,
        Self::UnlimitedPassivePoints,
        Self::AnyMasteryPassives,
        Self::UnlimitedWeaverPoints,
        Self::ForgeAboveLevel,
        Self::UnlimitedWovenEnchanter,
        Self::PickupAnywhere,
        Self::FreeHunterVendor,
        Self::EndgameCache,
        Self::FavorCache,
        Self::Nemesis,
        Self::OmenWindows,
        Self::ClassExperimentals,
        Self::RunePrisonComplex,
        Self::ShardsTo99,
        Self::RunesGlyphsTo99,
        Self::MaxRandomRolls,
        Self::BossLootChance,
        Self::Tier7Affixes,
        Self::MaxBonusStability,
        Self::AbilitySpeed,
        Self::OneEchoType,
    ];
    pub fn default_value(self) -> u64 {
        if matches!(self, Self::EnemyDamage | Self::FixedAttackRate | Self::AbilitySpeed) {
            1.0f64.to_bits()
        } else if self == Self::FixedLeechRate {
            0.0f64.to_bits()
        } else {
            1
        }
    }
    pub const fn id(self) -> &'static str {
        match self {
            Self::MysteriousRift => "mysterious-rift",
            Self::AnyOffhand => "any-offhand",
            Self::IncomingDamage => "incoming-damage",
            Self::EnemyDamage => "enemy-damage",
            Self::FixedAttackRate => "fixed-attack-rate",
            Self::FixedLeechRate => "fixed-leech-rate",
            Self::NoManaCost => "no-mana-cost",
            Self::SkillsToLevel20 => "skills-to-level-20",
            Self::EquipWithoutRequirements => "equip-without-requirements",
            Self::FullSetBonus => "full-set-bonus",
            Self::UnlimitedSkillPoints => "unlimited-skill-points",
            Self::UnlimitedPassivePoints => "unlimited-passive-points",
            Self::AnyMasteryPassives => "any-mastery-passives",
            Self::UnlimitedWeaverPoints => "unlimited-weaver-points",
            Self::ForgeAboveLevel => "forge-above-level",
            Self::UnlimitedWovenEnchanter => "unlimited-woven-enchanter",
            Self::PickupAnywhere => "pickup-anywhere",
            Self::FreeHunterVendor => "free-hunter-vendor",
            Self::EndgameCache => "endgame-cache",
            Self::FavorCache => "favor-cache",
            Self::Nemesis => "nemesis",
            Self::OmenWindows => "omen-windows",
            Self::ClassExperimentals => "class-experimentals",
            Self::RunePrisonComplex => "rune-prison-complex",
            Self::ShardsTo99 => "shards-to-99",
            Self::RunesGlyphsTo99 => "runes-glyphs-to-99",
            Self::MaxRandomRolls => "max-random-rolls",
            Self::BossLootChance => "boss-loot-chance",
            Self::Tier7Affixes => "tier-7-affixes",
            Self::MaxBonusStability => "max-bonus-stability",
            Self::AbilitySpeed => "ability-speed",
            Self::OneEchoType => "one-echo-type",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|hook| hook.id() == id)
    }
    /// Multiplier uses IEEE f64 bits; other settings are integer percentages or Boolean selections.
    pub fn validate_value(self, value: u64) -> Result<u64, ProtocolError> {
        if match self {
            Self::IncomingDamage => value <= 100,
            Self::AnyOffhand
            | Self::NoManaCost
            | Self::SkillsToLevel20
            | Self::EquipWithoutRequirements
            | Self::FullSetBonus
            | Self::UnlimitedSkillPoints
            | Self::UnlimitedPassivePoints
            | Self::AnyMasteryPassives
            | Self::UnlimitedWeaverPoints
            | Self::ForgeAboveLevel
            | Self::UnlimitedWovenEnchanter
            | Self::PickupAnywhere
            | Self::FreeHunterVendor
            | Self::EndgameCache
            | Self::FavorCache
            | Self::Nemesis
            | Self::OmenWindows
            | Self::ClassExperimentals
            | Self::RunePrisonComplex
            | Self::ShardsTo99
            | Self::RunesGlyphsTo99
            | Self::MaxRandomRolls
            | Self::BossLootChance
            | Self::Tier7Affixes
            | Self::MaxBonusStability => value == 1,
            Self::AbilitySpeed => f64::from_bits(value).is_finite() && (0.0..=100.0).contains(&f64::from_bits(value)),
            Self::OneEchoType => value <= 9,
            Self::EnemyDamage => f64::from_bits(value).is_finite() && ((0.0..=100.0).contains(&f64::from_bits(value)) || value == 101.0f64.to_bits()),
            Self::MysteriousRift => value <= 1,
            Self::FixedAttackRate => f64::from_bits(value).is_finite() && (0.01..=20.0).contains(&f64::from_bits(value)),
            Self::FixedLeechRate => f64::from_bits(value).is_finite() && (-1.0..=100.0).contains(&f64::from_bits(value)),
        } {
            Ok(value)
        } else {
            Err(ProtocolError::InvalidData)
        }
    }
    pub(crate) fn decode(value: u8) -> Result<Self, ProtocolError> {
        Self::ALL.get(value as usize).copied().ok_or(ProtocolError::InvalidData)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchState {
    Original,
    Patched,
    Unknown,
    /// Entry bytes are patched and another feature still owns the patch group.
    Shared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookObservation {
    pub patch: PatchState,
    pub enabled: bool,
    pub value: u64,
}
impl HookObservation {
    pub fn confirmed(self, wanted: bool) -> bool {
        self.enabled == wanted
            && if wanted {
                self.patch == PatchState::Patched
            } else {
                matches!(self.patch, PatchState::Original | PatchState::Shared)
            }
    }
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = vec![
            match self.patch {
                PatchState::Original => 0,
                PatchState::Patched => 1,
                PatchState::Unknown => 2,
                PatchState::Shared => 3,
            },
            u8::from(self.enabled),
        ];
        bytes.extend(self.value.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let [patch, enabled @ 0..=1, ref value @ ..] = *bytes else {
            return Err(ProtocolError::InvalidData);
        };
        Ok(Self {
            patch: match patch {
                0 => PatchState::Original,
                1 => PatchState::Patched,
                2 => PatchState::Unknown,
                3 => PatchState::Shared,
                _ => return Err(ProtocolError::InvalidData),
            },
            enabled: enabled != 0,
            value: u64::from_le_bytes(value.try_into().map_err(|_| ProtocolError::InvalidData)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DispatcherSpec, Request};
    #[test]
    fn requests_reject_unknown_hooks_flags_and_values() {
        let dispatcher = DispatcherSpec { rva: 32, bytes: [0x90; 16] };
        for hook in GameplayHook::ALL {
            for request in [
                Request::SetGameplayHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                    on: true,
                    value: hook.default_value(),
                },
                Request::ReadGameplayHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                },
            ] {
                let bytes = request.encode();
                let mut invalid = bytes.clone();
                invalid[44] = 255;
                assert!(Request::decode(&invalid).is_err());
                if matches!(request, Request::SetGameplayHook { .. }) {
                    for offset in [45, 46] {
                        let mut invalid = bytes.clone();
                        if offset == 46 {
                            invalid[offset..offset + 8].fill(255);
                        } else {
                            invalid[offset] = 255;
                        }
                        assert!(Request::decode(&invalid).is_err());
                    }
                }
            }
        }
    }
}
