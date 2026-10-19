//! Owned attack/leech observations. Identity numbers are comparison tokens, not pointers.
//!
//! The versioned codec has an eight-byte header (magic u32, version u16,
//! tag u8, zero reserved u8). Available is exactly 64 bytes: manager, actor and
//! stats wrapper/native u64 pairs followed by two raw f32 u32 bit patterns.
//! Unavailable is exactly 12 bytes: the header and one typed u32 reason.
//! All integers are little endian. No process identity, generation, labels,
//! interpreted float, write authority or fallback value comes from this wire.

use crate::protocol::ProtocolError;

pub const MAGIC: u32 = 0x45474631;
pub const VERSION: u16 = 1;
pub const AVAILABLE_SIZE: usize = 64;
pub const UNAVAILABLE_SIZE: usize = 12;

/// Ephemeral equality tokens only. Both may be reused after object death;
/// the trainer must additionally bind its process, connection and observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservedObjectIdentity {
    wrapper: u64,
    native: u64,
}

impl ObservedObjectIdentity {
    pub fn new(wrapper_token: u64, native_token: u64) -> Result<Self, ProtocolError> {
        if wrapper_token == 0 || native_token == 0 {
            return Err(ProtocolError::InvalidData);
        }
        Ok(Self {
            wrapper: wrapper_token,
            native: native_token,
        })
    }

    pub const fn wrapper_token(self) -> u64 {
        self.wrapper
    }
    pub const fn native_token(self) -> u64 {
        self.native
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateUnavailable {
    NonFinite,
    OutsideEditorDomain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservedRate {
    pub raw_bits: u32,
}

impl ObservedRate {
    pub fn finite_value(self) -> Result<f32, RateUnavailable> {
        let value = f32::from_bits(self.raw_bits);
        if value.is_finite() { Ok(value) } else { Err(RateUnavailable::NonFinite) }
    }

    /// UI-domain classification only, never permission to write. Use f32 bounds
    /// so the original accepted double 0.01 -> float remains inside the domain.
    pub fn attack_editor_value(self) -> Result<f32, RateUnavailable> {
        let value = self.finite_value()?;
        if (0.01_f32..=20.0_f32).contains(&value) {
            Ok(value)
        } else {
            Err(RateUnavailable::OutsideEditorDomain)
        }
    }

    pub fn leech_editor_value(self) -> Result<f32, RateUnavailable> {
        let value = self.finite_value()?;
        if (-1.0_f32..=100.0_f32).contains(&value) {
            Ok(value)
        } else {
            Err(RateUnavailable::OutsideEditorDomain)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackLeechObservation {
    pub manager: ObservedObjectIdentity,
    pub actor: ObservedObjectIdentity,
    pub stats: ObservedObjectIdentity,
    pub attack_rate: ObservedRate,
    pub increased_leech_rate: ObservedRate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AttackLeechUnavailable {
    OfflineRequired = 1,
    /// No manager wrapper or its native object has been destroyed; global gate loss.
    ManagerAbsent = 2,
    /// Expected wrapper/native manager identity no longer matches; global gate loss.
    ManagerChanged = 3,
    ActorCacheAbsent = 4,
    StatsAbsent = 5,
    /// The actor/stats native object is gone; the manager gate may still hold.
    NativeObjectDestroyed = 6,
    GraphChanged = 7,
    UnsupportedStatsClass = 8,
}

impl TryFrom<u32> for AttackLeechUnavailable {
    type Error = ProtocolError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::OfflineRequired),
            2 => Ok(Self::ManagerAbsent),
            3 => Ok(Self::ManagerChanged),
            4 => Ok(Self::ActorCacheAbsent),
            5 => Ok(Self::StatsAbsent),
            6 => Ok(Self::NativeObjectDestroyed),
            7 => Ok(Self::GraphChanged),
            8 => Ok(Self::UnsupportedStatsClass),
            _ => Err(ProtocolError::InvalidData),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackLeechRead {
    Available(AttackLeechObservation),
    Unavailable(AttackLeechUnavailable),
}

/// Private identity fields and typed reasons make every public value encodable.
/// Raw f32 patterns, including signed zero and NaN payloads, are preserved.
pub fn encode(value: &AttackLeechRead) -> Vec<u8> {
    let (tag, size) = match value {
        AttackLeechRead::Available(_) => (1_u8, AVAILABLE_SIZE),
        AttackLeechRead::Unavailable(_) => (0_u8, UNAVAILABLE_SIZE),
    };
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&[tag, 0]);
    match value {
        AttackLeechRead::Available(observation) => {
            for identity in [observation.manager, observation.actor, observation.stats] {
                out.extend_from_slice(&identity.wrapper.to_le_bytes());
                out.extend_from_slice(&identity.native.to_le_bytes());
            }
            out.extend_from_slice(&observation.attack_rate.raw_bits.to_le_bytes());
            out.extend_from_slice(&observation.increased_leech_rate.raw_bits.to_le_bytes());
        }
        AttackLeechRead::Unavailable(reason) => out.extend_from_slice(&(*reason as u32).to_le_bytes()),
    }
    out
}

pub fn decode(bytes: &[u8]) -> Result<AttackLeechRead, ProtocolError> {
    if bytes.len() < 8 || bytes[..4] != MAGIC.to_le_bytes() {
        return Err(ProtocolError::InvalidData);
    }
    if bytes[4..6] != VERSION.to_le_bytes() {
        return Err(ProtocolError::IncompatibleVersion);
    }
    if bytes[7] != 0 {
        return Err(ProtocolError::InvalidData);
    }
    match bytes[6] {
        0 if bytes.len() == UNAVAILABLE_SIZE => {
            let reason = u32::from_le_bytes(bytes[8..12].try_into().expect("fixed reason length"));
            Ok(AttackLeechRead::Unavailable(reason.try_into()?))
        }
        1 if bytes.len() == AVAILABLE_SIZE => {
            let identity = |offset: usize| {
                ObservedObjectIdentity::new(
                    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("fixed identity width")),
                    u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().expect("fixed identity width")),
                )
            };
            Ok(AttackLeechRead::Available(AttackLeechObservation {
                manager: identity(8)?,
                actor: identity(24)?,
                stats: identity(40)?,
                attack_rate: ObservedRate {
                    raw_bits: u32::from_le_bytes(bytes[56..60].try_into().expect("fixed scalar width")),
                },
                increased_leech_rate: ObservedRate {
                    raw_bits: u32::from_le_bytes(bytes[60..64].try_into().expect("fixed scalar width")),
                },
            }))
        }
        _ => Err(ProtocolError::InvalidData),
    }
}

/// Selected field for a conditional CURRENT-player operation; never lifetime or replay authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackLeechTarget {
    AttackRate,
    IncreasedLeechRate,
}
