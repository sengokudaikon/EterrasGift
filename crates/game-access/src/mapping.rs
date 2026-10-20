//! Conditional presentation requests and bounded tooltip/mapping response codecs.
//! Mapping f64 is a raw game quantity, NOT the displayed scalar. The host must
//! parse the exact display/unit using the captured language; guessing is unsafe.
//! Source: NativeGameData.DecodeMappingPage, NativeBridge.ReadModifiers, and
//! native/{game-data.h:queryMapping,item.h:encodeModifiers} at e1c1ed4.
//! All lengths/integers are LE int32, quantities LE f64, editable int32 0/1.
//! Payloads are capped at 1MiB; text/context at 32KiB, language at 4KiB,
//! effects at 128/sample, modifiers at 128, roll IDs at 8/line. Storage-bound roll
//! indices are affix<32, unique<8 and implicit<3. Empty text is preserved.
//! Unlike the old permissive decoder, malformed UTF-8 and flag values other
//! than 0/1 fail. Different modifier lines may share a roll ID.

use crate::protocol::{MAX_RESPONSE_SIZE, ProtocolError};
use std::collections::BTreeSet;

pub const MAX_TEXT_BYTES: usize = 32768;
pub const MAX_LANGUAGE_BYTES: usize = 4096;
pub const MAX_CONTEXT_BYTES: usize = 32768;
pub const MAX_EFFECTS: usize = 128;
pub const MAX_MODIFIERS: usize = 128;
pub const MAX_ROLL_IDS: usize = 8;
pub const PAGE_SIZE: usize = 16;
pub const SAMPLE_COUNT: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum MappingTarget {
    Affix = 0,
    Unique = 1,
    Implicit = 2,
}
impl TryFrom<i32> for MappingTarget {
    type Error = ProtocolError;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Affix),
            1 => Ok(Self::Unique),
            2 => Ok(Self::Implicit),
            _ => invalid(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingAddress {
    pub target: MappingTarget,
    pub index: u8,
    pub line: Option<u32>,
}
impl MappingAddress {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let cap = match self.target {
            MappingTarget::Affix => 32,
            MappingTarget::Unique => 8,
            MappingTarget::Implicit => 3,
        };
        if self.index >= cap || self.line.is_some_and(|line| self.target != MappingTarget::Unique || line > i32::MAX as u32) {
            return invalid();
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingStart {
    ContextOnly,
    Samples(u8),
}
impl MappingStart {
    pub const fn count(self) -> usize {
        match self {
            Self::ContextOnly => 0,
            Self::Samples(start) => {
                let remaining = SAMPLE_COUNT - start as usize;
                if remaining < PAGE_SIZE { remaining } else { PAGE_SIZE }
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct RawValueSample {
    pub raw_quantity: f64,
    pub display: String,
    pub unit: String,
    pub effects: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct RawMappingPage {
    pub language: String,
    pub context: Vec<u8>,
    pub address: MappingAddress,
    pub start: MappingStart,
    pub samples: Vec<RawValueSample>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierSnapshot {
    pub index: u32,
    pub text: String,
    pub range: String,
    pub roll_ids: Vec<u8>,
    pub editable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueRollPreview {
    pub modifiers: Vec<ModifierSnapshot>,
}

/// Conditional presentation read. Tokens are compared against a freshly rooted graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplicitDescriptionRequest {
    pub(crate) dispatcher: crate::protocol::DispatcherSpec,
    pub(crate) manager: u64,
    pub(crate) expected: Box<crate::forge::ForgeSnapshot>,
}
impl ImplicitDescriptionRequest {
    pub const OPERATION: u32 = 12;
    pub fn new(dispatcher: crate::protocol::DispatcherSpec, manager: u64, expected: crate::forge::ForgeSnapshot) -> Result<Self, ProtocolError> {
        if manager == 0 {
            return invalid();
        }
        crate::forge::encode(Some(&expected))?;
        Ok(Self {
            dispatcher,
            manager,
            expected: Box::new(expected),
        })
    }
    pub fn dispatcher(&self) -> crate::protocol::DispatcherSpec {
        self.dispatcher
    }
    pub fn expected(&self) -> &crate::forge::ForgeSnapshot {
        &self.expected
    }
    pub fn manager(&self) -> u64 {
        self.manager
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        use crate::protocol::{MAGIC, REQUEST_SIZE};
        if self.manager == 0 {
            return invalid();
        }
        let tail = crate::forge::encode(Some(&self.expected))?;
        let mut bytes = vec![0; REQUEST_SIZE];
        bytes[..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&Self::OPERATION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(tail.len() as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.manager.to_le_bytes());
        bytes[48..56].copy_from_slice(&self.dispatcher.rva.to_le_bytes());
        bytes[56..72].copy_from_slice(&self.dispatcher.bytes);
        bytes.extend(tail);
        Ok(bytes)
    }
    pub(crate) fn validate_header(bytes: &[u8]) -> Result<(), ProtocolError> {
        if bytes[8..12].iter().chain(&bytes[24..48]).chain(&bytes[72..]).any(|b| *b != 0) {
            return invalid();
        }
        Ok(())
    }
}

/// One affix projected on the detached item, never on the forge item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffixMappingDraft {
    pub id: u16,
    pub tier: u8,
    pub roll: u8,
    pub seal: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixMappingQuery {
    pub(crate) target: ImplicitDescriptionRequest,
    pub(crate) slot: u8,
    pub(crate) start: MappingStart,
    pub(crate) draft: Option<AffixMappingDraft>,
}
impl AffixMappingQuery {
    pub const OPERATION: u32 = 9;
    pub fn new(
        dispatcher: crate::protocol::DispatcherSpec,
        manager: u64,
        expected: crate::forge::ForgeSnapshot,
        slot: u8,
        start: MappingStart,
        draft: Option<AffixMappingDraft>,
    ) -> Result<Self, ProtocolError> {
        if usize::from(slot) >= expected.affixes.len() || slot >= 32 || draft.is_some_and(|d| d.id > 4095 || !(1..=8).contains(&d.tier) || d.seal > 3) {
            return invalid();
        }
        Ok(Self {
            target: ImplicitDescriptionRequest::new(dispatcher, manager, expected)?,
            slot,
            start,
            draft,
        })
    }
    pub fn dispatcher(&self) -> crate::protocol::DispatcherSpec {
        self.target.dispatcher
    }
    pub fn slot(&self) -> u8 {
        self.slot
    }
    pub fn start(&self) -> MappingStart {
        self.start
    }
    pub fn draft(&self) -> Option<AffixMappingDraft> {
        self.draft
    }
    pub fn expected(&self) -> &crate::forge::ForgeSnapshot {
        &self.target.expected
    }
    pub fn manager(&self) -> u64 {
        self.target.manager
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = self.target.encode().expect("validated mapping target");
        bytes[4..8].copy_from_slice(&Self::OPERATION.to_le_bytes());
        bytes[8..12].copy_from_slice(&u32::from(self.slot).to_le_bytes());
        bytes[24..28].copy_from_slice(&self.draft.map_or(u32::MAX, |d| u32::from(d.id)).to_le_bytes());
        if let Some(draft) = self.draft {
            bytes[28..32].copy_from_slice(&u32::from(draft.tier).to_le_bytes());
            bytes[32..36].copy_from_slice(&u32::from(draft.seal).to_le_bytes());
            bytes[40..44].copy_from_slice(&u32::from(draft.roll).to_le_bytes());
        }
        bytes[36..40].copy_from_slice(
            &match self.start {
                MappingStart::ContextOnly => u32::MAX,
                MappingStart::Samples(start) => u32::from(start),
            }
            .to_le_bytes(),
        );
        bytes
    }
    pub(crate) fn fields(bytes: &[u8]) -> Result<(u8, MappingStart, Option<AffixMappingDraft>), ProtocolError> {
        if bytes[44..48].iter().chain(&bytes[72..]).any(|b| *b != 0) {
            return invalid();
        }
        let value = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let slot = u8::try_from(value(8)).map_err(|_| ProtocolError::InvalidData)?;
        if slot >= 32 {
            return invalid();
        }
        let start = if value(36) == u32::MAX {
            MappingStart::ContextOnly
        } else {
            MappingStart::Samples(u8::try_from(value(36)).map_err(|_| ProtocolError::InvalidData)?)
        };
        let draft = if value(24) == u32::MAX {
            if value(28) != 0 || value(32) != 0 || value(40) != 0 {
                return invalid();
            }
            None
        } else {
            let id = u16::try_from(value(24)).map_err(|_| ProtocolError::InvalidData)?;
            let tier = u8::try_from(value(28)).map_err(|_| ProtocolError::InvalidData)?;
            let roll = u8::try_from(value(40)).map_err(|_| ProtocolError::InvalidData)?;
            let seal = u8::try_from(value(32)).map_err(|_| ProtocolError::InvalidData)?;
            if id > 4095 || !(1..=8).contains(&tier) || seal > 3 {
                return invalid();
            }
            Some(AffixMappingDraft { id, tier, roll, seal })
        };
        Ok((slot, start, draft))
    }
}

fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}
struct Writer(Vec<u8>);
impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if self.0.len().checked_add(bytes.len()).is_none_or(|n| n > MAX_RESPONSE_SIZE) {
            return invalid();
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn int(&mut self, n: i32) -> Result<(), ProtocolError> {
        self.bytes(&n.to_le_bytes())
    }
    fn text(&mut self, text: &str, cap: usize) -> Result<(), ProtocolError> {
        if text.len() > cap {
            return invalid();
        }
        self.int(text.len() as i32)?;
        self.bytes(text.as_bytes())
    }
    fn quantity(&mut self, n: f64) -> Result<(), ProtocolError> {
        if !n.is_finite() {
            return invalid();
        }
        self.bytes(&n.to_le_bytes())
    }
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        let (head, tail) = self.0.split_at_checked(len).ok_or(ProtocolError::InvalidData)?;
        self.0 = tail;
        Ok(head)
    }
    fn int(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().map_err(|_| ProtocolError::InvalidData)?))
    }
    fn count(&mut self, cap: usize) -> Result<usize, ProtocolError> {
        let n = self.int()?;
        if n < 0 || n as usize > cap {
            return invalid();
        }
        Ok(n as usize)
    }
    fn text(&mut self, cap: usize) -> Result<String, ProtocolError> {
        let n = self.count(cap)?;
        std::str::from_utf8(self.bytes(n)?).map(str::to_owned).map_err(|_| ProtocolError::InvalidData)
    }
    fn quantity(&mut self) -> Result<f64, ProtocolError> {
        let n = f64::from_le_bytes(self.bytes(8)?.try_into().map_err(|_| ProtocolError::InvalidData)?);
        if !n.is_finite() {
            return invalid();
        }
        Ok(n)
    }
}
fn valid_page(page: &RawMappingPage) -> Result<(), ProtocolError> {
    page.address.validate()?;
    if page.language.trim().is_empty() || page.language.len() > MAX_LANGUAGE_BYTES || page.context.len() > MAX_CONTEXT_BYTES || page.samples.len() != page.start.count() {
        return invalid();
    }
    if let Some(first) = page.samples.first()
        && page.samples.iter().any(|s| s.unit != first.unit)
    {
        return invalid();
    }
    Ok(())
}
pub fn encode_mapping(page: &RawMappingPage) -> Result<Vec<u8>, ProtocolError> {
    valid_page(page)?;
    let mut w = Writer(Vec::new());
    w.text(&page.language, MAX_LANGUAGE_BYTES)?;
    w.int(page.context.len() as i32)?;
    w.bytes(&page.context)?;
    w.int(page.address.target as i32)?;
    w.int(i32::from(page.address.index))?;
    w.int(page.address.line.map_or(-1, |line| line as i32))?;
    if matches!(page.start, MappingStart::Samples(_)) {
        w.int(page.samples.len() as i32)?;
        for sample in &page.samples {
            w.quantity(sample.raw_quantity)?;
            w.text(&sample.display, MAX_TEXT_BYTES)?;
            w.text(&sample.unit, MAX_TEXT_BYTES)?;
            if sample.effects.len() > MAX_EFFECTS {
                return invalid();
            }
            w.int(sample.effects.len() as i32)?;
            for effect in &sample.effects {
                w.text(effect, MAX_TEXT_BYTES)?;
            }
        }
    }
    Ok(w.0)
}
/// `start` and expected address come from the submitted request: the response
/// does not echo its start position. This function cannot establish delivery.
pub fn decode_mapping(payload: &[u8], expected: MappingAddress, start: MappingStart) -> Result<RawMappingPage, ProtocolError> {
    if payload.len() > MAX_RESPONSE_SIZE {
        return invalid();
    }
    expected.validate()?;
    let mut r = Reader(payload);
    let language = r.text(MAX_LANGUAGE_BYTES)?;
    let length = r.count(MAX_CONTEXT_BYTES)?;
    let context = r.bytes(length)?.to_vec();
    let target = MappingTarget::try_from(r.int()?)?;
    let index = u8::try_from(r.int()?).map_err(|_| ProtocolError::InvalidData)?;
    let line = match r.int()? {
        -1 => None,
        n if n >= 0 => Some(n as u32),
        _ => return invalid(),
    };
    let address = MappingAddress { target, index, line };
    if address != expected {
        return invalid();
    }
    let mut samples = Vec::new();
    if matches!(start, MappingStart::Samples(_)) {
        let count = r.count(PAGE_SIZE)?;
        if count != start.count() {
            return invalid();
        }
        for _ in 0..count {
            let raw_quantity = r.quantity()?;
            let display = r.text(MAX_TEXT_BYTES)?;
            let unit = r.text(MAX_TEXT_BYTES)?;
            let effects = (0..r.count(MAX_EFFECTS)?).map(|_| r.text(MAX_TEXT_BYTES)).collect::<Result<_, _>>()?;
            samples.push(RawValueSample {
                raw_quantity,
                display,
                unit,
                effects,
            });
        }
    }
    if !r.0.is_empty() {
        return invalid();
    }
    let page = RawMappingPage {
        language,
        context,
        address,
        start,
        samples,
    };
    valid_page(&page)?;
    Ok(page)
}
fn valid_modifiers(modifiers: &[ModifierSnapshot]) -> Result<(), ProtocolError> {
    if modifiers.len() > MAX_MODIFIERS || modifiers.iter().map(|m| m.index).collect::<BTreeSet<_>>().len() != modifiers.len() {
        return invalid();
    }
    for modifier in modifiers {
        if modifier.index > i32::MAX as u32
            || modifier.roll_ids.len() > MAX_ROLL_IDS
            || modifier.roll_ids.iter().any(|id| *id > 7)
            || modifier.roll_ids.iter().collect::<BTreeSet<_>>().len() != modifier.roll_ids.len()
        {
            return invalid();
        }
    }
    Ok(())
}
pub fn encode_modifiers(modifiers: &[ModifierSnapshot]) -> Result<Vec<u8>, ProtocolError> {
    valid_modifiers(modifiers)?;
    let mut w = Writer(Vec::new());
    w.int(modifiers.len() as i32)?;
    for m in modifiers {
        w.int(m.index as i32)?;
        w.text(&m.text, MAX_TEXT_BYTES)?;
        w.text(&m.range, MAX_TEXT_BYTES)?;
        w.int(m.roll_ids.len() as i32)?;
        for id in &m.roll_ids {
            w.int(i32::from(*id))?;
        }
        w.int(i32::from(m.editable))?;
    }
    Ok(w.0)
}
pub fn decode_modifiers(payload: &[u8]) -> Result<Vec<ModifierSnapshot>, ProtocolError> {
    if payload.len() > MAX_RESPONSE_SIZE {
        return invalid();
    }
    let mut r = Reader(payload);
    let count = r.count(MAX_MODIFIERS)?;
    let mut modifiers = Vec::with_capacity(count);
    for _ in 0..count {
        let index = u32::try_from(r.int()?).map_err(|_| ProtocolError::InvalidData)?;
        let text = r.text(MAX_TEXT_BYTES)?;
        let range = r.text(MAX_TEXT_BYTES)?;
        let roll_ids = (0..r.count(MAX_ROLL_IDS)?)
            .map(|_| u8::try_from(r.int()?).map_err(|_| ProtocolError::InvalidData))
            .collect::<Result<_, _>>()?;
        let editable = match r.int()? {
            0 => false,
            1 => true,
            _ => return invalid(),
        };
        modifiers.push(ModifierSnapshot {
            index,
            text,
            range,
            roll_ids,
            editable,
        });
    }
    if !r.0.is_empty() {
        return invalid();
    }
    valid_modifiers(&modifiers)?;
    Ok(modifiers)
}
pub fn encode_preview(preview: &UniqueRollPreview) -> Result<Vec<u8>, ProtocolError> {
    encode_modifiers(&preview.modifiers)
}
pub fn decode_preview(payload: &[u8]) -> Result<UniqueRollPreview, ProtocolError> {
    Ok(UniqueRollPreview {
        modifiers: decode_modifiers(payload)?,
    })
}
