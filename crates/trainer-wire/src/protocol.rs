//! Bounded message codecs for control, observations and conditional writes.
//!
//! Decoding establishes wire validity only. Process identity, helper provenance,
//! dispatcher code, and permission to run a request belong to the session layer.

#![forbid(unsafe_code)]

use crate::{
    forge,
    rate_write::{AttackLeechRateWrite, SET_ATTACK_LEECH_RATE_OPERATION},
    write::{
        ADD_AFFIX_OPERATION, CHANGE_AFFIX_OPERATION, CHANGE_AFFIX_ROLLS_OPERATION, CHANGE_BASE_OPERATION, CHANGE_IDENTITY_OPERATION, ForgeMutation, ForgeWrite, MAX_EXPECTED_BYTES,
        REMOVE_AFFIX_OPERATION, SET_STATUS_OPERATION,
    },
};
use std::{error::Error, fmt};

pub const MAGIC: u32 = 0x4546_4735;
pub const REQUEST_HEADER_SIZE: usize = 12;
mod request;
pub const RESPONSE_HEADER_SIZE: usize = 12;
/// Maximum payload size, excluding the response header, as in NativeBridge.
pub const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidData,
    InvalidMagic,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidData => "invalid helper protocol data",
            Self::InvalidMagic => "invalid helper message signature",
        })
    }
}

impl Error for ProtocolError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatcherSpec {
    pub rva: u64,
    pub bytes: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Discover,
    /// Owner-only control request; no dispatcher installation or offline admission required.
    Unload,
    SetBooleanHook {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        hook: crate::hooks::BooleanHook,
        on: bool,
    },
    ReadBooleanHook {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        hook: crate::hooks::BooleanHook,
    },
    Status {
        dispatcher: DispatcherSpec,
    },
    /// Operation 15 with an empty wanted set: count is zero and there is no tail.
    ClearHooks {
        dispatcher: DispatcherSpec,
    },
    /// Reads an owned snapshot of the current forge item on the Unity main thread.
    ReadForge {
        dispatcher: DispatcherSpec,
    },
    /// Reads the completed locale selection and application version, without starting selection.
    ReadLocale {
        dispatcher: DispatcherSpec,
    },
    /// Explicit mutation; native admission compares the complete expected item.
    EditForge(ForgeWrite),
    DescribeImplicits(crate::mapping::ForgeDescriptionRequest),
    DescribeUnique(crate::mapping::ForgeDescriptionRequest),
    PreviewUnique(crate::mapping::UniquePreviewRequest),
    MapRoll(crate::mapping::AffixMappingQuery),
    /// Conditional CURRENT-player intent; full expected tokens/bits, no lifetime ticket.
    SetAttackLeechRate(AttackLeechRateWrite),
    /// Reads cached player fields only; expected_manager is an opaque identity token.
    ReadAttackLeech {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
    },
}

impl Request {
    pub fn dispatcher(&self) -> Option<DispatcherSpec> {
        match self {
            Self::Discover | Self::Unload => None,
            Self::Status { dispatcher }
            | Self::SetBooleanHook { dispatcher, .. }
            | Self::ReadBooleanHook { dispatcher, .. }
            | Self::ClearHooks { dispatcher }
            | Self::ReadForge { dispatcher }
            | Self::ReadLocale { dispatcher }
            | Self::ReadAttackLeech { dispatcher, .. } => Some(*dispatcher),
            Self::EditForge(write) => Some(write.dispatcher()),
            Self::DescribeImplicits(query) | Self::DescribeUnique(query) => Some(query.dispatcher),
            Self::PreviewUnique(query) => Some(query.dispatcher()),
            Self::MapRoll(query) => Some(query.dispatcher()),
            Self::SetAttackLeechRate(write) => Some(write.dispatcher()),
        }
    }
}

/// A bounded frame length, not a decoded request. Read the complete tail before
/// assigning helper ownership, installing a dispatcher or submitting any work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestHeader {
    operation: u32,
    body_len: usize,
}
impl RequestHeader {
    pub fn body_len(self) -> usize {
        self.body_len
    }
    pub fn total_len(self) -> usize {
        REQUEST_HEADER_SIZE + self.body_len
    }
}

/// Pure receiver state. Seen excess poisons the frame; future stream bytes
/// cannot retroactively revoke a command already admitted as one complete frame.
pub struct RequestFrame {
    header: RequestHeader,
    bytes: Vec<u8>,
    poisoned: bool,
}
impl RequestFrame {
    pub fn new(header: &[u8]) -> Result<Self, ProtocolError> {
        let header_plan = Request::decode_header(header)?;
        let mut bytes = Vec::with_capacity(header_plan.total_len());
        bytes.extend_from_slice(header);
        Ok(Self {
            header: header_plan,
            bytes,
            poisoned: false,
        })
    }
    pub fn remaining(&self) -> usize {
        self.header.total_len() - self.bytes.len()
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if self.poisoned {
            return Err(ProtocolError::InvalidData);
        }
        if bytes.len() > self.remaining() {
            self.poisoned = true;
            return Err(ProtocolError::InvalidData);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub fn finish(self) -> Result<Request, ProtocolError> {
        if self.poisoned || self.remaining() != 0 {
            return Err(ProtocolError::InvalidData);
        }
        Request::decode_body(self.header.operation, &self.bytes[REQUEST_HEADER_SIZE..])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ResponseCode {
    Done = 0,
    Failed = 1,
    NotReady = 2,
    StaleItem = 3,
    Refused = 4,
    OfflineRequired = 5,
    RestartRequired = 6,
    /// Offline InGame instance between player wrappers; no operation started.
    CharacterLoading = 7,
}

impl TryFrom<u32> for ResponseCode {
    type Error = ProtocolError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Done,
            1 => Self::Failed,
            2 => Self::NotReady,
            3 => Self::StaleItem,
            4 => Self::Refused,
            5 => Self::OfflineRequired,
            6 => Self::RestartRequired,
            7 => Self::CharacterLoading,
            _ => return Err(ProtocolError::InvalidData),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub code: ResponseCode,
    pub payload: Vec<u8>,
}

impl Response {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.payload.len() > MAX_RESPONSE_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut out = Vec::with_capacity(RESPONSE_HEADER_SIZE + self.payload.len());
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&(self.code as u32).to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Validate the header before allocating or reading its announced payload.
    pub fn decode_header(header: &[u8]) -> Result<(ResponseCode, usize), ProtocolError> {
        if header.len() != RESPONSE_HEADER_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut reader = Reader::new(header);
        if reader.u32()? != MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }
        let code = reader.u32()?.try_into()?;
        let length = reader.u32()? as usize;
        if length > MAX_RESPONSE_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        Ok((code, length))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let header = bytes.get(..RESPONSE_HEADER_SIZE).ok_or(ProtocolError::InvalidData)?;
        let (code, length) = Self::decode_header(header)?;
        if bytes.len() != RESPONSE_HEADER_SIZE + length {
            return Err(ProtocolError::InvalidData);
        }
        Ok(Self {
            code,
            payload: bytes[RESPONSE_HEADER_SIZE..].to_vec(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub build_id: String,
    pub dispatcher: DispatcherSpec,
}

impl Discovery {
    pub fn decode(payload: &[u8], expected_pid: u32) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(payload);
        let build_id = reader.text()?.to_owned();
        if reader.u32()? != expected_pid {
            return Err(ProtocolError::InvalidData);
        }
        let dispatcher = DispatcherSpec {
            rva: reader.u64()?,
            bytes: reader.array()?,
        };
        reader.finish()?;
        let discovery = Self { build_id, dispatcher };
        discovery.validate()?;
        Ok(discovery)
    }
    pub fn encode(&self, pid: u32) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let mut out = Vec::new();
        append_text(&mut out, &self.build_id)?;
        out.extend_from_slice(&pid.to_le_bytes());
        out.extend_from_slice(&self.dispatcher.rva.to_le_bytes());
        out.extend_from_slice(&self.dispatcher.bytes);
        Ok(out)
    }
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.build_id.len() != 64 || !self.build_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ProtocolError::InvalidData);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub online: bool,
    pub manager: u64,
    pub features_on: u32,
}

impl RuntimeStatus {
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(payload);
        let online = match reader.u32()? {
            0 => false,
            1 => true,
            _ => return Err(ProtocolError::InvalidData),
        };
        let manager = reader.u64()?;
        let features_on = reader.u32()?;
        reader.finish()?;
        if features_on > i32::MAX as u32 {
            return Err(ProtocolError::InvalidData);
        }
        Ok(Self { online, manager, features_on })
    }

    pub fn encode(&self) -> Result<[u8; 16], ProtocolError> {
        if self.features_on > i32::MAX as u32 {
            return Err(ProtocolError::InvalidData);
        }
        let mut out = [0; 16];
        out[..4].copy_from_slice(&u32::from(self.online).to_le_bytes());
        out[4..12].copy_from_slice(&self.manager.to_le_bytes());
        out[12..].copy_from_slice(&self.features_on.to_le_bytes());
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookState {
    pub id: String,
    pub on: bool,
    pub failure: Option<String>,
}

impl HookState {
    pub fn decode_all(payload: &[u8]) -> Result<Vec<Self>, ProtocolError> {
        if payload.len() > MAX_RESPONSE_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut reader = Reader::new(payload);
        let count = reader.u32()? as usize;
        if count > 1024 {
            return Err(ProtocolError::InvalidData);
        }
        let mut states = Vec::with_capacity(count);
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..count {
            let length = usize::from(reader.byte()?);
            let id = reader.take(length)?;
            if id.is_empty() || !id.is_ascii() {
                return Err(ProtocolError::InvalidData);
            }
            let id = std::str::from_utf8(id).map_err(|_| ProtocolError::InvalidData)?.to_owned();
            if !ids.insert(id.clone()) {
                return Err(ProtocolError::InvalidData);
            }
            let on = match reader.byte()? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::InvalidData),
            };
            let failure_length = usize::from(u16::from_le_bytes(reader.array()?));
            let failure = std::str::from_utf8(reader.take(failure_length)?).map_err(|_| ProtocolError::InvalidData)?;
            states.push(Self {
                id,
                on,
                failure: (!failure.is_empty()).then(|| failure.to_owned()),
            });
        }
        reader.finish()?;
        Ok(states)
    }
}

fn append_text(out: &mut Vec<u8>, value: &str) -> Result<(), ProtocolError> {
    if value.is_empty() || value.len() > 4096 {
        return Err(ProtocolError::InvalidData);
    }
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(remaining: &'a [u8]) -> Self {
        Self { remaining }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let (value, remaining) = self.remaining.split_at_checked(length).ok_or(ProtocolError::InvalidData)?;
        self.remaining = remaining;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        self.take(N)?.try_into().map_err(|_| ProtocolError::InvalidData)
    }

    fn byte(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn text(&mut self) -> Result<&'a str, ProtocolError> {
        let length = self.u32()? as usize;
        if !(1..=4096).contains(&length) {
            return Err(ProtocolError::InvalidData);
        }
        std::str::from_utf8(self.take(length)?).map_err(|_| ProtocolError::InvalidData)
    }

    fn finish(self) -> Result<(), ProtocolError> {
        if self.remaining.is_empty() { Ok(()) } else { Err(ProtocolError::InvalidData) }
    }
}
