//! Pure, observation-bound tooltip/preview data and exact sampled value tables.
//! No request, runtime invocation, cache promotion, or endpoint interpolation.
//! A supplied parser must implement the captured game's display/culture rules;
//! there is deliberately no default parser and raw_quantity is never substituted.
//! The builder requires all 16 pages and pins full observation, draft, opaque
//! context, language and unit. Failed pushes poison it; no partial table escapes.

mod display;
use crate::edits::{ExpectedObservation, ItemEdit, MAX_WRITABLE_AFFIX_ID, RollEdit, RollTarget};
pub use display::CapturedDisplayParser;
pub use eg_trainer_wire::mapping::{MappingAddress, MappingStart, MappingTarget, RawMappingPage, RawValueSample};
use eg_trainer_wire::{
    mapping::{self as wire, ModifierSnapshot, UniqueRollPreview},
    protocol::MAX_RESPONSE_SIZE,
};
use std::{collections::BTreeSet, fmt};

/// Aggregate encoded page budget, excluding bounded observation/context and
/// container overhead. This is not an exact allocation-size assertion.
pub const MAX_TABLE_BYTES: usize = 16 * MAX_RESPONSE_SIZE;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MappingError {
    InvalidData,
    InvalidQuery,
    MissingTooltipEvidence,
    WrongObservation,
    WrongContext,
    OutOfOrder,
    Incomplete,
    Poisoned,
    DisplayUnavailable(String),
    FixedValue,
    NonMonotonic,
    OutOfRange,
}
impl fmt::Display for MappingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mapping: {self:?}")
    }
}
impl std::error::Error for MappingError {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TooltipKind {
    Unique,
    Implicit,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundTooltip {
    expected: ExpectedObservation,
    kind: TooltipKind,
    modifiers: Vec<ModifierSnapshot>,
}
impl BoundTooltip {
    /// Binds validated response data to its submitted full observation. This is
    /// not evidence that a native request actually ran or permission to edit.
    pub fn new(expected: &ExpectedObservation, kind: TooltipKind, modifiers: Vec<ModifierSnapshot>) -> Result<Self, MappingError> {
        wire::encode_modifiers(&modifiers).map_err(|_| MappingError::InvalidData)?;
        let item = expected.snapshot();
        let count = match kind {
            TooltipKind::Unique => {
                if !matches!(item.rarity, 7 | 9) {
                    return Err(MappingError::InvalidQuery);
                }
                item.identity.as_ref().ok_or(MappingError::InvalidQuery)?.rolls.len()
            }
            TooltipKind::Implicit => item.implicit_rolls.len(),
        };
        if modifiers
            .iter()
            .any(|m| m.roll_ids.iter().any(|id| usize::from(*id) >= count) || kind == TooltipKind::Implicit && m.roll_ids.len() > 1)
        {
            return Err(MappingError::InvalidData);
        }
        Ok(Self {
            expected: expected.clone(),
            kind,
            modifiers,
        })
    }
    pub fn modifiers(&self) -> &[ModifierSnapshot] {
        &self.modifiers
    }
    pub fn recheck(&self, current: &ExpectedObservation) -> Result<(), MappingError> {
        if self.expected.matches(current) { Ok(()) } else { Err(MappingError::WrongObservation) }
    }
    fn rolling(&self, index: u8) -> bool {
        self.modifiers.iter().any(|m| m.editable && m.roll_ids.contains(&index))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingRequest {
    pub address: MappingAddress,
    pub draft: Option<ItemEdit>,
}
fn target(target: MappingTarget) -> RollTarget {
    match target {
        MappingTarget::Affix => RollTarget::Affix,
        MappingTarget::Unique => RollTarget::Unique,
        MappingTarget::Implicit => RollTarget::Implicit,
    }
}
fn valid_roll_draft(expected: &ExpectedObservation, edit: &RollEdit, tooltip: Option<&BoundTooltip>, maximum: usize) -> Result<(), MappingError> {
    if edit.changes.is_empty() || edit.changes.len() > maximum || edit.changes.iter().map(|c| c.index).collect::<BTreeSet<_>>().len() != edit.changes.len() {
        return Err(MappingError::InvalidQuery);
    }
    if edit.target == RollTarget::Unique {
        let evidence = tooltip.ok_or(MappingError::MissingTooltipEvidence)?;
        evidence.recheck(expected)?;
        if evidence.kind != TooltipKind::Unique {
            return Err(MappingError::InvalidQuery);
        }
    }
    for change in &edit.changes {
        if !(0..=255).contains(&change.value) {
            return Err(MappingError::InvalidQuery);
        }
        let index = u8::try_from(change.index).map_err(|_| MappingError::InvalidQuery)?;
        let item = expected.snapshot();
        match edit.target {
            RollTarget::Affix => {
                if !item.affixes.iter().any(|a| a.slot == index) {
                    return Err(MappingError::InvalidQuery);
                }
            }
            RollTarget::Unique => {
                let evidence = tooltip.ok_or(MappingError::MissingTooltipEvidence)?;
                if !evidence.rolling(index) {
                    return Err(MappingError::InvalidQuery);
                }
            }
            RollTarget::Implicit => return Err(MappingError::InvalidQuery),
        }
    }
    Ok(())
}
impl MappingRequest {
    pub fn validate(&self, expected: &ExpectedObservation, tooltip: Option<&BoundTooltip>) -> Result<(), MappingError> {
        self.address.validate().map_err(|_| MappingError::InvalidQuery)?;
        let item = expected.snapshot();
        match self.address.target {
            MappingTarget::Affix => {
                if !item.affixes.iter().any(|a| a.slot == self.address.index) {
                    return Err(MappingError::InvalidQuery);
                }
            }
            MappingTarget::Unique => {
                if !matches!(item.rarity, 7 | 9) || item.identity.as_ref().is_none_or(|id| usize::from(self.address.index) >= id.rolls.len()) {
                    return Err(MappingError::InvalidQuery);
                }
            }
            MappingTarget::Implicit => {
                let evidence = tooltip.ok_or(MappingError::MissingTooltipEvidence)?;
                evidence.recheck(expected)?;
                if evidence.kind != TooltipKind::Implicit || !evidence.rolling(self.address.index) {
                    return Err(MappingError::InvalidQuery);
                }
            }
        }
        if let Some(line) = self.address.line {
            let evidence = tooltip.ok_or(MappingError::MissingTooltipEvidence)?;
            evidence.recheck(expected)?;
            if evidence.kind != TooltipKind::Unique || !evidence.modifiers.iter().any(|m| m.index == line && m.roll_ids.contains(&self.address.index)) {
                return Err(MappingError::InvalidQuery);
            }
        }
        match &self.draft {
            None => {}
            Some(ItemEdit::Affix(edit))
                if self.address.target == MappingTarget::Affix
                    && edit.slot == i32::from(self.address.index)
                    && (0..=MAX_WRITABLE_AFFIX_ID).contains(&edit.id)
                    && (1..=8).contains(&edit.tier)
                    && (0..=255).contains(&edit.roll) => {}
            Some(ItemEdit::Roll(edit)) if self.address.target != MappingTarget::Implicit && edit.target == target(self.address.target) => {
                valid_roll_draft(expected, edit, tooltip, 31)?
            }
            _ => return Err(MappingError::InvalidQuery),
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundPreview {
    expected: ExpectedObservation,
    draft: RollEdit,
    preview: UniqueRollPreview,
}
impl BoundPreview {
    pub fn new(expected: &ExpectedObservation, draft: RollEdit, preview: UniqueRollPreview, evidence: &BoundTooltip) -> Result<Self, MappingError> {
        Self::prepare(expected, draft, evidence)?.with_reply(preview)
    }
    pub(crate) fn prepare(expected: &ExpectedObservation, draft: RollEdit, evidence: &BoundTooltip) -> Result<Self, MappingError> {
        if draft.target != RollTarget::Unique {
            return Err(MappingError::InvalidQuery);
        }
        valid_roll_draft(expected, &draft, Some(evidence), 32)?;
        Ok(Self {
            expected: expected.clone(),
            draft,
            preview: UniqueRollPreview { modifiers: vec![] },
        })
    }
    pub(crate) fn with_reply(mut self, preview: UniqueRollPreview) -> Result<Self, MappingError> {
        BoundTooltip::new(&self.expected, TooltipKind::Unique, preview.modifiers.clone())?;
        self.preview = preview;
        Ok(self)
    }
    pub fn preview(&self) -> &UniqueRollPreview {
        &self.preview
    }
    pub fn recheck(&self, current: &ExpectedObservation, draft: &RollEdit) -> Result<(), MappingError> {
        if !self.expected.matches(current) {
            return Err(MappingError::WrongObservation);
        }
        if &self.draft != draft {
            return Err(MappingError::WrongContext);
        }
        Ok(())
    }
}

pub trait DisplayScalarParser {
    /// Return the scalar represented by exact display/unit in captured language,
    /// or a reason it cannot be resolved. No fallback culture or raw-number use.
    /// Source behavior strips the exact ordinal unit suffix (empty means whole
    /// display), then uses captured CultureInfo and Float|AllowThousands parsing.
    /// The builder checks suffix/finite output; this parser owns culture parity.
    /// Authored test implementations do not establish host/game BCL equivalence.
    fn parse(&mut self, language: &str, display: &str, unit: &str) -> Result<f64, String>;
}
#[derive(Clone, Debug, PartialEq)]
pub struct ValueSample {
    pub value: f64,
    pub raw_quantity: f64,
    pub display: String,
    pub effects: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ValueTable {
    expected: ExpectedObservation,
    request: MappingRequest,
    context: Vec<u8>,
    context_key: String,
    unit: String,
    samples: Vec<ValueSample>,
}
impl ValueTable {
    pub fn samples(&self) -> &[ValueSample] {
        &self.samples
    }
    pub fn sample(&self, roll: u8) -> &ValueSample {
        &self.samples[usize::from(roll)]
    }
    pub fn unit(&self) -> &str {
        &self.unit
    }
    pub fn context_key(&self) -> &str {
        &self.context_key
    }
    pub fn minimum(&self) -> f64 {
        self.samples.iter().map(|s| s.value).fold(f64::INFINITY, f64::min)
    }
    pub fn maximum(&self) -> f64 {
        self.samples.iter().map(|s| s.value).fold(f64::NEG_INFINITY, f64::max)
    }
    pub fn nearest(&self, value: f64, current: u8) -> Result<u8, MappingError> {
        if !value.is_finite() || value < self.minimum() || value > self.maximum() {
            return Err(MappingError::OutOfRange);
        }
        let mut best = 0usize;
        for (index, sample) in self.samples.iter().enumerate() {
            let distance = (sample.value - value).abs();
            let previous = (self.samples[best].value - value).abs();
            if distance < previous || distance == previous && index.abs_diff(usize::from(current)) < best.abs_diff(usize::from(current)) {
                best = index;
            }
        }
        Ok(best as u8)
    }
    pub fn recheck(&self, current: &ExpectedObservation, request: &MappingRequest) -> Result<(), MappingError> {
        if !self.expected.matches(current) {
            return Err(MappingError::WrongObservation);
        }
        if &self.request != request {
            return Err(MappingError::WrongContext);
        }
        Ok(())
    }
}

pub struct MappingBuilder {
    expected: ExpectedObservation,
    request: MappingRequest,
    language: String,
    context: Vec<u8>,
    unit: Option<String>,
    samples: Vec<ValueSample>,
    bytes: usize,
    failed: bool,
}
impl MappingBuilder {
    pub fn new(expected: ExpectedObservation, request: MappingRequest, context: RawMappingPage, tooltip: Option<&BoundTooltip>) -> Result<Self, MappingError> {
        request.validate(&expected, tooltip)?;
        let identity = expected.catalog_identity();
        if !eg_core::hashes::is_sha256(&identity.binary_hash)
            || !eg_core::hashes::is_sha256(&identity.metadata_hash)
            || identity.extraction_version != crate::catalog::EXTRACTION_VERSION
        {
            return Err(MappingError::InvalidData);
        }
        if context.start != MappingStart::ContextOnly || context.address != request.address || context.language != identity.locale.language {
            return Err(MappingError::WrongContext);
        }
        wire::encode_mapping(&context).map_err(|_| MappingError::InvalidData)?;
        Ok(Self {
            expected,
            request,
            language: context.language,
            context: context.context,
            unit: None,
            samples: vec![],
            bytes: 0,
            failed: false,
        })
    }
    pub fn next_start(&self) -> Option<u8> {
        (!self.failed && self.samples.len() < wire::SAMPLE_COUNT).then_some(self.samples.len() as u8)
    }
    pub fn push(&mut self, current: &ExpectedObservation, request: &MappingRequest, page: RawMappingPage, parser: &mut impl DisplayScalarParser) -> Result<(), MappingError> {
        let result = self.push_checked(current, request, page, parser);
        self.failed |= result.is_err();
        result
    }
    fn push_checked(&mut self, current: &ExpectedObservation, request: &MappingRequest, page: RawMappingPage, parser: &mut impl DisplayScalarParser) -> Result<(), MappingError> {
        if self.failed {
            return Err(MappingError::Poisoned);
        }
        if !self.expected.matches(current) {
            return Err(MappingError::WrongObservation);
        }
        if &self.request != request || page.address != self.request.address || page.language != self.language || page.context != self.context {
            return Err(MappingError::WrongContext);
        }
        if self.samples.len() >= wire::SAMPLE_COUNT || page.start != MappingStart::Samples(self.samples.len() as u8) || page.samples.len() != wire::PAGE_SIZE {
            return Err(MappingError::OutOfOrder);
        }
        let bytes = wire::encode_mapping(&page).map_err(|_| MappingError::InvalidData)?.len();
        self.bytes = self.bytes.checked_add(bytes).filter(|bytes| *bytes <= MAX_TABLE_BYTES).ok_or(MappingError::InvalidData)?;
        for sample in page.samples {
            if self.unit.as_ref().is_some_and(|unit| unit != &sample.unit) {
                return Err(MappingError::WrongContext);
            }
            self.unit.get_or_insert(sample.unit.clone());
            if !sample.unit.is_empty() && !sample.display.ends_with(&sample.unit) {
                return Err(MappingError::DisplayUnavailable("The display does not end in its exact unit.".into()));
            }
            let value = parser.parse(&self.language, &sample.display, &sample.unit).map_err(MappingError::DisplayUnavailable)?;
            if !value.is_finite() {
                return Err(MappingError::InvalidData);
            }
            self.samples.push(ValueSample {
                value,
                raw_quantity: sample.raw_quantity,
                display: sample.display,
                effects: sample.effects,
            });
        }
        Ok(())
    }
    pub fn finish(self) -> Result<ValueTable, MappingError> {
        if self.failed {
            return Err(MappingError::Poisoned);
        }
        if self.samples.len() != wire::SAMPLE_COUNT {
            return Err(MappingError::Incomplete);
        }
        if self.samples.iter().all(|sample| sample.value == self.samples[0].value) {
            return Err(MappingError::FixedValue);
        }
        if !self.samples.windows(2).all(|s| s[0].value <= s[1].value) && !self.samples.windows(2).all(|s| s[0].value >= s[1].value) {
            return Err(MappingError::NonMonotonic);
        }
        let line = self.request.address.line.map_or_else(String::new, |line| format!("line{line}:"));
        let context_key = format!(
            "{}:{}:{}:{}{}",
            self.language,
            self.request.address.target as i32,
            self.request.address.index,
            line,
            eg_core::hashes::of_bytes(&self.context)
        );
        Ok(ValueTable {
            expected: self.expected,
            request: self.request,
            context: self.context,
            context_key,
            unit: self.unit.unwrap_or_default(),
            samples: self.samples,
        })
    }
}
