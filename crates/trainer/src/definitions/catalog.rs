//! Complete, owned definition captures and explicit forge-name resolution.
//!
//! Page identity and population are pinned, but this is not an atomic game snapshot.
//! The adapter requires definition graphs to remain immutable during a capture.
//! No partial catalog is published and no name fallback overwrites the raw observation.

use eg_trainer_wire::{catalog, forge::ForgeSnapshot, protocol::ProtocolError};
pub use eg_trainer_wire::{
    catalog::{
        AffixDefinition, AffixTier, Availability, CatalogEntries, CatalogKind, CatalogPage, DefinitionName, DefinitionNameSource, ImplicitDefinition, ItemTypeDefinition,
        NumericProperty, RarityDefinition, SubTypeDefinition, UniqueDefinition, UniqueNumericData,
    },
    definitions::{ItemLimits, ValueRange},
    locale::GameLocale,
};
use std::collections::{BTreeMap, BTreeSet};

pub const EXTRACTION_VERSION: u32 = 4;
pub const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogIdentity {
    pub binary_hash: String,
    pub metadata_hash: String,
    pub locale: GameLocale,
    pub extraction_version: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameCatalog {
    pub identity: CatalogIdentity,
    pub limits: ItemLimits,
    pub bases: BTreeMap<u8, ItemTypeDefinition>,
    pub affixes: BTreeMap<u16, AffixDefinition>,
    pub uniques: BTreeMap<u16, UniqueDefinition>,
    pub rarities: BTreeMap<u8, RarityDefinition>,
}

/// A base-name fallback identifies the base only: it does not reproduce affix titles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemNameSource {
    RuntimeCache,
    UniqueDefinition { source: DefinitionNameSource },
    BaseDefinition { source: DefinitionNameSource },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedItemName {
    pub text: String,
    pub source: ItemNameSource,
}

impl GameCatalog {
    pub fn resolve_item_name(&self, item: &ForgeSnapshot) -> Option<ResolvedItemName> {
        if !item.name.is_empty() {
            return Some(ResolvedItemName {
                text: item.name.clone(),
                source: ItemNameSource::RuntimeCache,
            });
        }
        if let Some(identity) = &item.identity {
            // Never label an item with an unrelated unique definition. A missing/mismatched
            // unique remains unresolved rather than quietly falling back to a generic base.
            let unique = self.uniques.get(&identity.id)?;
            if unique.item_type != item.item_type
                || !unique.sub_types.contains(&u8::try_from(item.sub_type).ok()?)
                || unique.is_set_item != (item.rarity == 8)
                || unique.name.text.is_empty()
            {
                return None;
            }
            return Some(ResolvedItemName {
                text: unique.name.text.clone(),
                source: ItemNameSource::UniqueDefinition { source: unique.name.source },
            });
        }
        let base = self.bases.get(&item.item_type)?;
        let sub = base.sub_types.iter().find(|sub| sub.sub_type == item.sub_type)?;
        (!sub.name.text.is_empty()).then(|| ResolvedItemName {
            text: sub.name.text.clone(),
            source: ItemNameSource::BaseDefinition { source: sub.name.source },
        })
    }
}

#[derive(Default)]
struct Progress {
    total: Option<u32>,
    cursor: u32,
    ids: BTreeSet<u16>,
}

/// Consumes canonical pages in cursor order for each kind; a rejected page poisons the builder.
/// There is no retry or recovery that could combine fragments from two failed captures.
pub struct CatalogBuilder {
    catalog: GameCatalog,
    progress: [Progress; 4],
    bytes: usize,
    failed: bool,
}

impl CatalogBuilder {
    pub fn new(identity: CatalogIdentity, limits: ItemLimits) -> Result<Self, ProtocolError> {
        if !eg_core::hashes::is_sha256(&identity.binary_hash) || !eg_core::hashes::is_sha256(&identity.metadata_hash) || identity.extraction_version != EXTRACTION_VERSION {
            return Err(ProtocolError::InvalidData);
        }
        eg_trainer_wire::locale::encode(&identity.locale)?;
        eg_trainer_wire::definitions::encode(&limits)?;
        Ok(Self {
            catalog: GameCatalog {
                identity,
                limits,
                bases: BTreeMap::new(),
                affixes: BTreeMap::new(),
                uniques: BTreeMap::new(),
                rarities: BTreeMap::new(),
            },
            progress: std::array::from_fn(|_| Progress::default()),
            bytes: 0,
            failed: false,
        })
    }

    pub fn push(&mut self, page: CatalogPage) -> Result<(), ProtocolError> {
        let result = self.push_checked(page);
        self.failed |= result.is_err();
        result
    }

    fn push_checked(&mut self, page: CatalogPage) -> Result<(), ProtocolError> {
        if self.failed || page.locale != self.catalog.identity.locale {
            return Err(ProtocolError::InvalidData);
        }
        // The encoder applies the same complete validation to locally constructed pages as
        // the wire decoder. Its bounded size also supplies a conservative aggregate budget.
        let bytes = catalog::encode(&page)?.len();
        self.bytes = self.bytes.checked_add(bytes).filter(|n| *n <= MAX_CATALOG_BYTES).ok_or(ProtocolError::InvalidData)?;
        let progress = &mut self.progress[page.kind as usize];
        if progress.total.is_some_and(|total| total != page.total) || page.cursor != progress.cursor || progress.total.is_some_and(|total| progress.cursor == total) {
            return Err(ProtocolError::InvalidData);
        }
        progress.total = Some(page.total);
        let ids: Vec<u16> = match &page.entries {
            CatalogEntries::Bases(v) => v.iter().map(|v| u16::from(v.item_type)).collect(),
            CatalogEntries::Affixes(v) => v.iter().map(|v| v.id).collect(),
            CatalogEntries::Uniques(v) => v.iter().map(|v| v.id).collect(),
            CatalogEntries::Rarities(v) => v.iter().map(|v| u16::from(v.id)).collect(),
        };
        for id in ids {
            if !progress.ids.insert(id) {
                return Err(ProtocolError::InvalidData);
            }
        }
        progress.cursor += page.entries.len() as u32;
        match page.entries {
            CatalogEntries::Bases(v) => self.catalog.bases.extend(v.into_iter().map(|v| (v.item_type, v))),
            CatalogEntries::Affixes(v) => self.catalog.affixes.extend(v.into_iter().map(|v| (v.id, v))),
            CatalogEntries::Uniques(v) => self.catalog.uniques.extend(v.into_iter().map(|v| (v.id, v))),
            CatalogEntries::Rarities(v) => self.catalog.rarities.extend(v.into_iter().map(|v| (v.id, v))),
        }
        Ok(())
    }

    pub fn next_cursor(&self, kind: CatalogKind) -> Option<u32> {
        let progress = &self.progress[kind as usize];
        (!self.failed && progress.total.is_none_or(|total| progress.cursor < total)).then_some(progress.cursor)
    }

    pub fn finish(self) -> Result<GameCatalog, ProtocolError> {
        if self.failed || self.progress.iter().any(|p| p.total != Some(p.cursor)) {
            return Err(ProtocolError::InvalidData);
        }
        Ok(self.catalog)
    }
}
