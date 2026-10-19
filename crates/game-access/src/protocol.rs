//! Pure, bounded codecs for protocol33: control, observations and narrow conditional writes.
//!
//! Decoding establishes wire validity only. Process identity, helper provenance,
//! dispatcher code, and permission to run a request belong to the session layer.

use crate::{
    catalog::CatalogKind,
    forge,
    rate_write::{AttackLeechRateWrite, SET_ATTACK_LEECH_RATE_OPERATION},
    write::{
        CHANGE_AFFIX_OPERATION, CHANGE_AFFIX_ROLLS_OPERATION, CHANGE_BASE_OPERATION, CHANGE_IDENTITY_OPERATION, ForgeMutation, ForgeWrite, MAX_EXPECTED_BYTES, MIN_EXPECTED_BYTES,
        SET_STATUS_OPERATION,
    },
};
use std::{collections::BTreeMap, error::Error, fmt};

pub const MAGIC: u32 = 0x4546_4735;
/// Version32 adds bounded affix roll batches to the complete expected-state write path.
pub const VERSION: u32 = 33;
pub const ADAPTER: &str = "il2cpp-x64-v1";
pub const REQUEST_SIZE: usize = 336;
pub const RESPONSE_HEADER_SIZE: usize = 12;
/// Maximum payload size, excluding the response header, as in NativeBridge.
pub const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidData,
    IncompatibleVersion,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidData => "invalid helper protocol data",
            Self::IncompatibleVersion => "incompatible helper protocol or adapter",
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
    /// Reads literal metadata ranges; it does not capture a localized game-data catalog.
    ReadItemLimits {
        dispatcher: DispatcherSpec,
    },
    /// Reads the completed locale selection and application version, without starting selection.
    ReadLocale {
        dispatcher: DispatcherSpec,
    },
    /// Copies at most sixteen definitions, with locale and total repeated on every page.
    ReadCatalogPage {
        dispatcher: DispatcherSpec,
        kind: CatalogKind,
        cursor: u32,
    },
    /// Explicit mutation; native admission compares the complete expected item.
    EditForge(ForgeWrite),
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
            Self::Discover => None,
            Self::Status { dispatcher }
            | Self::ClearHooks { dispatcher }
            | Self::ReadForge { dispatcher }
            | Self::ReadItemLimits { dispatcher }
            | Self::ReadLocale { dispatcher }
            | Self::ReadCatalogPage { dispatcher, .. }
            | Self::ReadAttackLeech { dispatcher, .. } => Some(*dispatcher),
            Self::EditForge(write) => Some(write.dispatcher()),
            Self::SetAttackLeechRate(write) => Some(write.dispatcher()),
        }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; REQUEST_SIZE];
        bytes[..4].copy_from_slice(&MAGIC.to_le_bytes());
        let (operation, dispatcher) = match self {
            Self::Discover => (6_u32, None),
            Self::Status { dispatcher } => (7, Some(dispatcher)),
            Self::ClearHooks { dispatcher } => (15, Some(dispatcher)),
            Self::ReadForge { dispatcher } => (18, Some(dispatcher)),
            Self::ReadItemLimits { dispatcher } => (19, Some(dispatcher)),
            Self::ReadLocale { dispatcher } => (20, Some(dispatcher)),
            Self::ReadCatalogPage { dispatcher, kind, cursor } => {
                bytes[8..12].copy_from_slice(&(*kind as u32).to_le_bytes());
                bytes[12..16].copy_from_slice(&cursor.to_le_bytes());
                (21, Some(dispatcher))
            }
            Self::EditForge(write) => return write.encode(),
            Self::SetAttackLeechRate(write) => return write.encode(),
            Self::ReadAttackLeech { dispatcher, expected_manager } => {
                bytes[8..16].copy_from_slice(&expected_manager.to_le_bytes());
                (23, Some(dispatcher))
            }
        };
        bytes[4..8].copy_from_slice(&operation.to_le_bytes());
        if let Some(dispatcher) = dispatcher {
            bytes[48..56].copy_from_slice(&dispatcher.rva.to_le_bytes());
            bytes[56..72].copy_from_slice(&dispatcher.bytes);
        }
        bytes
    }

    /// A bounded read plan only, never an executable request or ownership claim.
    pub fn decode_header(bytes: &[u8]) -> Result<RequestHeader, ProtocolError> {
        if bytes.len() != REQUEST_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut reader = Reader::new(bytes);
        if reader.u32()? != MAGIC {
            return Err(ProtocolError::IncompatibleVersion);
        }
        let operation = reader.u32()?;
        if !matches!(
            operation,
            CHANGE_AFFIX_OPERATION | SET_STATUS_OPERATION | CHANGE_BASE_OPERATION | CHANGE_IDENTITY_OPERATION | CHANGE_AFFIX_ROLLS_OPERATION
        ) {
            Self::decode_fixed(bytes)?;
            return Ok(RequestHeader { tail_len: 0 });
        }
        let _value = reader.u32()?;
        let tail_len = reader.u32()? as usize;
        let manager = reader.u64()?;
        ForgeMutation::decode_header(bytes)?;
        if manager == 0 || !(MIN_EXPECTED_BYTES..=MAX_EXPECTED_BYTES).contains(&tail_len) {
            return Err(ProtocolError::InvalidData);
        }
        Ok(RequestHeader { tail_len })
    }
    /// Accepts exactly one complete frame, including all reserved bytes and the
    /// write's entire strict expected-state tail. A header alone cannot submit it.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let header = Self::decode_header(bytes.get(..REQUEST_SIZE).ok_or(ProtocolError::InvalidData)?)?;
        if bytes.len() != header.total_len() {
            return Err(ProtocolError::InvalidData);
        }
        if header.tail_len == 0 {
            return Self::decode_fixed(bytes);
        }
        let mut fields = Reader::new(&bytes[8..24]);
        let _value = fields.u32()?;
        let _tail_len = fields.u32()?;
        let expected_manager = fields.u64()?;
        let mut dispatcher = Reader::new(&bytes[48..72]);
        let dispatcher = DispatcherSpec {
            rva: dispatcher.u64()?,
            bytes: dispatcher.array()?,
        };
        let expected = forge::decode(&bytes[REQUEST_SIZE..])?.ok_or(ProtocolError::InvalidData)?;
        let mutation = ForgeMutation::decode_header(&bytes[..REQUEST_SIZE])?;
        Ok(Self::EditForge(ForgeWrite::edit(dispatcher, expected_manager, expected, mutation)?))
    }

    fn decode_fixed(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != REQUEST_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut reader = Reader::new(bytes);
        if reader.u32()? != MAGIC {
            return Err(ProtocolError::IncompatibleVersion);
        }
        let operation = reader.u32()?;
        if operation == SET_ATTACK_LEECH_RATE_OPERATION {
            return Ok(Self::SetAttackLeechRate(AttackLeechRateWrite::decode(bytes)?));
        }
        let reserved_start = if matches!(operation, 21 | 23) { 16 } else { 8 };
        if bytes[reserved_start..48].iter().chain(&bytes[72..]).any(|byte| *byte != 0) {
            return Err(ProtocolError::InvalidData);
        }
        let mut dispatcher_reader = Reader::new(&bytes[48..72]);
        let dispatcher = DispatcherSpec {
            rva: dispatcher_reader.u64()?,
            bytes: dispatcher_reader.array()?,
        };
        match operation {
            6 if dispatcher.rva == 0 && dispatcher.bytes == [0; 16] => Ok(Self::Discover),
            7 => Ok(Self::Status { dispatcher }),
            15 => Ok(Self::ClearHooks { dispatcher }),
            18 => Ok(Self::ReadForge { dispatcher }),
            19 => Ok(Self::ReadItemLimits { dispatcher }),
            20 => Ok(Self::ReadLocale { dispatcher }),
            23 => {
                let mut query = Reader::new(&bytes[8..16]);
                let expected_manager = query.u64()?;
                if expected_manager == 0 {
                    return Err(ProtocolError::InvalidData);
                }
                Ok(Self::ReadAttackLeech { dispatcher, expected_manager })
            }
            21 => {
                let mut query = Reader::new(&bytes[8..16]);
                let kind = CatalogKind::try_from(query.u32()?)?;
                let cursor = query.u32()?;
                if cursor > kind.maximum_total() {
                    return Err(ProtocolError::InvalidData);
                }
                Ok(Self::ReadCatalogPage { dispatcher, kind, cursor })
            }
            _ => Err(ProtocolError::InvalidData),
        }
    }
}

/// A bounded frame length, not a decoded request. Read the complete tail before
/// assigning helper ownership, installing a dispatcher or submitting any work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestHeader {
    tail_len: usize,
}
impl RequestHeader {
    pub fn tail_len(self) -> usize {
        self.tail_len
    }
    pub fn total_len(self) -> usize {
        REQUEST_SIZE + self.tail_len
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
        Request::decode(&self.bytes)
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
            return Err(ProtocolError::IncompatibleVersion);
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

/// Exact field names in `RuntimeDiscovery.RequiredKeys` and `requiredFields`.
pub const REQUIRED_KEYS: [&str; 37] = [
    "ItemContainersManager.CraftingOffset",
    "CraftingContainers.MainOffset",
    "OneSlotItemContainer.ContentOffset",
    "ItemContainerEntry.DataOffset",
    "ItemDataUnpacked.SerializationOffset",
    "ItemDataUnpacked.IndividualIdOffset",
    "ItemDataUnpacked.ItemTypeOffset",
    "ItemDataUnpacked.SubTypeOffset",
    "ItemDataUnpacked.RarityOffset",
    "ItemDataUnpacked.UniqueIdOffset",
    "ItemDataUnpacked.ImplicitRollsOffset",
    "ItemDataUnpacked.UniqueRollsOffset",
    "ItemDataUnpacked.AffixesOffset",
    "ItemDataUnpacked.ClassRequirementOffset",
    "ItemDataUnpacked.FullNameOffset",
    "ItemDataUnpacked.SealedRegularOffset",
    "ItemDataUnpacked.SealedPrimordialOffset",
    "ItemDataUnpacked.SealedCorruptionOffset",
    "ItemDataUnpacked.ForgingPotentialOffset",
    "ItemDataUnpacked.ForgingPotentialTypeOffset",
    "ItemDataUnpacked.LegendaryPotentialOffset",
    "ItemDataUnpacked.WeaversWillOffset",
    "ItemDataUnpacked.WeaversTouchOffset",
    "ItemDataUnpacked.CorruptedOffset",
    "ItemDataUnpacked.RuinedOffset",
    "ItemDataUnpacked.LevelRequirementOffset",
    "ItemAffix.IdOffset",
    "ItemAffix.TierOffset",
    "ItemAffix.RollOffset",
    "ItemAffix.TypeOffset",
    "ItemAffix.SpecialAffixTypeOffset",
    "ItemAffix.NameOffset",
    "ItemAffix.SealOffset",
    "AffixList.SingleAffixesOffset",
    "AffixList.MultiAffixesOffset",
    "AffixDefinition.IdOffset",
    "AffixDefinition.TiersOffset",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub build_id: String,
    pub dispatcher: DispatcherSpec,
    pub offsets: BTreeMap<String, u64>,
}

impl Discovery {
    pub fn decode(payload: &[u8], expected_pid: u32) -> Result<Self, ProtocolError> {
        if payload.len() > MAX_RESPONSE_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut reader = Reader::new(payload);
        let version = reader.u32()?;
        let build_id = reader.text()?.to_owned();
        let adapter = reader.text()?;
        let pid = reader.u32()?;
        let dispatcher = DispatcherSpec {
            rva: reader.u64()?,
            bytes: reader.array()?,
        };
        let count = reader.u32()? as usize;
        if version != VERSION || adapter != ADAPTER {
            return Err(ProtocolError::IncompatibleVersion);
        }
        if pid != expected_pid || !(1..=128).contains(&count) {
            return Err(ProtocolError::InvalidData);
        }
        let mut offsets = BTreeMap::new();
        for _ in 0..count {
            let key = reader.text()?.to_owned();
            let offset = reader.u64()?;
            if offsets.insert(key, offset).is_some() {
                return Err(ProtocolError::InvalidData);
            }
        }
        reader.finish()?;
        let discovery = Self { build_id, dispatcher, offsets };
        discovery.validate()?;
        Ok(discovery)
    }

    /// Useful for authored helper fixtures; applies the same bounds as decoding.
    pub fn encode(&self, pid: u32) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let mut out = Vec::new();
        out.extend_from_slice(&VERSION.to_le_bytes());
        append_text(&mut out, &self.build_id)?;
        append_text(&mut out, ADAPTER)?;
        out.extend_from_slice(&pid.to_le_bytes());
        out.extend_from_slice(&self.dispatcher.rva.to_le_bytes());
        out.extend_from_slice(&self.dispatcher.bytes);
        out.extend_from_slice(&(self.offsets.len() as u32).to_le_bytes());
        for (key, offset) in &self.offsets {
            append_text(&mut out, key)?;
            out.extend_from_slice(&offset.to_le_bytes());
        }
        if out.len() > MAX_RESPONSE_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        Ok(out)
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if self.build_id.len() != 64 || !self.build_id.bytes().all(|byte| byte.is_ascii_hexdigit()) || !(1..=128).contains(&self.offsets.len()) {
            return Err(ProtocolError::InvalidData);
        }
        for key in REQUIRED_KEYS {
            if !self.offsets.get(key).is_some_and(|offset| (16..=16384).contains(offset)) {
                return Err(ProtocolError::InvalidData);
            }
        }
        // C# validates offsets used by the current schema. Extra keys are forward
        // compatible, but their names still obey the handshake text bounds.
        if self.offsets.keys().any(|key| key.is_empty() || key.len() > 4096) {
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
