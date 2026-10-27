//! Concrete supported gameplay effects; arbitrary signatures never cross the pipe.
use crate::protocol::ProtocolError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GameplayHook {
    MysteriousRift = 0,
    Offhand = 1,
    IncomingDamage = 2,
}

impl GameplayHook {
    pub const ALL: [Self; 3] = [Self::MysteriousRift, Self::Offhand, Self::IncomingDamage];
    pub const fn id(self) -> &'static str {
        match self {
            Self::MysteriousRift => "mysterious-rift",
            Self::Offhand => "offhand",
            Self::IncomingDamage => "incoming-damage",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|hook| hook.id() == id)
    }
    /// Wire settings are whole percentages for incoming damage, Boolean selections otherwise.
    pub fn validate_value(self, value: u8) -> Result<u8, ProtocolError> {
        if match self {
            Self::IncomingDamage => value <= 100,
            Self::Offhand => value == 1,
            Self::MysteriousRift => value <= 1,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookObservation {
    pub patch: PatchState,
    pub enabled: bool,
    pub value: u8,
}
impl HookObservation {
    pub fn confirmed(self, wanted: bool) -> bool {
        self.enabled == wanted && self.patch == if wanted { PatchState::Patched } else { PatchState::Original }
    }
    pub fn encode(self) -> Vec<u8> {
        vec![
            match self.patch {
                PatchState::Original => 0,
                PatchState::Patched => 1,
                PatchState::Unknown => 2,
            },
            u8::from(self.enabled),
            self.value,
        ]
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let [patch, enabled @ 0..=1, value @ 0..=100] = *bytes else {
            return Err(ProtocolError::InvalidData);
        };
        Ok(Self {
            patch: match patch {
                0 => PatchState::Original,
                1 => PatchState::Patched,
                2 => PatchState::Unknown,
                _ => return Err(ProtocolError::InvalidData),
            },
            enabled: enabled != 0,
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DispatcherSpec, Request};
    #[test]
    fn requests_are_bounded_and_reject_unknown_hook_flags_and_extra_bytes() {
        let dispatcher = DispatcherSpec { rva: 32, bytes: [0x90; 16] };
        for hook in GameplayHook::ALL {
            for request in [
                Request::SetGameplayHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                    on: true,
                    value: 1,
                },
                Request::ReadGameplayHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                },
            ] {
                let bytes = request.encode();
                assert_eq!(Request::decode(&bytes).unwrap(), request);
                for len in 0..bytes.len() {
                    assert!(Request::decode(&bytes[..len]).is_err());
                }
                let mut invalid = bytes.clone();
                invalid[44] = 3;
                assert!(Request::decode(&invalid).is_err());
                let mut extra = bytes.clone();
                extra.push(0);
                assert!(Request::decode(&extra).is_err());
                if matches!(request, Request::SetGameplayHook { .. }) {
                    for offset in [45, 46] {
                        let mut invalid = bytes.clone();
                        invalid[offset] = 255;
                        assert!(Request::decode(&invalid).is_err());
                    }
                }
            }
        }
    }
}
