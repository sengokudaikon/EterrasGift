//! The two supported Boolean overrides. No arbitrary hook signatures cross the pipe.
use crate::protocol::ProtocolError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BooleanHook {
    MysteriousRift = 0,
    Offhand = 1,
}

impl BooleanHook {
    pub const ALL: [Self; 2] = [Self::MysteriousRift, Self::Offhand];
    pub const fn id(self) -> &'static str {
        match self {
            Self::MysteriousRift => "mysterious-rift",
            Self::Offhand => "offhand",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|hook| hook.id() == id)
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
    pub answer: bool,
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
            u8::from(self.answer),
        ]
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let [patch, enabled @ 0..=1, answer @ 0..=1] = *bytes else {
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
            answer: answer != 0,
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
        for hook in BooleanHook::ALL {
            for request in [
                Request::SetBooleanHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                    on: true,
                    answer: true,
                },
                Request::ReadBooleanHook {
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
                invalid[44] = 2;
                assert!(Request::decode(&invalid).is_err());
                let mut extra = bytes.clone();
                extra.push(0);
                assert!(Request::decode(&extra).is_err());
                if matches!(request, Request::SetBooleanHook { .. }) {
                    for offset in [45, 46] {
                        let mut invalid = bytes.clone();
                        invalid[offset] = 2;
                        assert!(Request::decode(&invalid).is_err());
                    }
                }
            }
        }
    }
}
