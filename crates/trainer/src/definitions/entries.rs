//! The catalog's entries (bases, affixes, uniques, rarities), the rules each meets, and the form the cache keeps them in.
//! Names retain their source; unavailable formatter/numeric data is explicit and never replaced with guessed values.
//!
//! The form: the four lists in that order, each an LE u32 count then its entries in declaration order below. Texts and
//! vectors have LE u32 lengths; integers/f32 are LE, bools are single 0/1 bytes. Availability is a single byte (0 =
//! reason text, 1 = value). Optional equipment lists use a 0/1 byte then the list when present, an optional affix source
//! a 0/1 byte then u16 unique and text line, an affix's effects a list of three texts and two bools. DefinitionName is
//! text then u32 source. This schema preserves unknown enum/flag values and signed/reversed ranges.
//!
//! The rules are the form's: writing an entry checks it, and a catalog is checked by measuring what it would write
//! (`check`), so a catalog that exists is one the cache can keep.

use std::collections::BTreeSet;

use crate::definitions::catalog::{GameCatalog, MAX_CATALOG_BYTES};
pub use eg_trainer_wire::forge::MAX_IMPLICITS;
use eg_trainer_wire::protocol::ProtocolError;

pub const MAX_BASES: usize = 256;
pub const MAX_AFFIXES: usize = 20_000;
pub const MAX_UNIQUES: usize = 10_000;
/// The game's rarities, numbered below this.
pub const MAX_RARITIES: usize = 10;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_SUB_TYPES: usize = 1024;
pub const MAX_EQUIPMENT_TYPES: usize = 128;
pub const MAX_AFFIX_TIERS: usize = 128;
pub const MAX_PROPERTIES: usize = 128;
pub const MAX_UNIQUE_SUB_TYPES: usize = 128;
pub const MAX_DESCRIPTIONS: usize = 128;
pub const MAX_DISPLAY_ORDER: usize = 128;

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
    /// The game's words for each of its stat lines, beside `numeric`'s properties, in the session's language.
    pub effects: Vec<AffixEffect>,
    /// Its name says where it is from and nothing of what it does ("Ucenui's"): the game's name is the same as its title.
    pub named_by_title: bool,
}

/// One stat line in the game's words. A text the game lacks is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixEffect {
    /// The line named by itself: "Cold Penetration".
    pub heading: String,
    /// What a tooltip writes after the value: "increased Mana", "to All Minion Skills".
    pub words: String,
    /// The game's longer explanation.
    pub explanation: String,
    pub hides_value: bool,
    pub no_plus: bool,
}

/// Where a unique modifier comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixSource {
    pub unique: u16,
    /// The modifier's own words, without the unique's name; empty where the unique has no text for it and `effects` say it.
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
    /// What this base does to an affix's values: `value * (1 + this) / (1 + the affix's standard_effect)`.
    pub affix_effect_modifier: f32,
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

/// A catalog's definitions, each kind in a list of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogEntries {
    pub bases: Vec<ItemTypeDefinition>,
    pub affixes: Vec<AffixDefinition>,
    pub uniques: Vec<UniqueDefinition>,
    pub rarities: Vec<RarityDefinition>,
}

fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}

/// Checks `catalog`'s entries against every rule of their form, by measuring it: nothing is written.
pub(crate) fn check(catalog: &GameCatalog) -> Result<(), ProtocolError> {
    write(&mut Writer { out: None, len: 0 }, catalog)
}

/// `catalog`'s entries in their form, each checked as it is written.
pub fn encode(catalog: &GameCatalog) -> Result<Vec<u8>, ProtocolError> {
    let mut w = Writer { out: Some(Vec::new()), len: 0 };
    write(&mut w, catalog)?;
    Ok(w.out.unwrap_or_default())
}

/// Entries back from their form, bounded before anything is allocated. Whether they make a catalog is
/// [`GameCatalog::new`]'s to say.
pub fn decode(bytes: &[u8]) -> Result<CatalogEntries, ProtocolError> {
    if bytes.len() > MAX_CATALOG_BYTES {
        return invalid();
    }
    let mut r = Reader(bytes);
    let entries = CatalogEntries {
        bases: r.list(MAX_BASES, read_base)?,
        affixes: r.list(MAX_AFFIXES, read_affix)?,
        uniques: r.list(MAX_UNIQUES, read_unique)?,
        rarities: r.list(MAX_RARITIES, |r| {
            Ok(RarityDefinition {
                id: r.u8()?,
                name: r.available(SelfReader::name)?,
            })
        })?,
    };
    if !r.0.is_empty() {
        return invalid();
    }
    Ok(entries)
}

fn write(w: &mut Writer, catalog: &GameCatalog) -> Result<(), ProtocolError> {
    w.list(catalog.bases.values(), MAX_BASES, write_base)?;
    w.list(catalog.affixes.values(), MAX_AFFIXES, write_affix)?;
    w.list(catalog.uniques.values(), MAX_UNIQUES, write_unique)?;
    w.list(catalog.rarities.values(), MAX_RARITIES, |w, v| {
        if usize::from(v.id) >= MAX_RARITIES {
            return invalid();
        }
        w.u8(v.id)?;
        w.available(&v.name, Writer::name)
    })
}

/// Writes the form, or with no `out` only measures it, so checking and saving apply one set of rules. The whole stays
/// within [`MAX_CATALOG_BYTES`].
struct Writer {
    out: Option<Vec<u8>>,
    len: usize,
}
impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        self.len = self
            .len
            .checked_add(bytes.len())
            .filter(|len| *len <= MAX_CATALOG_BYTES)
            .ok_or(ProtocolError::InvalidData)?;
        if let Some(out) = &mut self.out {
            out.extend_from_slice(bytes);
        }
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
    fn list<'a, T: 'a, I>(&mut self, items: I, cap: usize, mut write: impl FnMut(&mut Self, &T) -> Result<(), ProtocolError>) -> Result<(), ProtocolError>
    where
        I: IntoIterator<Item = &'a T, IntoIter: ExactSizeIterator>,
    {
        let items = items.into_iter();
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
    if base.sub_types.iter().map(|s| s.sub_type).collect::<BTreeSet<_>>().len() != base.sub_types.len() {
        return invalid();
    }
    w.u8(base.item_type)?;
    w.name(&base.name)?;
    w.list(&base.sub_types, MAX_SUB_TYPES, |w, s| {
        w.u16(s.sub_type)?;
        w.name(&s.name)?;
        w.i32(s.level_requirement)?;
        w.i32(s.class_requirement)?;
        w.boolean(s.cannot_drop)?;
        w.boolean(s.is_corrupted)?;
        w.available(&s.implicits, |w, v| w.list(v, MAX_IMPLICITS, write_implicit))?;
        w.float(s.affix_effect_modifier)
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
                affix_effect_modifier: r.float()?,
            })
        })?,
    })
}
fn write_affix(w: &mut Writer, a: &AffixDefinition) -> Result<(), ProtocolError> {
    if usize::from(a.tier_count) > MAX_AFFIX_TIERS || matches!(&a.numeric, Availability::Available(tiers) if tiers.len() != usize::from(a.tier_count)) {
        return invalid();
    }
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
        None => w.u8(0)?,
        Some(source) => {
            w.u8(1)?;
            w.u16(source.unique)?;
            w.text(&source.line)?;
        }
    }
    w.list(&a.effects, MAX_PROPERTIES, |w, effect| {
        w.text(&effect.heading)?;
        w.text(&effect.words)?;
        w.text(&effect.explanation)?;
        w.boolean(effect.hides_value)?;
        w.boolean(effect.no_plus)
    })?;
    w.boolean(a.named_by_title)
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
        effects: r.list(MAX_PROPERTIES, |r| {
            Ok(AffixEffect {
                heading: r.text()?,
                words: r.text()?,
                explanation: r.text()?,
                hides_value: r.boolean()?,
                no_plus: r.boolean()?,
            })
        })?,
        named_by_title: r.boolean()?,
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
