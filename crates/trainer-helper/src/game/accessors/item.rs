//! The held item: the forge's crafting slot, reached from the player's item manager, copied into owned values.
//! No game method is called.

use crate::game::{GameError, Images, Ptr, Root, RuntimeAccess, copy_values, read_value, target, unsupported};
use eg_trainer_wire::forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity};
use il2cpp_runtime::{AttachedThread, raw};
use std::{ffi::CStr, ptr::NonNull};

pub(crate) struct ForgeGraph<'a, 'r> {
    pub(crate) images: Images,
    pub(crate) manager: Root<'a, 'r>,
    pub(crate) item: Root<'a, 'r>,
    pub(crate) online_field: Ptr,
    pub(crate) manager_field: Ptr,
    pub(crate) cached_ptr_field: Ptr,
    pub(crate) links: Vec<(Ptr, Ptr, Ptr)>,
    pub(crate) _parents: Vec<Root<'a, 'r>>,
}

fn bounded_count(count: i32, capacity: u32) -> Result<usize, GameError> {
    if !(0..=32).contains(&count) || capacity > 4096 || count as u32 > capacity {
        Err(unsupported("affix count/capacity"))
    } else {
        Ok(count as usize)
    }
}

impl RuntimeAccess {
    unsafe fn roll_array(&self, attached: &AttachedThread<'_>, object: &Root<'_, '_>, name: &CStr, maximum: u32) -> Result<Vec<u8>, GameError> {
        // SAFETY: The rooted array has exact Byte element storage and bounded length before its owned byte copy.
        unsafe {
            let byte = self.system_class(c"Byte")?;
            let class = (self.object_class)(target(object)?.as_ptr());
            let field = self.field(class, name)?;
            let array_class = self.field_class(field, "item")?;
            self.array_class(array_class, byte)?;
            let Some(array) = self.reference(attached, object, name, array_class)? else {
                return Ok(Vec::new());
            };
            self.exact(&array, array_class)?;
            let count = self.array_count(&array, byte, maximum)?;
            let start = target(&array)?.as_ptr().cast::<u8>().add(32);
            Ok(copy_values(start, count as usize))
        }
    }

    unsafe fn affixes(&self, attached: &AttachedThread<'_>, images: &Images, item: &Root<'_, '_>) -> Result<Vec<AffixSnapshot>, GameError> {
        // SAFETY: The rooted list and backing array are bounded; each affix is rooted and type-checked before field copying.
        unsafe {
            let affix_class = self.class(images, c"", c"ItemAffix")?;
            let item_class = (self.object_class)(target(item)?.as_ptr());
            let list_field = self.field(item_class, c"affixes")?;
            let list_class = self.field_class(list_field, "item")?;
            self.list_class(list_class)?;
            let list = self
                .reference(attached, item, c"affixes", list_class)?
                .ok_or_else(|| unsupported("forge affix list missing"))?;
            self.exact(&list, list_class)?;
            let count: i32 = self.primitive(&list, c"_size")?;
            let array_field = self.field(list_class, c"_items")?;
            let array_class = self.field_class(array_field, "item")?;
            self.array_class(array_class, affix_class)?;
            let array = self
                .reference(attached, &list, c"_items", array_class)?
                .ok_or_else(|| unsupported("forge affix storage missing"))?;
            self.exact(&array, array_class)?;
            let capacity = self.array_count(&array, affix_class, 4096)?;
            let count = bounded_count(count, capacity)?;
            let mut values = Vec::with_capacity(count);
            for slot in 0..count {
                let array = target(&array)?;
                let pointer = read_value(array.as_ptr().cast::<u8>().add(32 + slot * std::mem::size_of::<Ptr>()).cast::<*mut raw::Object>());
                let pointer = NonNull::new(pointer).ok_or_else(|| unsupported("forge null affix"))?;
                let affix = attached.root(pointer)?;
                self.exact(&affix, affix_class)?;
                values.push(AffixSnapshot {
                    slot: slot as u8,
                    id: self.primitive(&affix, c"affixId")?,
                    tier: u16::from(self.primitive::<u8>(&affix, c"affixTier")?) + 1,
                    roll: self.primitive(&affix, c"affixRoll")?,
                    kind: self.enumeration(&affix, c"affixType", c"AffixType")?,
                    seal: self.enumeration(&affix, c"sealedAffixType", c"SealedAffixType")?,
                    name: self.text(attached, &affix, c"affixName")?,
                    special: self.enumeration(&affix, c"specialAffixType", c"SpecialAffixType")?,
                });
            }
            Ok(values)
        }
    }

    /// Copy the current offline forge graph into owned Rust values. No game method is invoked.
    /// An absent container/entry/item is `None`; online play or a dead/missing player manager is
    /// `OfflineRequired`. Malformed objects, metadata, text or collections fail the whole read.
    ///
    /// # Safety
    /// All runtime/attachment/layout conditions of `discover` and main-thread conditions of
    /// `status` apply. The current thread must have exclusive read access to this graph for the
    /// entire copy: other threads must not mutate its fields, collection slots or strings, replace
    /// its reachable references, or destroy its manager. The runtime uses the nonmoving x64 GC
    /// and array data starts at offset 32, as separately validated. Every acquired reference stays
    /// live through rooting because its rooted parent retains that unchanged strong reference.
    /// No root, raw pointer or borrow returned by this operation grants later memory access; the
    /// snapshot's item address is only an ephemeral identity token. Runtime shutdown is excluded.
    pub unsafe fn read_forge(&self, attached: &AttachedThread<'_>) -> Result<Option<ForgeSnapshot>, GameError> {
        // SAFETY: The caller supplies the documented main-thread graph span; all objects remain rooted through the owned snapshot.
        unsafe {
            let Some(graph) = self.forge_graph(attached)? else { return Ok(None) };
            self.forge_snapshot(attached, &graph).map(Some)
        }
    }

    // Internal callers inherit read_forge's complete live-runtime/graph contract.
    pub(crate) unsafe fn forge_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>) -> Result<Option<ForgeGraph<'a, 'r>>, GameError> {
        // SAFETY: The caller excludes graph changes; static manager and every graph edge are rooted and class-checked before traversal.
        unsafe {
            let images = self.images()?;
            let online = self.static_field(self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?, c"_isOnlinePlay", "System.Boolean")?;
            if (self.type_class)((self.field_type)(online)) != self.system_class(c"Boolean")? {
                return Err(unsupported("forge online flag class"));
            }
            let mut online_value = 1_u8;
            (self.static_get)(online, (&mut online_value as *mut u8).cast());
            if online_value > 1 {
                return Err(unsupported("forge online Boolean"));
            }
            if online_value != 0 {
                return Err(GameError::OfflineRequired);
            }
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let manager_field = self.static_field(self.class(&images, c"", c"PlayerFinder")?, c"localItemContainersManager", "ItemContainersManager")?;
            if (self.type_class)((self.field_type)(manager_field)) != manager_class {
                return Err(unsupported("forge manager static class"));
            }
            let mut manager = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(manager_field, (&mut manager as *mut *mut raw::Object).cast());
            let manager = NonNull::new(manager).ok_or(GameError::OfflineRequired)?;
            let manager = attached.root(manager)?;
            self.check_object(manager_class, target(&manager)?)?;
            let cached_ptr_field = self.instance_field(&manager, c"m_CachedPtr", self.system_class(c"IntPtr")?)?;
            if self.primitive::<usize>(&manager, c"m_CachedPtr")? == 0 {
                return Err(GameError::OfflineRequired);
            }
            // No item-graph reference is read until both runtime offline conditions have passed.
            let crafting_class = self.class(&images, c"", c"CraftingContainers")?;
            let Some(crafting) = self.reference(attached, &manager, c"crafting", crafting_class)? else {
                return Ok(None);
            };
            let main_class = self.class(&images, c"", c"OneItemContainer")?;
            let Some(main) = self.reference(attached, &crafting, c"main", main_class)? else {
                return Ok(None);
            };
            let entry_class = self.class(&images, c"", c"ItemContainerEntry")?;
            let Some(entry) = self.reference(attached, &main, c"content", entry_class)? else {
                return Ok(None);
            };
            let item_class = self.class(&images, c"", c"ItemData")?;
            let Some(item) = self.reference(attached, &entry, c"data", item_class)? else {
                return Ok(None);
            };
            self.exact(&item, self.class(&images, c"", c"ItemDataUnpacked")?)?;
            // Retain the intermediate parents and exact validated reference fields so a writer
            // can recheck every edge without further metadata work or a managed invocation.
            let links = vec![
                (
                    target(&manager)?.as_ptr().cast(),
                    self.instance_field(&manager, c"crafting", crafting_class)?,
                    target(&crafting)?.as_ptr().cast(),
                ),
                (
                    target(&crafting)?.as_ptr().cast(),
                    self.instance_field(&crafting, c"main", main_class)?,
                    target(&main)?.as_ptr().cast(),
                ),
                (
                    target(&main)?.as_ptr().cast(),
                    self.instance_field(&main, c"content", entry_class)?,
                    target(&entry)?.as_ptr().cast(),
                ),
                (
                    target(&entry)?.as_ptr().cast(),
                    self.instance_field(&entry, c"data", item_class)?,
                    target(&item)?.as_ptr().cast(),
                ),
            ];
            Ok(Some(ForgeGraph {
                images,
                manager,
                item,
                online_field: online,
                manager_field,
                cached_ptr_field,
                links,
                _parents: vec![crafting, main, entry],
            }))
        }
    }

    /// Whether `graph` still holds the item a request was made for: the same manager and an unchanged snapshot. The
    /// snapshot is read only when the manager is the same.
    /// # Safety
    /// Same contract as `forge_snapshot`.
    pub(crate) unsafe fn forge_unchanged(&self, attached: &AttachedThread<'_>, graph: &ForgeGraph<'_, '_>, manager: u64, expected: &ForgeSnapshot) -> Result<bool, GameError> {
        // SAFETY: the caller's forge_snapshot contract.
        unsafe { Ok(target(&graph.manager)?.as_ptr() as u64 == manager && self.forge_snapshot(attached, graph)? == *expected) }
    }

    pub(crate) unsafe fn forge_snapshot(&self, attached: &AttachedThread<'_>, graph: &ForgeGraph<'_, '_>) -> Result<ForgeSnapshot, GameError> {
        // SAFETY: Existing graph roots retain the selected item and arrays while exact typed fields are copied.
        unsafe {
            let item = &graph.item;
            let rarity: u8 = self.primitive(item, c"rarity")?;
            let identity = if matches!(rarity, 7..=9) {
                Some(UniqueIdentity {
                    id: self.primitive(item, c"uniqueID")?,
                    rolls: self.roll_array(attached, item, c"uniqueRolls", 8)?,
                })
            } else {
                None
            };
            let mut snapshot = ForgeSnapshot {
                item: target(item)?.as_ptr() as u64,
                individual_id: self.primitive(item, c"individualID")?,
                fingerprint: 0,
                name: self.text(attached, item, c"_fullName")?,
                item_type: self.primitive(item, c"itemType")?,
                sub_type: self.primitive(item, c"subType")?,
                rarity,
                class_requirement: self.enumeration(item, c"classReq", c"ClassRequirement")?,
                affixes: self.affixes(attached, &graph.images, item)?,
                implicit_rolls: self.roll_array(attached, item, c"implicitRolls", 3)?,
                identity,
                status: ItemStatus {
                    forging_potential: self.primitive(item, c"forgingPotential")?,
                    forging_potential_type: self.enumeration(item, c"forgingPotentialType", c"ForgingPotentialType")?,
                    legendary_potential: self.primitive(item, c"legendaryPotential")?,
                    weavers_will: self.primitive(item, c"weaversWill")?,
                    weavers_touch: self.primitive(item, c"weaversTouch")?,
                    corrupted: self.boolean(item, c"corrupted")?,
                    ruined: self.boolean(item, c"ruined")?,
                    level_requirement: self.primitive(item, c"<LvlReq>k__BackingField")?,
                },
            };
            snapshot.fingerprint = snapshot.legacy_fingerprint();
            Ok(snapshot)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_affix_count_and_storage_bounds() {
        assert!(bounded_count(-1, 8).is_err());
        assert!(bounded_count(33, 64).is_err());
        assert!(bounded_count(2, 1).is_err());
        assert!(bounded_count(1, 4097).is_err());
        assert_eq!(bounded_count(32, 64).unwrap(), 32);
        assert_eq!(bounded_count(0, 0).unwrap(), 0);
    }
}
