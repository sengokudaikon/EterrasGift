//! Owned protocol30 status writes and bounded native acknowledgement decoding.
//! These codecs grant no process access or write permission. Host generation,
//! profile, policy and main-thread runtime admission remain independent checks.

use crate::{
    forge::{self, ForgeSnapshot},
    protocol::{DispatcherSpec, MAGIC, MAX_RESPONSE_SIZE, ProtocolError, REQUEST_SIZE, Response, ResponseCode},
};

pub const SET_STATUS_OPERATION: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StatusField {
    ForgingPotential = 0,
    LegendaryPotential = 1,
    WeaversWill = 2,
    Corrupted = 3,
    Ruined = 4,
    LevelRequirement = 5,
    WeaversTouch = 6,
}
impl StatusField {
    pub fn from_wire(value: u32) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::ForgingPotential),
            1 => Ok(Self::LegendaryPotential),
            2 => Ok(Self::WeaversWill),
            3 => Ok(Self::Corrupted),
            4 => Ok(Self::Ruined),
            5 => Ok(Self::LevelRequirement),
            6 => Ok(Self::WeaversTouch),
            _ => invalid(),
        }
    }
    pub fn requires_definition(self, item: &ForgeSnapshot) -> bool {
        matches!(self, Self::LegendaryPotential | Self::WeaversWill) && item.rarity == 7
    }
    /// Game status rules shared by planning and the native pre-store guard.
    /// `legendary_type` comes from the captured catalog on the host and getUnique
    /// inside the game. Neither an override nor an absent definition guesses it.
    pub fn allows(self, item: &ForgeSnapshot, limits: &crate::definitions::ItemLimits, value: i32, unrestricted: bool, legendary_type: Option<i32>) -> bool {
        if !(0..=255).contains(&value) {
            return false;
        }
        let status = &item.status;
        let in_range = |range: crate::definitions::ValueRange| (i32::from(range.minimum)..=i32::from(range.maximum)).contains(&value);
        match self {
            Self::ForgingPotential => !matches!(item.rarity, 7..=9) && in_range(limits.forging_potential),
            Self::LevelRequirement => !matches!(item.rarity, 7..=9) && in_range(limits.level_requirement),
            Self::Corrupted | Self::Ruined => matches!(value, 0 | 1),
            Self::WeaversTouch => unrestricted && (25..=33).contains(&item.item_type) && (value == 0 || in_range(limits.weavers_touch)),
            Self::LegendaryPotential | Self::WeaversWill => {
                let will = self == Self::WeaversWill;
                if will && unrestricted && item.rarity == 9 {
                    value <= i32::from(status.weavers_will.max(limits.weavers_will.maximum)) && (status.weavers_will > 0 || status.legendary_potential == 0 || value == 0)
                } else if item.rarity == 7 {
                    let forced = will && unrestricted && legendary_type == Some(0);
                    (legendary_type == Some(i32::from(will)) || forced)
                        && (if will { status.legendary_potential == 0 } else { status.weavers_will == 0 })
                        && (if will {
                            in_range(limits.weavers_will) || (forced && value == 0)
                        } else {
                            in_range(limits.legendary_potential)
                        })
                } else {
                    false
                }
            }
        }
    }
    pub fn value(self, item: &ForgeSnapshot) -> i32 {
        match self {
            Self::ForgingPotential => i32::from(item.status.forging_potential),
            Self::LegendaryPotential => i32::from(item.status.legendary_potential),
            Self::WeaversWill => i32::from(item.status.weavers_will),
            Self::Corrupted => i32::from(item.status.corrupted),
            Self::Ruined => i32::from(item.status.ruined),
            Self::LevelRequirement => item.status.level_requirement,
            Self::WeaversTouch => i32::from(item.status.weavers_touch),
        }
    }
    pub fn project(self, item: &mut ForgeSnapshot, value: u8) {
        match self {
            Self::ForgingPotential => item.status.forging_potential = value,
            Self::LegendaryPotential => item.status.legendary_potential = value,
            Self::WeaversWill => item.status.weavers_will = value,
            Self::Corrupted => item.status.corrupted = value != 0,
            Self::Ruined => item.status.ruined = value != 0,
            Self::LevelRequirement => item.status.level_requirement = i32::from(value),
            Self::WeaversTouch => item.status.weavers_touch = value,
        }
        item.fingerprint = item.legacy_fingerprint();
    }
}
pub const MAX_ERROR_TEXT: usize = 4096;
/// Exact maximum of the existing strict owned forge schema: fixed48, main name,
/// 32 affixes each20+name bytes,3 implicit rolls,optional identity3+8roll bytes.
/// This is a versioned schema limit, not an arbitrary caller-provided allocation.
pub const MAX_EXPECTED_BYTES: usize = 48 + forge::MAX_NAME_BYTES + forge::MAX_AFFIXES * (20 + forge::MAX_NAME_BYTES) + forge::MAX_IMPLICIT_ROLLS + 3 + forge::MAX_UNIQUE_ROLLS;
pub const MIN_EXPECTED_BYTES: usize = 48;
pub const MAX_REQUEST_BYTES: usize = REQUEST_SIZE + MAX_EXPECTED_BYTES;
fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusWrite {
    dispatcher: DispatcherSpec,
    expected_manager: u64,
    expected: Box<ForgeSnapshot>,
    value: u8,
    field: StatusField,
    unrestricted: bool,
    canonical_expected: Vec<u8>,
}
impl StatusWrite {
    /// Validates/copies the complete expected item before any transport operation.
    /// Manager and item addresses are opaque identity tokens, never dereferenced.
    /// Eligibility and the current game's literal maxLevel remain native checks.
    pub fn level_requirement(dispatcher: DispatcherSpec, expected_manager: u64, expected: ForgeSnapshot, value: i32) -> Result<Self, ProtocolError> {
        Self::new(dispatcher, expected_manager, expected, StatusField::LevelRequirement, value)
    }
    pub fn new(dispatcher: DispatcherSpec, expected_manager: u64, expected: ForgeSnapshot, field: StatusField, value: i32) -> Result<Self, ProtocolError> {
        Self::with_override(dispatcher, expected_manager, expected, field, value, false)
    }
    pub fn with_override(
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        expected: ForgeSnapshot,
        field: StatusField,
        value: i32,
        unrestricted: bool,
    ) -> Result<Self, ProtocolError> {
        let value = u8::try_from(value).map_err(|_| ProtocolError::InvalidData)?;
        if expected_manager == 0 || (matches!(field, StatusField::Corrupted | StatusField::Ruined) && value > 1) {
            return invalid();
        }
        let canonical_expected = forge::encode(Some(&expected))?;
        if !(MIN_EXPECTED_BYTES..=MAX_EXPECTED_BYTES).contains(&canonical_expected.len()) {
            return invalid();
        }
        Ok(Self {
            dispatcher,
            expected_manager,
            expected: Box::new(expected),
            value,
            field,
            unrestricted,
            canonical_expected,
        })
    }
    pub fn expected(&self) -> &ForgeSnapshot {
        &self.expected
    }
    pub fn expected_manager(&self) -> u64 {
        self.expected_manager
    }
    pub fn value(&self) -> u8 {
        self.value
    }
    pub fn field(&self) -> StatusField {
        self.field
    }
    pub fn unrestricted(&self) -> bool {
        self.unrestricted
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
        self.dispatcher
    }
    pub fn projected(&self) -> ForgeSnapshot {
        let mut projected = (*self.expected).clone();
        self.field.project(&mut projected, self.value);
        projected
    }
    /// Infallible apart from allocation: all fields and canonical tail are owned,
    /// immutable and validated by construction. No encoder unwrap of caller data.
    pub fn encode(&self) -> Vec<u8> {
        let mut frame = vec![0; REQUEST_SIZE];
        frame[..4].copy_from_slice(&MAGIC.to_le_bytes());
        frame[4..8].copy_from_slice(&SET_STATUS_OPERATION.to_le_bytes());
        frame[8..12].copy_from_slice(&i32::from(self.value).to_le_bytes());
        frame[12..16].copy_from_slice(&(self.canonical_expected.len() as u32).to_le_bytes());
        frame[16..24].copy_from_slice(&self.expected_manager.to_le_bytes());
        frame[24..28].copy_from_slice(&(self.field as u32).to_le_bytes());
        frame[28..32].copy_from_slice(&u32::from(self.unrestricted).to_le_bytes());
        frame[48..56].copy_from_slice(&self.dispatcher.rva.to_le_bytes());
        frame[56..72].copy_from_slice(&self.dispatcher.bytes);
        frame.extend_from_slice(&self.canonical_expected);
        frame
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteReply {
    /// Complete native acknowledgement for the same item. This is provisional;
    /// only the session's separate read may publish Confirmed or NotKept.
    Confirmed(ForgeSnapshot),
    /// Trusted helper contract raises these codes only before the store boundary.
    /// This is never permission for an automatic write retry.
    NotApplied { code: ResponseCode, message: String },
    /// Store may have happened; retain diagnostics and require explicit fresh read.
    Uncertain { reason: String },
}
fn uncertain(reason: impl Into<String>) -> WriteReply {
    WriteReply::Uncertain { reason: reason.into() }
}
pub fn classify_response(response: &Response, request: &StatusWrite) -> WriteReply {
    if response.payload.len() > MAX_RESPONSE_SIZE {
        return uncertain("response exceeds frame bound");
    }
    if response.code == ResponseCode::Done {
        return match forge::decode(&response.payload) {
            Ok(Some(actual)) if actual.item == request.expected().item && actual.individual_id == request.expected().individual_id => WriteReply::Confirmed(actual),
            Ok(_) => uncertain("write acknowledgement has no current target"),
            Err(_) => uncertain("invalid write readback payload"),
        };
    }
    if response.payload.len() > MAX_ERROR_TEXT {
        return uncertain("failure text exceeds bound");
    }
    let Ok(message) = std::str::from_utf8(&response.payload) else {
        return uncertain("invalid UTF-8 failure text");
    };
    if response.code == ResponseCode::Failed {
        return uncertain(message);
    }
    WriteReply::NotApplied {
        code: response.code,
        message: message.into(),
    }
}
