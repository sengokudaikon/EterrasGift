//! Complete, owned definition catalogs and explicit forge-name resolution.
//!
//! A catalog is made whole from complete lists ([`GameCatalog::new`]), every entry checked; no partial catalog is
//! published and no name fallback overwrites the raw observation.

use crate::definitions::entries;
pub use crate::definitions::entries::{
    AffixDefinition, AffixEffect, AffixSource, AffixTier, Availability, CatalogEntries, DefinitionName, DefinitionNameSource, ImplicitDefinition, ItemTypeDefinition,
    NumericProperty, RarityDefinition, SubTypeDefinition, UniqueDefinition, UniqueNumericData,
};
pub use eg_trainer_wire::{
    definitions::{ItemLimits, ValueRange},
    locale::GameLocale,
};
use eg_trainer_wire::{forge::ForgeSnapshot, protocol::ProtocolError};
use std::collections::BTreeMap;

pub const EXTRACTION_VERSION: u32 = 8;
pub const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogIdentity {
    pub binary_hash: String,
    pub metadata_hash: String,
    pub locale: GameLocale,
    pub extraction_version: u32,
}

impl CatalogIdentity {
    /// Whether a catalog can have it: two SHA-256 hashes, a locale the wire can carry, and this extraction.
    pub fn check(&self) -> Result<(), ProtocolError> {
        if !file_ops::hashes::is_sha256(&self.binary_hash) || !file_ops::hashes::is_sha256(&self.metadata_hash) || self.extraction_version != EXTRACTION_VERSION {
            return Err(ProtocolError::InvalidData);
        }
        eg_trainer_wire::locale::encode(&self.locale).map(drop)
    }
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
    /// The catalog of complete lists: every entry meets its form's rules ([`entries`]) and no two of a kind share an ID.
    /// One that fails fails the catalog.
    pub fn new(identity: CatalogIdentity, limits: ItemLimits, entries: CatalogEntries) -> Result<Self, ProtocolError> {
        let catalog = Self {
            identity,
            limits,
            bases: keyed(entries.bases, |v| v.item_type)?,
            affixes: keyed(entries.affixes, |v| v.id)?,
            uniques: keyed(entries.uniques, |v| v.id)?,
            rarities: keyed(entries.rarities, |v| v.id)?,
        };
        catalog.check()?;
        Ok(catalog)
    }

    /// Whether it meets every rule [`Self::new`] checks. Its fields are public, so one changed or made by hand may not.
    pub fn check(&self) -> Result<(), ProtocolError> {
        self.check_keys()?;
        entries::check(self)
    }

    /// Its identity and limits, and each entry under its own ID: all but the entries' own rules, which writing them checks.
    pub(crate) fn check_keys(&self) -> Result<(), ProtocolError> {
        self.identity.check()?;
        eg_trainer_wire::definitions::encode(&self.limits)?;
        if self.bases.iter().any(|(k, v)| *k != v.item_type)
            || self.affixes.iter().any(|(k, v)| *k != v.id)
            || self.uniques.iter().any(|(k, v)| *k != v.id)
            || self.rarities.iter().any(|(k, v)| *k != v.id)
        {
            return Err(ProtocolError::InvalidData);
        }
        Ok(())
    }

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

/// `items` by their IDs, none twice.
fn keyed<K: Ord, T>(items: Vec<T>, id: impl Fn(&T) -> K) -> Result<BTreeMap<K, T>, ProtocolError> {
    let mut keyed = BTreeMap::new();
    for item in items {
        if keyed.insert(id(&item), item).is_some() {
            return Err(ProtocolError::InvalidData);
        }
    }
    Ok(keyed)
}
