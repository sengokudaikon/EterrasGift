//! Concrete supported gameplay effects; arbitrary signatures never cross the pipe.
use crate::protocol::ProtocolError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GameplayHook {
    MysteriousRift = 0,
    Offhand = 1,
    IncomingDamage = 2,
    DamageDealt = 3,
    AttackRate = 4,
    LeechRate = 5,
}

impl GameplayHook {
    pub const ALL: [Self; 6] = [
        Self::MysteriousRift,
        Self::Offhand,
        Self::IncomingDamage,
        Self::DamageDealt,
        Self::AttackRate,
        Self::LeechRate,
    ];
    pub fn default_value(self) -> u64 {
        if matches!(self, Self::DamageDealt | Self::AttackRate) {
            1.0f64.to_bits()
        } else if self == Self::LeechRate {
            0.0f64.to_bits()
        } else {
            1
        }
    }
    pub const fn id(self) -> &'static str {
        match self {
            Self::MysteriousRift => "mysterious-rift",
            Self::Offhand => "offhand",
            Self::IncomingDamage => "incoming-damage",
            Self::DamageDealt => "damage-dealt",
            Self::AttackRate => "attack-rate",
            Self::LeechRate => "leech-rate",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|hook| hook.id() == id)
    }
    /// Multiplier uses IEEE f64 bits; other settings are integer percentages or Boolean selections.
    pub fn validate_value(self, value: u64) -> Result<u64, ProtocolError> {
        if match self {
            Self::IncomingDamage => value <= 100,
            Self::Offhand => value == 1,
            Self::DamageDealt => f64::from_bits(value).is_finite() && ((0.0..=100.0).contains(&f64::from_bits(value)) || value == 101.0f64.to_bits()),
            Self::MysteriousRift => value <= 1,
            Self::AttackRate => f64::from_bits(value).is_finite() && (0.01..=20.0).contains(&f64::from_bits(value)),
            Self::LeechRate => f64::from_bits(value).is_finite() && (-1.0..=100.0).contains(&f64::from_bits(value)),
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
