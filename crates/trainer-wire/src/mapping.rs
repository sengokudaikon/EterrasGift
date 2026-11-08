//! Conditional presentation requests and bounded tooltip/mapping response codecs.
//! Mapping f64 is a raw game quantity, NOT the displayed scalar. The host must
//! parse the exact display/unit using the captured language; guessing is unsafe.
//! All lengths/integers are LE int32, quantities LE f64, editable int32 0/1.
//! Payloads are capped at 1MiB; text/context at 32KiB, language at 4KiB,
//! effects at 128/sample, modifiers at 128, roll IDs at 8/line. Storage-bound roll
//! indices are affix<32, unique<8 and implicit<3. Empty text is preserved.
//! Malformed UTF-8 and flag values other than 0/1 fail. Different modifier
//! lines may share a roll ID.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Wire, Writer, invalid, wire_enum},
    protocol::{DispatcherSpec, ProtocolError, request::Body},
};
use std::collections::BTreeSet;

pub const MAX_TEXT_BYTES: usize = 32768;
pub const MAX_LANGUAGE_BYTES: usize = 4096;
pub const MAX_CONTEXT_BYTES: usize = 32768;
pub const MAX_EFFECTS: usize = 128;
pub const MAX_MODIFIERS: usize = 128;
pub const MAX_ROLL_IDS: usize = 8;
pub const PAGE_SIZE: usize = 16;
pub const SAMPLE_COUNT: usize = 256;

wire_enum! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum MappingTarget: i32 {
        Affix = 0,
        Unique = 1,
        Implicit = 2,
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
pub struct ForgeDescriptionRequest {
    pub(crate) dispatcher: DispatcherSpec,
    pub(crate) manager: u64,
    pub(crate) expected: crate::forge::CheckedSnapshot,
}
impl ForgeDescriptionRequest {
    pub fn new(dispatcher: DispatcherSpec, manager: u64, expected: crate::forge::ForgeSnapshot) -> Result<Self, ProtocolError> {
        Self::from_checked(dispatcher, manager, crate::forge::CheckedSnapshot::new(expected)?)
    }
    pub(crate) fn from_checked(dispatcher: DispatcherSpec, manager: u64, expected: crate::forge::CheckedSnapshot) -> Result<Self, ProtocolError> {
        if manager == 0 {
            return invalid();
        }
        Ok(Self { dispatcher, manager, expected })
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
        self.dispatcher
    }
    pub fn expected(&self) -> &crate::forge::ForgeSnapshot {
        &self.expected
    }
    pub fn manager(&self) -> u64 {
        self.manager
    }
}

/// Draft unique rolls are applied only to a private item copy for tooltip text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniqueRollChange {
    pub index: u8,
    pub value: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniquePreviewRequest {
    pub(crate) target: ForgeDescriptionRequest,
    changes: Vec<UniqueRollChange>,
}
impl UniquePreviewRequest {
    pub fn new(dispatcher: DispatcherSpec, manager: u64, expected: crate::forge::ForgeSnapshot, changes: Vec<UniqueRollChange>) -> Result<Self, ProtocolError> {
        Self::from_target(ForgeDescriptionRequest::new(dispatcher, manager, expected)?, changes)
    }
    pub(crate) fn from_target(target: ForgeDescriptionRequest, changes: Vec<UniqueRollChange>) -> Result<Self, ProtocolError> {
        let expected = target.expected();
        if !matches!(expected.rarity, 7..=9) || changes.is_empty() || changes.len() > 8 {
            return invalid();
        }
        let count = expected.identity.as_ref().ok_or(ProtocolError::InvalidData)?.rolls.len();
        let mut seen = BTreeSet::new();
        if changes.iter().any(|c| usize::from(c.index) >= count || !seen.insert(c.index)) {
            return invalid();
        }
        Ok(Self { target, changes })
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
        self.target.dispatcher()
    }
    pub fn expected(&self) -> &crate::forge::ForgeSnapshot {
        self.target.expected()
    }
    pub fn manager(&self) -> u64 {
        self.target.manager()
    }
    pub fn changes(&self) -> &[UniqueRollChange] {
        &self.changes
    }
    pub fn target(&self) -> &ForgeDescriptionRequest {
        &self.target
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
    pub(crate) target: ForgeDescriptionRequest,
    pub(crate) slot: u8,
    pub(crate) start: MappingStart,
    pub(crate) draft: Option<AffixMappingDraft>,
}
impl AffixMappingQuery {
    pub fn new(
        dispatcher: DispatcherSpec,
        manager: u64,
        expected: crate::forge::ForgeSnapshot,
        slot: u8,
        start: MappingStart,
        draft: Option<AffixMappingDraft>,
    ) -> Result<Self, ProtocolError> {
        Self::from_target(ForgeDescriptionRequest::new(dispatcher, manager, expected)?, slot, start, draft)
    }
    pub(crate) fn from_target(target: ForgeDescriptionRequest, slot: u8, start: MappingStart, draft: Option<AffixMappingDraft>) -> Result<Self, ProtocolError> {
        let expected = target.expected();
        if usize::from(slot) >= expected.affixes.len() || slot >= 32 || draft.is_some_and(|d| d.id > 4095 || !(1..=8).contains(&d.tier) || d.seal > 3) {
            return invalid();
        }
        Ok(Self { target, slot, start, draft })
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
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
}

/// A unique or implicit roll sampled on a detached copy: the roll (`address.index`), for a unique the tooltip line that
/// reads it (`address.line`; lines that share a roll show it differently), and the other drafted unique rolls its line
/// depends on. Implicits depend on their own roll only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierMappingQuery {
    pub(crate) target: ForgeDescriptionRequest,
    pub(crate) address: MappingAddress,
    pub(crate) start: MappingStart,
    pub(crate) draft: Vec<UniqueRollChange>,
}
impl ModifierMappingQuery {
    pub fn new(
        dispatcher: DispatcherSpec,
        manager: u64,
        expected: crate::forge::ForgeSnapshot,
        address: MappingAddress,
        start: MappingStart,
        draft: Vec<UniqueRollChange>,
    ) -> Result<Self, ProtocolError> {
        Self::from_target(ForgeDescriptionRequest::new(dispatcher, manager, expected)?, address, start, draft)
    }
    pub(crate) fn from_target(target: ForgeDescriptionRequest, address: MappingAddress, start: MappingStart, draft: Vec<UniqueRollChange>) -> Result<Self, ProtocolError> {
        address.validate()?;
        let expected = target.expected();
        let count = match address.target {
            MappingTarget::Affix => return invalid(),
            MappingTarget::Unique if matches!(expected.rarity, 7..=9) => expected.identity.as_ref().ok_or(ProtocolError::InvalidData)?.rolls.len(),
            MappingTarget::Unique => return invalid(),
            MappingTarget::Implicit if draft.is_empty() => expected.implicit_rolls.len(),
            MappingTarget::Implicit => return invalid(),
        };
        let mut seen = BTreeSet::from([address.index]);
        if usize::from(address.index) >= count || draft.len() > 7 || draft.iter().any(|c| usize::from(c.index) >= count || !seen.insert(c.index)) {
            return invalid();
        }
        Ok(Self { target, address, start, draft })
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
        self.target.dispatcher
    }
    pub fn address(&self) -> MappingAddress {
        self.address
    }
    pub fn start(&self) -> MappingStart {
        self.start
    }
    pub fn draft(&self) -> &[UniqueRollChange] {
        &self.draft
    }
    pub fn expected(&self) -> &crate::forge::ForgeSnapshot {
        &self.target.expected
    }
    pub fn manager(&self) -> u64 {
        self.target.manager
    }
}

// The queries as requests carry them, after the envelope's dispatcher: the manager, the query's own fields, then the
// expected item.

impl Wire for UniqueRollChange {
    fn write(&self, w: &mut Writer) {
        w.byte(self.index);
        w.byte(self.value);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        Ok(Self {
            index: r.byte()?,
            value: r.byte()?,
        })
    }
}

/// Whether it samples, then where it starts when it does.
impl Wire for MappingStart {
    fn write(&self, w: &mut Writer) {
        w.flag(matches!(self, Self::Samples(_)));
        if let Self::Samples(start) = *self {
            w.byte(start);
        }
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        Ok(if r.flag()? { Self::Samples(r.byte()?) } else { Self::ContextOnly })
    }
}

impl Wire for AffixMappingDraft {
    fn write(&self, w: &mut Writer) {
        w.u16(self.id);
        w.byte(self.tier);
        w.byte(self.roll);
        w.byte(self.seal);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: r.u16()?,
            tier: r.byte()?,
            roll: r.byte()?,
            seal: r.byte()?,
        })
    }
}

/// Roll changes behind their count.
fn write_changes(w: &mut Writer, changes: &[UniqueRollChange]) {
    w.byte(changes.len() as u8);
    for change in changes {
        w.put(change);
    }
}
/// Roll changes behind their count: at most `limit`, and at least one when `some`.
fn read_changes(r: &mut Reader<'_>, limit: usize, some: bool) -> Result<Vec<UniqueRollChange>, ProtocolError> {
    let count = r.count(limit)?;
    if some && count == 0 {
        return invalid();
    }
    (0..count).map(|_| r.get()).collect()
}

impl Body for ForgeDescriptionRequest {
    fn write(&self, w: &mut Writer) {
        w.u64(self.manager);
        w.put(&self.expected);
    }
    fn read(dispatcher: DispatcherSpec, r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let manager = r.u64()?;
        Self::from_checked(dispatcher, manager, r.get()?)
    }
}

impl Body for UniquePreviewRequest {
    fn write(&self, w: &mut Writer) {
        w.u64(self.target.manager);
        write_changes(w, &self.changes);
        w.put(&self.target.expected);
    }
    fn read(dispatcher: DispatcherSpec, r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let manager = r.u64()?;
        let changes = read_changes(r, 8, true)?;
        Self::from_target(ForgeDescriptionRequest::from_checked(dispatcher, manager, r.get()?)?, changes)
    }
}

impl Body for AffixMappingQuery {
    fn write(&self, w: &mut Writer) {
        w.u64(self.target.manager);
        w.byte(self.slot);
        w.put(&self.start);
        w.flag(self.draft.is_some());
        if let Some(draft) = &self.draft {
            w.put(draft);
        }
        w.put(&self.target.expected);
    }
    fn read(dispatcher: DispatcherSpec, r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let manager = r.u64()?;
        let slot = r.byte()?;
        let start = r.get()?;
        let draft = if r.flag()? { Some(r.get()?) } else { None };
        Self::from_target(ForgeDescriptionRequest::from_checked(dispatcher, manager, r.get()?)?, slot, start, draft)
    }
}

impl Body for ModifierMappingQuery {
    fn write(&self, w: &mut Writer) {
        w.u64(self.target.manager);
        w.byte(self.address.target as u8);
        w.byte(self.address.index);
        w.flag(self.address.line.is_some());
        if let Some(line) = self.address.line {
            w.u32(line);
        }
        w.put(&self.start);
        write_changes(w, &self.draft);
        w.put(&self.target.expected);
    }
    fn read(dispatcher: DispatcherSpec, r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let manager = r.u64()?;
        let target = MappingTarget::try_from(i32::from(r.byte()?))?;
        let index = r.byte()?;
        let line = if r.flag()? { Some(r.u32()?) } else { None };
        let start = r.get()?;
        let draft = read_changes(r, 7, false)?;
        Self::from_target(
            ForgeDescriptionRequest::from_checked(dispatcher, manager, r.get()?)?,
            MappingAddress { target, index, line },
            start,
            draft,
        )
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
    let mut w = Writer::default();
    w.text(&page.language);
    w.blob(&page.context);
    w.i32(page.address.target as i32);
    w.i32(i32::from(page.address.index));
    w.i32(page.address.line.map_or(-1, |line| line as i32));
    if matches!(page.start, MappingStart::Samples(_)) {
        w.u32(page.samples.len() as u32);
        for sample in &page.samples {
            if !sample.raw_quantity.is_finite() || sample.effects.len() > MAX_EFFECTS {
                return invalid();
            }
            w.f64(sample.raw_quantity);
            w.text_within(&sample.display, MAX_TEXT_BYTES)?;
            w.text_within(&sample.unit, MAX_TEXT_BYTES)?;
            w.u32(sample.effects.len() as u32);
            for effect in &sample.effects {
                w.text_within(effect, MAX_TEXT_BYTES)?;
            }
        }
    }
    w.finish()
}
/// `start` and expected address come from the submitted request: the response
/// does not echo its start position. This function cannot establish delivery.
pub fn decode_mapping(payload: &[u8], expected: MappingAddress, start: MappingStart) -> Result<RawMappingPage, ProtocolError> {
    let mut r = Reader::reply(payload)?;
    expected.validate()?;
    let language = r.text(MAX_LANGUAGE_BYTES)?.to_owned();
    let context = r.blob(MAX_CONTEXT_BYTES)?.to_vec();
    let target = MappingTarget::try_from(r.i32()?)?;
    let index = u8::try_from(r.i32()?).map_err(|_| ProtocolError::InvalidData)?;
    let line = match r.i32()? {
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
        let count = r.count32(PAGE_SIZE)?;
        if count != start.count() {
            return invalid();
        }
        for _ in 0..count {
            let raw_quantity = r.f64()?;
            if !raw_quantity.is_finite() {
                return invalid();
            }
            let display = r.text(MAX_TEXT_BYTES)?.to_owned();
            let unit = r.text(MAX_TEXT_BYTES)?.to_owned();
            let effects = (0..r.count32(MAX_EFFECTS)?).map(|_| r.text(MAX_TEXT_BYTES).map(str::to_owned)).collect::<Result<_, _>>()?;
            samples.push(RawValueSample {
                raw_quantity,
                display,
                unit,
                effects,
            });
        }
    }
    r.finish()?;
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
    let mut w = Writer::default();
    w.u32(modifiers.len() as u32);
    for m in modifiers {
        w.i32(m.index as i32);
        w.text_within(&m.text, MAX_TEXT_BYTES)?;
        w.text_within(&m.range, MAX_TEXT_BYTES)?;
        w.u32(m.roll_ids.len() as u32);
        for id in &m.roll_ids {
            w.i32(i32::from(*id));
        }
        w.i32(i32::from(m.editable));
    }
    w.finish()
}
pub fn decode_modifiers(payload: &[u8]) -> Result<Vec<ModifierSnapshot>, ProtocolError> {
    let mut r = Reader::reply(payload)?;
    let count = r.count32(MAX_MODIFIERS)?;
    let mut modifiers = Vec::with_capacity(count);
    for _ in 0..count {
        let index = u32::try_from(r.i32()?).map_err(|_| ProtocolError::InvalidData)?;
        let text = r.text(MAX_TEXT_BYTES)?.to_owned();
        let range = r.text(MAX_TEXT_BYTES)?.to_owned();
        let roll_ids = (0..r.count32(MAX_ROLL_IDS)?)
            .map(|_| u8::try_from(r.i32()?).map_err(|_| ProtocolError::InvalidData))
            .collect::<Result<_, _>>()?;
        let editable = match r.i32()? {
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
    r.finish()?;
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
