//! Bounded message codecs for control, observations and conditional writes.
//!
//! Decoding establishes wire validity only. Process identity, helper provenance,
//! dispatcher code, and permission to run a request belong to the session layer.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Writer, wire_enum},
    write::{ForgeWrite, MAX_EXPECTED_BYTES},
};
use std::{error::Error, fmt};

/// A helper build ID: the 64 hex digits of its DLL's SHA-256, which the desktop gives it at bootstrap. Only the format is
/// checked here; the session compares the ID itself.
pub fn is_build_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub use request::op;

pub const MAGIC: u32 = 0x4546_4735;
pub const REQUEST_HEADER_SIZE: usize = 12;
pub(crate) mod request;
pub const RESPONSE_HEADER_SIZE: usize = 12;
/// Maximum payload size, excluding the response header.
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
    SetGameplayHook {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        hook: crate::hooks::GameplayHook,
        on: bool,
        value: crate::hooks::HookValue,
    },
    ReadGameplayHook {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        hook: crate::hooks::GameplayHook,
    },
    /// Several hooks read in one turn of the game's main thread, as a monitor reads them; ascending, each once. The
    /// reply holds one observation per hook, in the same order.
    ReadGameplayHooks {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        hooks: Vec<crate::hooks::GameplayHook>,
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
    ReadFields {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        set: crate::field_edits::FieldSet,
    },
    WriteFields {
        dispatcher: DispatcherSpec,
        edit: Box<crate::field_edits::Edit>,
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
    MapModifierRoll(crate::mapping::ModifierMappingQuery),
    /// Adds affix definitions to the game's list; one it already holds as ours stays.
    RegisterAffixes {
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        affixes: Vec<crate::new_affixes::NewAffix>,
    },
}

impl Request {
    pub fn dispatcher(&self) -> Option<DispatcherSpec> {
        match self {
            Self::Discover | Self::Unload => None,
            Self::Status { dispatcher }
            | Self::SetGameplayHook { dispatcher, .. }
            | Self::RegisterAffixes { dispatcher, .. }
            | Self::ReadGameplayHook { dispatcher, .. }
            | Self::ReadGameplayHooks { dispatcher, .. }
            | Self::ClearHooks { dispatcher }
            | Self::ReadForge { dispatcher }
            | Self::ReadFields { dispatcher, .. }
            | Self::WriteFields { dispatcher, .. }
            | Self::ReadLocale { dispatcher } => Some(*dispatcher),
            Self::EditForge(write) => Some(write.dispatcher()),
            Self::DescribeImplicits(query) | Self::DescribeUnique(query) => Some(query.dispatcher),
            Self::PreviewUnique(query) => Some(query.dispatcher()),
            Self::MapRoll(query) => Some(query.dispatcher()),
            Self::MapModifierRoll(query) => Some(query.dispatcher()),
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
    /// Decodes the complete tail this header announced.
    pub(crate) fn decode_body(self, body: &[u8]) -> Result<Request, ProtocolError> {
        if body.len() != self.body_len {
            return Err(ProtocolError::InvalidData);
        }
        Request::decode_body(self.operation, body)
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

wire_enum! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ResponseCode: u32 {
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
}
impl ResponseCode {
    /// A refusal of a presentation read (a description, preview or value mapping) that leaves the item and the session
    /// as they were: the read is answered without data and the session stays ready.
    pub const fn spares_presentation_session(self) -> bool {
        matches!(self, Self::Refused | Self::Failed | Self::NotReady | Self::StaleItem)
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
        let build_id = reader.text(4096)?.to_owned();
        if reader.u32()? != expected_pid {
            return Err(ProtocolError::InvalidData);
        }
        let dispatcher = reader.get()?;
        reader.finish()?;
        let discovery = Self { build_id, dispatcher };
        discovery.validate()?;
        Ok(discovery)
    }
    pub fn encode(&self, pid: u32) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let mut w = Writer::default();
        w.text(&self.build_id);
        w.u32(pid);
        w.put(&self.dispatcher);
        Ok(w.0)
    }
    fn validate(&self) -> Result<(), ProtocolError> {
        if !is_build_id(&self.build_id) {
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
