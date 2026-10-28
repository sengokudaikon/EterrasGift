//! Scalar observations and player identity tokens shared by game accessors.
use crate::protocol::ProtocolError;

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
#[repr(u32)]
pub enum PlayerUnavailable {
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

impl TryFrom<u32> for PlayerUnavailable {
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
