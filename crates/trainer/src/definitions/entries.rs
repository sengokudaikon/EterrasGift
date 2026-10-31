//! The catalog's entries (bases, affixes, uniques, rarities) and the bounded page form the cache keeps them in.
//! Names retain their source; unavailable formatter/numeric data is explicit and never replaced with guessed values.
//!
//! Pages: LE u32 kind/cursor/total, locale language/version texts, entry count,
//! then typed entries in declaration order below. Texts and vectors have LE u32
//! lengths; integers/f32 are LE, bools are single 0/1 bytes. Availability is a
//! single byte (0 = reason text, 1 = value). Optional equipment lists use a 0/1
//! byte then the list when present, an optional affix source a 0/1 byte then
//! u16 unique and text line. DefinitionName is text then u32 source.
//! Pages contain exactly min(16, total-cursor) entries. The caller must pin
//! locale/total and reject duplicate identities across pages of one capture.
//! This schema preserves unknown enum/flag values and signed/reversed ranges.

use std::collections::BTreeSet;

pub use eg_trainer_wire::forge::MAX_IMPLICITS;
use eg_trainer_wire::{
    locale::GameLocale,
    protocol::{MAX_RESPONSE_SIZE, ProtocolError},
};

pub const MAX_PAGE_ENTRIES: usize = 16;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_SUB_TYPES: usize = 1024;
pub const MAX_EQUIPMENT_TYPES: usize = 128;
pub const MAX_AFFIX_TIERS: usize = 128;
pub const MAX_PROPERTIES: usize = 128;
pub const MAX_UNIQUE_SUB_TYPES: usize = 128;
pub const MAX_DESCRIPTIONS: usize = 128;
pub const MAX_DISPLAY_ORDER: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CatalogKind {
    Bases = 0,
    Affixes = 1,
    Uniques = 2,
    Rarities = 3,
}

impl CatalogKind {
    pub const fn maximum_total(self) -> u32 {
        match self {
            Self::Bases => 256,
            Self::Affixes => 20_000,
            Self::Uniques => 10_000,
            Self::Rarities => 10,
        }
    }
}

impl TryFrom<u32> for CatalogKind {
    type Error = ProtocolError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Bases),
            1 => Ok(Self::Affixes),
            2 => Ok(Self::Uniques),
            3 => Ok(Self::Rarities),
            _ => Err(ProtocolError::InvalidData),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability<T> {
    Available(T),
    Unavailable { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum DefinitionNameSource {
    RuntimeDisplay = 0,
    StoredDisplay = 1,
    InternalName = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionName {
    pub text: String,
    pub source: DefinitionNameSource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NumericProperty {
    pub property: u8,
    pub tags: i32,
    pub special_tag: u8,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub set_property: bool,
    pub minimum: f32,
    pub maximum: f32,
    pub roll_id: i32,
    pub rounding: i32,
    pub display_added_as_percentage: bool,
    pub display_added_as_tenth: bool,
    pub less_is_better: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AffixTier {
    pub properties: Vec<NumericProperty>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AffixDefinition {
    pub id: u16,
    pub raw_name: String,
    pub raw_display_name: String,
    pub game_display_name: Availability<String>,
    pub kind: i32,
    pub special: i32,
    pub class_requirement: i32,
    pub item_types: Option<Vec<i32>>,
    pub tier_count: u16,
    pub standard_effect: f32,
    pub numeric: Availability<Vec<AffixTier>>,
    /// The item level it needs: the game's own for a shipped affix, its source unique's for a unique modifier.
    pub level_requirement: i32,
    /// The unique it was made from; none for an affix the game ships.
    pub source: Option<AffixSource>,
}

/// Where a unique modifier comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixSource {
    pub unique: u16,
    /// The modifier's own words, without the unique's name.
    pub line: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniqueNumericData {
    pub properties: Vec<NumericProperty>,
    pub descriptions: Vec<String>,
    pub display_order: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniqueDefinition {
    pub id: u16,
    pub name: DefinitionName,
    pub item_type: u8,
    pub sub_types: Vec<u8>,
    pub is_set_item: bool,
    pub set_id: u8,
    pub legendary_type: i32,
    pub can_drop_as_legendary: bool,
    pub is_pre_corrupted: bool,
    pub numeric: Availability<UniqueNumericData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImplicitDefinition {
    pub property: u8,
    pub tags: i32,
    pub special_tag: u8,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub minimum: f32,
    pub maximum: f32,
    pub minimum_text: Availability<String>,
    pub maximum_text: Availability<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubTypeDefinition {
    pub sub_type: u16,
    pub name: DefinitionName,
    pub level_requirement: i32,
    pub class_requirement: i32,
    pub cannot_drop: bool,
    pub is_corrupted: bool,
    pub implicits: Availability<Vec<ImplicitDefinition>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemTypeDefinition {
    pub item_type: u8,
    pub name: DefinitionName,
    pub sub_types: Vec<SubTypeDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RarityDefinition {
    pub id: u8,
    pub name: Availability<DefinitionName>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CatalogEntries {
    Bases(Vec<ItemTypeDefinition>),
    Affixes(Vec<AffixDefinition>),
    Uniques(Vec<UniqueDefinition>),
    Rarities(Vec<RarityDefinition>),
}

impl CatalogEntries {
    pub fn len(&self) -> usize {
        match self {
            Self::Bases(v) => v.len(),
            Self::Affixes(v) => v.len(),
            Self::Uniques(v) => v.len(),
            Self::Rarities(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub const fn kind(&self) -> CatalogKind {
        match self {
            Self::Bases(_) => CatalogKind::Bases,
            Self::Affixes(_) => CatalogKind::Affixes,
            Self::Uniques(_) => CatalogKind::Uniques,
            Self::Rarities(_) => CatalogKind::Rarities,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogPage {
    pub locale: GameLocale,
    pub kind: CatalogKind,
    pub cursor: u32,
    pub total: u32,
    pub entries: CatalogEntries,
}

fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}

fn validate(page: &CatalogPage) -> Result<(), ProtocolError> {
    if page.kind != page.entries.kind()
        || page.total > page.kind.maximum_total()
        || page.cursor > page.total
        || page.entries.len() != (page.total - page.cursor).min(MAX_PAGE_ENTRIES as u32) as usize
        || page.locale.language.trim().is_empty()
    {
        return invalid();
    }
    let ids: Vec<u16> = match &page.entries {
        CatalogEntries::Bases(v) => {
            for base in v {
                if base.sub_types.iter().map(|s| s.sub_type).collect::<BTreeSet<_>>().len() != base.sub_types.len() {
                    return invalid();
                }
            }
            v.iter().map(|v| u16::from(v.item_type)).collect()
        }
        CatalogEntries::Affixes(v) => {
            for affix in v {
                if usize::from(affix.tier_count) > MAX_AFFIX_TIERS {
                    return invalid();
                }
                if let Availability::Available(tiers) = &affix.numeric
                    && tiers.len() != usize::from(affix.tier_count)
                {
                    return invalid();
                }
            }
            v.iter().map(|v| v.id).collect()
        }
        CatalogEntries::Uniques(v) => v.iter().map(|v| v.id).collect(),
        CatalogEntries::Rarities(v) => {
            if v.iter().any(|v| v.id >= 10) {
                return invalid();
            }
            v.iter().map(|v| u16::from(v.id)).collect()
        }
    };
    if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return invalid();
    }
    Ok(())
}

struct Writer(Vec<u8>);
impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if self.0.len().checked_add(bytes.len()).is_none_or(|len| len > MAX_RESPONSE_SIZE) {
            return invalid();
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn u8(&mut self, n: u8) -> Result<(), ProtocolError> {
        self.bytes(&[n])
    }
    fn u16(&mut self, n: u16) -> Result<(), ProtocolError> {
        self.bytes(&n.to_le_bytes())
    }
    fn u32(&mut self, n: u32) -> Result<(), ProtocolError> {
        self.bytes(&n.to_le_bytes())
    }
    fn i32(&mut self, n: i32) -> Result<(), ProtocolError> {
        self.bytes(&n.to_le_bytes())
    }
    fn float(&mut self, n: f32) -> Result<(), ProtocolError> {
        if !n.is_finite() {
            return invalid();
        }
        self.bytes(&n.to_le_bytes())
    }
    fn boolean(&mut self, n: bool) -> Result<(), ProtocolError> {
        self.u8(u8::from(n))
    }
    fn text(&mut self, text: &str) -> Result<(), ProtocolError> {
        if text.len() > MAX_TEXT_BYTES {
            return invalid();
        }
        self.u32(text.len() as u32)?;
        self.bytes(text.as_bytes())
    }
    fn list<T>(&mut self, items: &[T], cap: usize, mut write: impl FnMut(&mut Self, &T) -> Result<(), ProtocolError>) -> Result<(), ProtocolError> {
        if items.len() > cap {
            return invalid();
        }
        self.u32(items.len() as u32)?;
        for item in items {
            write(self, item)?;
        }
        Ok(())
    }
    fn available<T>(&mut self, item: &Availability<T>, write: impl FnOnce(&mut Self, &T) -> Result<(), ProtocolError>) -> Result<(), ProtocolError> {
        match item {
            Availability::Available(value) => {
                self.u8(1)?;
                write(self, value)
            }
            Availability::Unavailable { reason } => {
                if reason.trim().is_empty() {
                    return invalid();
                }
                self.u8(0)?;
                self.text(reason)
            }
        }
    }
    fn name(&mut self, name: &DefinitionName) -> Result<(), ProtocolError> {
        self.text(&name.text)?;
        self.u32(name.source as u32)
    }
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let (head, tail) = self.0.split_at_checked(N).ok_or(ProtocolError::InvalidData)?;
        self.0 = tail;
        head.try_into().map_err(|_| ProtocolError::InvalidData)
    }
    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn i32(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }
    fn float(&mut self) -> Result<f32, ProtocolError> {
        let n = f32::from_le_bytes(self.bytes()?);
        if !n.is_finite() {
            return invalid();
        }
        Ok(n)
    }
    fn boolean(&mut self) -> Result<bool, ProtocolError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => invalid(),
        }
    }
    fn text(&mut self) -> Result<String, ProtocolError> {
        let len = self.u32()? as usize;
        if len > MAX_TEXT_BYTES {
            return invalid();
        }
        let (head, tail) = self.0.split_at_checked(len).ok_or(ProtocolError::InvalidData)?;
        self.0 = tail;
        std::str::from_utf8(head).map(str::to_owned).map_err(|_| ProtocolError::InvalidData)
    }
    fn list<T>(&mut self, cap: usize, mut read: impl FnMut(&mut Self) -> Result<T, ProtocolError>) -> Result<Vec<T>, ProtocolError> {
        let len = self.u32()? as usize;
        if len > cap {
            return invalid();
        }
        (0..len).map(|_| read(self)).collect()
    }
    fn available<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T, ProtocolError>) -> Result<Availability<T>, ProtocolError> {
        match self.u8()? {
            0 => {
                let reason = self.text()?;
                if reason.trim().is_empty() {
                    return invalid();
                }
                Ok(Availability::Unavailable { reason })
            }
            1 => Ok(Availability::Available(read(self)?)),
            _ => invalid(),
        }
    }
    fn name(&mut self) -> Result<DefinitionName, ProtocolError> {
        let text = self.text()?;
        let source = match self.u32()? {
            0 => DefinitionNameSource::RuntimeDisplay,
            1 => DefinitionNameSource::StoredDisplay,
            2 => DefinitionNameSource::InternalName,
            _ => return invalid(),
        };
        Ok(DefinitionName { text, source })
    }
}

fn write_property(w: &mut Writer, p: &NumericProperty) -> Result<(), ProtocolError> {
    if !(-1..=7).contains(&p.roll_id) {
        return invalid();
    }
    w.u8(p.property)?;
    w.i32(p.tags)?;
    w.u8(p.special_tag)?;
    w.i32(p.extra_tag)?;
    w.i32(p.modifier_type)?;
    w.boolean(p.set_property)?;
    w.float(p.minimum)?;
    w.float(p.maximum)?;
    w.i32(p.roll_id)?;
    w.i32(p.rounding)?;
    w.boolean(p.display_added_as_percentage)?;
    w.boolean(p.display_added_as_tenth)?;
    w.boolean(p.less_is_better)
}
fn read_property(r: &mut Reader<'_>) -> Result<NumericProperty, ProtocolError> {
    let p = NumericProperty {
        property: r.u8()?,
        tags: r.i32()?,
        special_tag: r.u8()?,
        extra_tag: r.i32()?,
        modifier_type: r.i32()?,
        set_property: r.boolean()?,
        minimum: r.float()?,
        maximum: r.float()?,
        roll_id: r.i32()?,
        rounding: r.i32()?,
        display_added_as_percentage: r.boolean()?,
        display_added_as_tenth: r.boolean()?,
        less_is_better: r.boolean()?,
    };
    if !(-1..=7).contains(&p.roll_id) {
        return invalid();
    }
    Ok(p)
}
fn write_implicit(w: &mut Writer, p: &ImplicitDefinition) -> Result<(), ProtocolError> {
    w.u8(p.property)?;
    w.i32(p.tags)?;
    w.u8(p.special_tag)?;
    w.i32(p.extra_tag)?;
    w.i32(p.modifier_type)?;
    w.float(p.minimum)?;
    w.float(p.maximum)?;
    w.available(&p.minimum_text, |w, s| w.text(s))?;
    w.available(&p.maximum_text, |w, s| w.text(s))
}
fn read_implicit(r: &mut Reader<'_>) -> Result<ImplicitDefinition, ProtocolError> {
    Ok(ImplicitDefinition {
        property: r.u8()?,
        tags: r.i32()?,
        special_tag: r.u8()?,
        extra_tag: r.i32()?,
        modifier_type: r.i32()?,
        minimum: r.float()?,
        maximum: r.float()?,
        minimum_text: r.available(SelfReader::text)?,
        maximum_text: r.available(SelfReader::text)?,
    })
}
// This alias keeps method references used by the bounded nested readers concise.
type SelfReader<'a> = Reader<'a>;

fn write_base(w: &mut Writer, base: &ItemTypeDefinition) -> Result<(), ProtocolError> {
    w.u8(base.item_type)?;
    w.name(&base.name)?;
    w.list(&base.sub_types, MAX_SUB_TYPES, |w, s| {
        w.u16(s.sub_type)?;
        w.name(&s.name)?;
        w.i32(s.level_requirement)?;
        w.i32(s.class_requirement)?;
        w.boolean(s.cannot_drop)?;
        w.boolean(s.is_corrupted)?;
        w.available(&s.implicits, |w, v| w.list(v, MAX_IMPLICITS, write_implicit))
    })
}
fn read_base(r: &mut Reader<'_>) -> Result<ItemTypeDefinition, ProtocolError> {
    Ok(ItemTypeDefinition {
        item_type: r.u8()?,
        name: r.name()?,
        sub_types: r.list(MAX_SUB_TYPES, |r| {
            Ok(SubTypeDefinition {
                sub_type: r.u16()?,
                name: r.name()?,
                level_requirement: r.i32()?,
                class_requirement: r.i32()?,
                cannot_drop: r.boolean()?,
                is_corrupted: r.boolean()?,
                implicits: r.available(|r| r.list(MAX_IMPLICITS, read_implicit))?,
            })
        })?,
    })
}
fn write_affix(w: &mut Writer, a: &AffixDefinition) -> Result<(), ProtocolError> {
    w.u16(a.id)?;
    w.text(&a.raw_name)?;
    w.text(&a.raw_display_name)?;
    w.available(&a.game_display_name, |w, s| w.text(s))?;
    w.i32(a.kind)?;
    w.i32(a.special)?;
    w.i32(a.class_requirement)?;
    match &a.item_types {
        None => w.u8(0)?,
        Some(v) => {
            w.u8(1)?;
            w.list(v, MAX_EQUIPMENT_TYPES, |w, n| w.i32(*n))?;
        }
    }
    w.u16(a.tier_count)?;
    w.float(a.standard_effect)?;
    w.available(&a.numeric, |w, v| w.list(v, MAX_AFFIX_TIERS, |w, t| w.list(&t.properties, MAX_PROPERTIES, write_property)))?;
    w.i32(a.level_requirement)?;
    match &a.source {
        None => w.u8(0),
        Some(source) => {
            w.u8(1)?;
            w.u16(source.unique)?;
            w.text(&source.line)
        }
    }
}
fn read_affix(r: &mut Reader<'_>) -> Result<AffixDefinition, ProtocolError> {
    Ok(AffixDefinition {
        id: r.u16()?,
        raw_name: r.text()?,
        raw_display_name: r.text()?,
        game_display_name: r.available(SelfReader::text)?,
        kind: r.i32()?,
        special: r.i32()?,
        class_requirement: r.i32()?,
        item_types: match r.u8()? {
            0 => None,
            1 => Some(r.list(MAX_EQUIPMENT_TYPES, SelfReader::i32)?),
            _ => return invalid(),
        },
        tier_count: r.u16()?,
        standard_effect: r.float()?,
        numeric: r.available(|r| {
            r.list(MAX_AFFIX_TIERS, |r| {
                Ok(AffixTier {
                    properties: r.list(MAX_PROPERTIES, read_property)?,
                })
            })
        })?,
        level_requirement: r.i32()?,
        source: match r.u8()? {
            0 => None,
            1 => Some(AffixSource {
                unique: r.u16()?,
                line: r.text()?,
            }),
            _ => return invalid(),
        },
    })
}
fn write_unique(w: &mut Writer, u: &UniqueDefinition) -> Result<(), ProtocolError> {
    w.u16(u.id)?;
    w.name(&u.name)?;
    w.u8(u.item_type)?;
    w.list(&u.sub_types, MAX_UNIQUE_SUB_TYPES, |w, n| w.u8(*n))?;
    w.boolean(u.is_set_item)?;
    w.u8(u.set_id)?;
    w.i32(u.legendary_type)?;
    w.boolean(u.can_drop_as_legendary)?;
    w.boolean(u.is_pre_corrupted)?;
    w.available(&u.numeric, |w, n| {
        w.list(&n.properties, MAX_PROPERTIES, write_property)?;
        w.list(&n.descriptions, MAX_DESCRIPTIONS, |w, s| w.text(s))?;
        w.list(&n.display_order, MAX_DISPLAY_ORDER, |w, n| w.i32(*n))
    })
}
fn read_unique(r: &mut Reader<'_>) -> Result<UniqueDefinition, ProtocolError> {
    Ok(UniqueDefinition {
        id: r.u16()?,
        name: r.name()?,
        item_type: r.u8()?,
        sub_types: r.list(MAX_UNIQUE_SUB_TYPES, SelfReader::u8)?,
        is_set_item: r.boolean()?,
        set_id: r.u8()?,
        legendary_type: r.i32()?,
        can_drop_as_legendary: r.boolean()?,
        is_pre_corrupted: r.boolean()?,
        numeric: r.available(|r| {
            Ok(UniqueNumericData {
                properties: r.list(MAX_PROPERTIES, read_property)?,
                descriptions: r.list(MAX_DESCRIPTIONS, SelfReader::text)?,
                display_order: r.list(MAX_DISPLAY_ORDER, SelfReader::i32)?,
            })
        })?,
    })
}

pub fn encode(page: &CatalogPage) -> Result<Vec<u8>, ProtocolError> {
    validate(page)?;
    let mut w = Writer(Vec::new());
    w.u32(page.kind as u32)?;
    w.u32(page.cursor)?;
    w.u32(page.total)?;
    w.text(&page.locale.language)?;
    w.text(&page.locale.game_version)?;
    match &page.entries {
        CatalogEntries::Bases(v) => w.list(v, MAX_PAGE_ENTRIES, write_base)?,
        CatalogEntries::Affixes(v) => w.list(v, MAX_PAGE_ENTRIES, write_affix)?,
        CatalogEntries::Uniques(v) => w.list(v, MAX_PAGE_ENTRIES, write_unique)?,
        CatalogEntries::Rarities(v) => w.list(v, MAX_PAGE_ENTRIES, |w, v| {
            w.u8(v.id)?;
            w.available(&v.name, Writer::name)
        })?,
    }
    Ok(w.0)
}

pub fn decode(payload: &[u8]) -> Result<CatalogPage, ProtocolError> {
    if payload.len() > MAX_RESPONSE_SIZE {
        return invalid();
    }
    let mut r = Reader(payload);
    let kind = CatalogKind::try_from(r.u32()?)?;
    let cursor = r.u32()?;
    let total = r.u32()?;
    // Reject impossible headers before reading or allocating any nested entries.
    if total > kind.maximum_total() || cursor > total {
        return invalid();
    }
    let locale = GameLocale {
        language: r.text()?,
        game_version: r.text()?,
    };
    let entries = match kind {
        CatalogKind::Bases => CatalogEntries::Bases(r.list(MAX_PAGE_ENTRIES, read_base)?),
        CatalogKind::Affixes => CatalogEntries::Affixes(r.list(MAX_PAGE_ENTRIES, read_affix)?),
        CatalogKind::Uniques => CatalogEntries::Uniques(r.list(MAX_PAGE_ENTRIES, read_unique)?),
        CatalogKind::Rarities => CatalogEntries::Rarities(r.list(MAX_PAGE_ENTRIES, |r| {
            Ok(RarityDefinition {
                id: r.u8()?,
                name: r.available(SelfReader::name)?,
            })
        })?),
    };
    if !r.0.is_empty() {
        return invalid();
    }
    let page = CatalogPage {
        locale,
        kind,
        cursor,
        total,
        entries,
    };
    validate(&page)?;
    Ok(page)
}
