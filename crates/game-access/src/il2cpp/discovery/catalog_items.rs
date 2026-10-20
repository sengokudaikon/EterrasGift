//! Affix and unique identity capture from already-loaded definition assets.
//!
//! Numeric presentation is copied from ready PropertyList/special registries. Ailment dictionary
//! entries use audited framework Array.GetValue boxing; game formatters remain explicit unknowns.
//! Raw display fields retain their provenance and do not stand in for generated text.

use super::{DiscoveryError, Images, RuntimeAccess, catalog::page_bounds, forge::Root, unsupported};
use crate::{
    catalog::{AffixDefinition, Availability, CatalogEntries, DefinitionName, DefinitionNameSource, RarityDefinition, UniqueDefinition},
    il2cpp::{AttachedThread, raw},
};

impl RuntimeAccess {
    // These helpers inherit read_catalog_page's main-thread, nonmoving-GC, initialized-runtime,
    // exclusive immutable graph and no-shutdown contract, including its audited framework boxing.
    pub(super) unsafe fn catalog_affixes(
        &self,
        attached: &AttachedThread<'_>,
        images: &Images,
        cursor: u32,
        api: raw::InvocationApi,
    ) -> Result<(u32, CatalogEntries), DiscoveryError> {
        unsafe {
            let owner = self.class(images, c"", c"AffixList")?;
            let globals = self.class(images, c"LE.AssetManagement", c"GlobalAssets")?;
            let master = self
                .catalog_static(attached, globals, c"_storage_MasterAffixesList", owner)?
                .ok_or(DiscoveryError::NotReady("affix asset is not already loaded"))?;
            self.exact(&master, owner)?;
            if !self.catalog_live(attached, images, &master)? {
                return Err(DiscoveryError::NotReady("affix asset native object is unavailable"));
            }
            let single = self.catalog_nested(owner, c"SingleAffix")?;
            let multi = self.catalog_nested(owner, c"MultiAffix")?;
            let tier = self.catalog_nested(owner, c"Tier")?;
            let equipment = self.class(images, c"", c"EquipmentType")?;
            let mut definitions = Vec::new();
            for (field, class) in [(c"singleAffixes", single), (c"multiAffixes", multi)] {
                let array = self
                    .catalog_reference(attached, &master, field)?
                    .ok_or(DiscoveryError::NotReady("affix definition array unavailable"))?;
                for definition in self.catalog_array(attached, &array, class, 10000)?.into_iter().flatten() {
                    self.exact(&definition, class)?;
                    definitions.push(definition);
                }
            }
            let total = definitions.len() as u32;
            if total == 0 {
                return Err(DiscoveryError::NotReady("affix definitions are empty"));
            }
            let mut entries = Vec::new();
            let mut numeric = self.catalog_numeric(attached, images, api)?;
            for index in page_bounds(total, cursor)? {
                let definition = &definitions[index];
                let item_types = self
                    .catalog_reference(attached, definition, c"canRollOn")?
                    .map(|list| self.catalog_values::<i32>(attached, &list, equipment, crate::catalog::MAX_EQUIPMENT_TYPES as u32))
                    .transpose()?;
                let tiers = self
                    .catalog_reference(attached, definition, c"tiers")?
                    .map(|list| self.catalog_list(attached, &list, tier, 128))
                    .transpose()?
                    .unwrap_or_default();
                let id: i32 = self.primitive(definition, c"affixId")?;
                entries.push(AffixDefinition {
                    id: id.try_into().map_err(|_| unsupported("affix definition ID"))?,
                    raw_name: self.text(attached, definition, c"affixName")?,
                    raw_display_name: self.text(attached, definition, c"affixDisplayName")?,
                    game_display_name: Availability::Unavailable {
                        reason: "The generated affix display-name formatter has not been validated".into(),
                    },
                    kind: self.enumeration(definition, c"type", c"AffixType")?,
                    special: self.enumeration(definition, c"specialAffixType", c"SpecialAffixType")?,
                    class_requirement: self.enumeration(definition, c"classSpecificity", c"ClassSpecificity")?,
                    item_types,
                    tier_count: tiers.len() as u16,
                    standard_effect: self.primitive(definition, c"standardAffixEffectModifier")?,
                    numeric: self.catalog_affix_numeric(attached, images, definition, &tiers, &mut numeric)?,
                });
            }
            Ok((total, CatalogEntries::Affixes(entries)))
        }
    }

    pub(super) unsafe fn catalog_uniques(
        &self,
        attached: &AttachedThread<'_>,
        images: &Images,
        cursor: u32,
        api: raw::InvocationApi,
    ) -> Result<(u32, CatalogEntries), DiscoveryError> {
        unsafe {
            let owner = self.class(images, c"", c"UniqueList")?;
            let master = self
                .catalog_static(attached, owner, c"instance", owner)?
                .ok_or(DiscoveryError::NotReady("unique asset is not already loaded"))?;
            self.exact(&master, owner)?;
            if !self.catalog_live(attached, images, &master)? {
                return Err(DiscoveryError::NotReady("unique asset native object is unavailable"));
            }
            let entry = self.catalog_nested(owner, c"Entry")?;
            let list = self
                .catalog_reference(attached, &master, c"uniques")?
                .ok_or(DiscoveryError::NotReady("unique definitions unavailable"))?;
            let definitions = self.catalog_list(attached, &list, entry, 10000)?;
            let total = definitions.len() as u32;
            if total == 0 {
                return Err(DiscoveryError::NotReady("unique definitions are empty"));
            }
            let mut entries = Vec::new();
            let mut numeric = self.catalog_numeric(attached, images, api)?;
            for index in page_bounds(total, cursor)? {
                let definition = &definitions[index];
                self.exact(definition, entry)?;
                let sub_types = self
                    .catalog_reference(attached, definition, c"subTypes")?
                    .ok_or_else(|| unsupported("unique subtypes missing"))?;
                entries.push(UniqueDefinition {
                    id: self.primitive(definition, c"uniqueID")?,
                    name: self.catalog_stored_name(attached, definition)?,
                    item_type: self.primitive(definition, c"baseType")?,
                    sub_types: self.catalog_values(attached, &sub_types, self.system_class(c"Byte")?, crate::catalog::MAX_UNIQUE_SUB_TYPES as u32)?,
                    is_set_item: self.boolean(definition, c"isSetItem")?,
                    set_id: self.primitive(definition, c"setID")?,
                    legendary_type: self.enumeration(definition, c"legendaryType", c"LegendaryType")?,
                    can_drop_as_legendary: self.boolean(definition, c"canDropAsLegendary")?,
                    is_pre_corrupted: self.boolean(definition, c"isPreCorrupted")?,
                    numeric: self.catalog_unique_numeric(attached, images, definition, &mut numeric)?,
                });
            }
            Ok((total, CatalogEntries::Uniques(entries)))
        }
    }

    pub(super) unsafe fn catalog_stored_name(&self, attached: &AttachedThread<'_>, object: &Root<'_, '_>) -> Result<DefinitionName, DiscoveryError> {
        unsafe {
            // UniqueList.Entry.getDisplayName (current audited native body) chooses displayName
            // unless null/empty, then name. SubItem uses the same rule. Preserve whitespace.
            let display = self.text(attached, object, c"displayName")?;
            if !display.is_empty() {
                Ok(DefinitionName {
                    text: display,
                    source: DefinitionNameSource::StoredDisplay,
                })
            } else {
                Ok(DefinitionName {
                    text: self.text(attached, object, c"name")?,
                    source: DefinitionNameSource::InternalName,
                })
            }
        }
    }

    pub(super) unsafe fn catalog_rarities(&self, cursor: u32) -> Result<(u32, CatalogEntries), DiscoveryError> {
        let entries = page_bounds(10, cursor)?
            .map(|id| RarityDefinition {
                id: id as u8,
                name: Availability::Unavailable {
                    reason: "The localized rarity-name formatter has not been validated".into(),
                },
            })
            .collect();
        Ok((10, CatalogEntries::Rarities(entries)))
    }
}
