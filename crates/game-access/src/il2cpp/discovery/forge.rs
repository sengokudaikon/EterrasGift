//! Main-thread copies of the current forge graph. No managed/game method is invoked.

use super::{Class, DiscoveryError, Images, Ptr, RuntimeAccess, unsupported};
use crate::{
    forge::{AffixSnapshot, ForgeSnapshot, ItemStatus, UniqueIdentity},
    il2cpp::{AttachedThread, StrongRoot, raw},
};
use std::{ffi::CStr, ptr::NonNull};

pub(super) struct ForgeGraph<'a, 'r> {
    pub(super) images: Images,
    pub(super) manager: Root<'a, 'r>,
    pub(super) item: Root<'a, 'r>,
    pub(super) online_field: Ptr,
    pub(super) manager_field: Ptr,
    pub(super) cached_ptr_field: Ptr,
    pub(super) links: Vec<(Ptr, Ptr, Ptr)>,
    pub(super) _parents: Vec<Root<'a, 'r>>,
}

pub(super) type Root<'a, 'r> = StrongRoot<'a, 'r>;

// Private and closed: each destination type has one exact public runtime primitive class.
pub(super) trait Primitive: Copy + Default {
    const NAME: &'static CStr;
}
impl Primitive for u8 {
    const NAME: &'static CStr = c"Byte";
}
impl Primitive for u16 {
    const NAME: &'static CStr = c"UInt16";
}
impl Primitive for u32 {
    const NAME: &'static CStr = c"UInt32";
}
impl Primitive for i32 {
    const NAME: &'static CStr = c"Int32";
}
impl Primitive for f32 {
    const NAME: &'static CStr = c"Single";
}
impl Primitive for usize {
    const NAME: &'static CStr = c"IntPtr";
}

pub(super) fn target(root: &Root<'_, '_>) -> Result<NonNull<raw::Object>, DiscoveryError> {
    root.target().ok_or_else(|| unsupported("forge root target"))
}

fn bounded_count(count: i32, capacity: u32) -> Result<usize, DiscoveryError> {
    if !(0..=32).contains(&count) || capacity > 4096 || count as u32 > capacity {
        Err(unsupported("forge affix count/capacity"))
    } else {
        Ok(count as usize)
    }
}

fn decode_text(units: &[u16]) -> Result<String, DiscoveryError> {
    if units.len() > 4096 {
        return Err(unsupported("forge text length"));
    }
    let text = String::from_utf16(units).map_err(|_| unsupported("forge text UTF-16"))?;
    if text.len() > 4096 {
        return Err(unsupported("forge text UTF-8 length"));
    }
    Ok(text)
}

impl RuntimeAccess {
    pub(in crate::il2cpp) unsafe fn reference_field(
        &self,
        attached: &AttachedThread<'_>,
        object: &Root<'_, '_>,
        name: &CStr,
        value: &Root<'_, '_>,
    ) -> Result<NonNull<raw::FieldInfo>, DiscoveryError> {
        unsafe {
            if !object.belongs_to(attached) || !value.belongs_to(attached) {
                return Err(unsupported("reference store runtime mismatch"));
            }
            let actual = (self.object_class)(target(value)?.as_ptr());
            if actual.is_null() || (self.class_value)(actual) {
                return Err(unsupported("reference store cannot use a boxed value"));
            }
            let field = self.instance_field(object, name, actual)?;
            let ty = (self.field_type)(field);
            if (self.field_flags)(field) & 0x70 != 0 || !matches!((self.type_code)(ty), 0x0e | 0x12 | 0x14 | 0x15 | 0x1c | 0x1d) {
                return Err(unsupported("reference store requires a mutable reference field"));
            }
            let owner = (self.field_parent)(field);
            let receiver = (self.object_class)(target(object)?.as_ptr());
            if owner.is_null() || receiver.is_null() || !(self.assignable)(owner, receiver) {
                return Err(unsupported("reference store declaring class"));
            }
            NonNull::new(field.cast()).ok_or_else(|| unsupported("reference field absent"))
        }
    }
    // All internal unsafe helpers inherit read_forge's live attached-runtime and graph access contract.
    pub(super) unsafe fn system_class(&self, name: &CStr) -> Result<Class, DiscoveryError> {
        unsafe {
            let image = (self.corlib)();
            if image.is_null() {
                return Err(unsupported("forge corlib"));
            }
            let class = (self.class_from_name)(image, c"System".as_ptr(), name.as_ptr());
            if class.is_null() {
                return Err(unsupported(format!("forge System.{}", name.to_string_lossy())));
            }
            Ok(class)
        }
    }

    pub(super) unsafe fn instance_field(&self, object: &Root<'_, '_>, name: &CStr, expected: Class) -> Result<Ptr, DiscoveryError> {
        unsafe {
            let object = target(object)?;
            let class = (self.object_class)(object.as_ptr());
            if class.is_null() {
                return Err(unsupported("forge object class"));
            }
            let field = self.field(class, name)?;
            let ty = (self.field_type)(field);
            if ty.is_null() || (self.type_byref)(ty) || (self.field_flags)(field) & 0x10 != 0 || (self.type_class)(ty) != expected {
                return Err(unsupported(format!("forge field {} type/storage", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    pub(super) unsafe fn field_class(&self, field: Ptr) -> Result<Class, DiscoveryError> {
        unsafe {
            let ty = (self.field_type)(field);
            if ty.is_null() || (self.type_byref)(ty) {
                return Err(unsupported("forge field type"));
            }
            let class = (self.type_class)(ty);
            if class.is_null() {
                return Err(unsupported("forge field class"));
            }
            Ok(class)
        }
    }

    pub(super) unsafe fn primitive<T: Primitive>(&self, object: &Root<'_, '_>, name: &CStr) -> Result<T, DiscoveryError> {
        unsafe {
            let class = self.system_class(T::NAME)?;
            let field = self.instance_field(object, name, class)?;
            let mut value = T::default();
            // Exact primitive class identity determines the destination's width; user-supplied
            // metadata names cannot select this type or cause a wider write into its stack slot.
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut T).cast());
            Ok(value)
        }
    }

    pub(super) unsafe fn boolean(&self, object: &Root<'_, '_>, name: &CStr) -> Result<bool, DiscoveryError> {
        unsafe {
            let class = self.system_class(c"Boolean")?;
            let field = self.instance_field(object, name, class)?;
            let mut value = 0_u8;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut u8).cast());
            match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(unsupported("forge Boolean representation")),
            }
        }
    }

    pub(super) unsafe fn enumeration(&self, object: &Root<'_, '_>, name: &CStr, enum_name: &CStr) -> Result<i32, DiscoveryError> {
        unsafe {
            let object_class = (self.object_class)(target(object)?.as_ptr());
            if object_class.is_null() {
                return Err(unsupported("forge enum object class"));
            }
            let field = self.field(object_class, name)?;
            let class = self.field_class(field)?;
            let actual_name = (self.class_name)(class);
            // The field itself identifies its enum, including a nested or namespaced enum that
            // cannot be resolved by class_from_name(image, "", simpleName). Recheck both the
            // accepted semantic name and the enum storage before copying into an i32 slot.
            if (self.field_flags)(field) & 0x10 != 0
                || actual_name.is_null()
                || CStr::from_ptr(actual_name) != enum_name
                || !super::field_type_matches(&self.type_name((self.field_type)(field))?, &enum_name.to_string_lossy(), false)
            {
                return Err(unsupported("forge enum field name/storage"));
            }
            let base = self.system_class(c"Int32")?;
            if !(self.class_enum)(class) {
                return Err(unsupported("forge enum storage"));
            }
            let enum_base = (self.enum_base)(class);
            if enum_base.is_null() || (self.type_class)(enum_base) != base {
                return Err(unsupported("forge enum storage"));
            }
            let mut value = 0_i32;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut i32).cast());
            Ok(value)
        }
    }

    pub(super) unsafe fn reference<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        object: &Root<'_, '_>,
        name: &CStr,
        declared: Class,
    ) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
        unsafe {
            if declared.is_null() || (self.class_value)(declared) {
                return Err(unsupported("forge reference field class"));
            }
            let field = self.instance_field(object, name, declared)?;
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut *mut raw::Object).cast());
            // The parent stays rooted, its field stays unchanged, and the child is rooted before
            // any further metadata access or allocation. Graph ownership preserves it during rooting.
            let root = NonNull::new(value).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = &root {
                self.check_object(declared, target(root)?)?;
            }
            Ok(root)
        }
    }

    pub(super) unsafe fn exact(&self, object: &Root<'_, '_>, expected: Class) -> Result<(), DiscoveryError> {
        unsafe {
            if (self.object_class)(target(object)?.as_ptr()) != expected {
                return Err(unsupported("unexpected forge object type"));
            }
            Ok(())
        }
    }

    pub(super) unsafe fn text(&self, attached: &AttachedThread<'_>, object: &Root<'_, '_>, name: &CStr) -> Result<String, DiscoveryError> {
        unsafe {
            let class = self.system_class(c"String")?;
            let Some(text) = self.reference(attached, object, name, class)? else {
                return Ok(String::new());
            };
            self.exact(&text, class)?;
            let text = target(&text)?;
            let length = (self.string_length)(text.as_ptr());
            if !(0..=4096).contains(&length) {
                return Err(unsupported("forge text length"));
            }
            let chars = (self.string_chars)(text.as_ptr());
            if chars.is_null() && length != 0 {
                return Err(unsupported("forge string characters"));
            }
            let mut units = Vec::with_capacity(length as usize);
            for index in 0..length as usize {
                units.push(chars.add(index).read());
            }
            decode_text(&units)
        }
    }

    pub(super) unsafe fn array_class(&self, class: Class, element: Class) -> Result<(), DiscoveryError> {
        unsafe {
            if class.is_null() || (self.class_rank)(class) != 1 || (self.type_code)((self.class_type)(class)) != 0x1d || (self.class_element)(class) != element {
                return Err(unsupported("forge SZARRAY element/rank"));
            }
            Ok(())
        }
    }

    pub(super) unsafe fn array_count(&self, array: &Root<'_, '_>, element: Class, maximum: u32) -> Result<u32, DiscoveryError> {
        unsafe {
            let object = target(array)?;
            self.array_class((self.object_class)(object.as_ptr()), element)?;
            if (self.array_header_size)() != 32 || (self.array_length_offset)() != 24 {
                return Err(unsupported("forge array header"));
            }
            let count = (self.array_length)(object.as_ptr());
            if count > maximum {
                return Err(unsupported("forge array length"));
            }
            Ok(count)
        }
    }

    unsafe fn roll_array(&self, attached: &AttachedThread<'_>, object: &Root<'_, '_>, name: &CStr, maximum: u32) -> Result<Vec<u8>, DiscoveryError> {
        unsafe {
            let byte = self.system_class(c"Byte")?;
            let class = (self.object_class)(target(object)?.as_ptr());
            let field = self.field(class, name)?;
            let array_class = self.field_class(field)?;
            self.array_class(array_class, byte)?;
            let Some(array) = self.reference(attached, object, name, array_class)? else {
                return Ok(Vec::new());
            };
            self.exact(&array, array_class)?;
            let count = self.array_count(&array, byte, maximum)?;
            let start = target(&array)?.as_ptr().cast::<u8>().add(32);
            let mut values = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                values.push(start.add(index).read());
            }
            Ok(values)
        }
    }

    unsafe fn affixes(&self, attached: &AttachedThread<'_>, images: &Images, item: &Root<'_, '_>) -> Result<Vec<AffixSnapshot>, DiscoveryError> {
        unsafe {
            let affix_class = self.class(images, c"", c"ItemAffix")?;
            let item_class = (self.object_class)(target(item)?.as_ptr());
            let list_field = self.field(item_class, c"affixes")?;
            let list_class = self.field_class(list_field)?;
            if list_class.is_null()
                || (self.class_value)(list_class)
                || (self.type_code)((self.class_type)(list_class)) != 0x15
                || (self.class_image)(list_class) != (self.corlib)()
            {
                return Err(unsupported("forge closed List<ItemAffix> class"));
            }
            let list_name = (self.class_name)(list_class);
            let list_namespace = (self.class_namespace)(list_class);
            if list_name.is_null() || list_namespace.is_null() || CStr::from_ptr(list_name) != c"List`1" || CStr::from_ptr(list_namespace) != c"System.Collections.Generic" {
                return Err(unsupported("forge list definition"));
            }
            let list = self
                .reference(attached, item, c"affixes", list_class)?
                .ok_or_else(|| unsupported("forge affix list missing"))?;
            self.exact(&list, list_class)?;
            let count: i32 = self.primitive(&list, c"_size")?;
            let array_field = self.field(list_class, c"_items")?;
            let array_class = self.field_class(array_field)?;
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
                let pointer = array.as_ptr().cast::<u8>().add(32 + slot * std::mem::size_of::<Ptr>()).cast::<*mut raw::Object>().read();
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
    pub unsafe fn read_forge(&self, attached: &AttachedThread<'_>) -> Result<Option<ForgeSnapshot>, DiscoveryError> {
        unsafe {
            let Some(graph) = self.forge_graph(attached)? else { return Ok(None) };
            self.forge_snapshot(attached, &graph).map(Some)
        }
    }

    // Internal callers inherit read_forge's complete live-runtime/graph contract.
    pub(super) unsafe fn forge_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>) -> Result<Option<ForgeGraph<'a, 'r>>, DiscoveryError> {
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
                return Err(DiscoveryError::OfflineRequired);
            }
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let manager_field = self.static_field(self.class(&images, c"", c"PlayerFinder")?, c"localItemContainersManager", "ItemContainersManager")?;
            if (self.type_class)((self.field_type)(manager_field)) != manager_class {
                return Err(unsupported("forge manager static class"));
            }
            let mut manager = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(manager_field, (&mut manager as *mut *mut raw::Object).cast());
            let manager = NonNull::new(manager).ok_or(DiscoveryError::OfflineRequired)?;
            let manager = attached.root(manager)?;
            self.check_object(manager_class, target(&manager)?)?;
            let cached_ptr_field = self.instance_field(&manager, c"m_CachedPtr", self.system_class(c"IntPtr")?)?;
            if self.primitive::<usize>(&manager, c"m_CachedPtr")? == 0 {
                return Err(DiscoveryError::OfflineRequired);
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

    pub(super) unsafe fn forge_snapshot(&self, attached: &AttachedThread<'_>, graph: &ForgeGraph<'_, '_>) -> Result<ForgeSnapshot, DiscoveryError> {
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
    use crate::il2cpp::ThreadContext;
    use std::{cell::Cell, ffi::c_void};

    const PARENT: usize = 0x1000;
    const CHILD: usize = 0x2000;
    const PARENT_CLASS: usize = 0x3000;
    const CHILD_CLASS: usize = 0x4000;
    const BYTE_CLASS: usize = 0x5000;
    const WORD_CLASS: usize = 0x6000;
    const ENUM_CLASS: usize = 0xa000;
    const INT_CLASS: usize = 0xb000;
    const ENUM_BASE_TYPE: usize = 0xd000;
    thread_local! {
        static ROOTS: Cell<usize> = const { Cell::new(0) };
        static FREED: Cell<usize> = const { Cell::new(0) };
        static FIELD_CLASS: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static FIELD_READS: Cell<usize> = const { Cell::new(0) };
        static ASSIGNABLE: Cell<bool> = const { Cell::new(true) };
        static FAIL_CHILD_ROOT: Cell<bool> = const { Cell::new(false) };
        static RANK: Cell<i32> = const { Cell::new(1) };
        static TYPE_CODE: Cell<i32> = const { Cell::new(0x1d) };
        static ELEMENT: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static ENUM_NAME: Cell<&'static CStr> = const { Cell::new(c"ClassRequirement") };
        static ENUM_TYPE_NAME: Cell<&'static CStr> = const { Cell::new(c"Fixture.Owner/ClassRequirement") };
        static IS_ENUM: Cell<bool> = const { Cell::new(true) };
        static ENUM_BASE_CLASS: Cell<usize> = const { Cell::new(INT_CLASS) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static GLOBAL_ENUM_LOOKUPS: Cell<usize> = const { Cell::new(0) };
        static VALUE_CLASS: Cell<bool> = const { Cell::new(false) };
        static STORES: Cell<usize> = const { Cell::new(0) };
    }
    fn token<T>(address: usize) -> *mut T {
        std::ptr::without_provenance_mut(address)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(0x10)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(0x20)
    }
    unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
        panic!("borrowed engine thread must not attach")
    }
    unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
        panic!("borrowed engine thread must not detach")
    }
    unsafe extern "C-unwind" fn root_new(object: *mut raw::Object, pinned: bool) -> *mut c_void {
        assert!(!pinned);
        if object as usize == CHILD && FAIL_CHILD_ROOT.get() {
            return std::ptr::null_mut();
        }
        ROOTS.set(ROOTS.get() + 1);
        Box::into_raw(Box::new(object)).cast()
    }
    unsafe extern "C-unwind" fn root_target(handle: *mut c_void) -> *mut raw::Object {
        // SAFETY: each handle is a live Box allocated by root_new until its one root_free.
        unsafe { handle.cast::<*mut raw::Object>().read() }
    }
    unsafe extern "C-unwind" fn root_free(handle: *mut c_void) {
        // SAFETY: StrongRoot releases each handle exactly once, after its last target access.
        unsafe {
            drop(Box::from_raw(handle.cast::<*mut raw::Object>()));
        }
        ROOTS.set(ROOTS.get() - 1);
        FREED.set(FREED.get() + 1);
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        token(0x7000)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, namespace: *const std::ffi::c_char, name: *const std::ffi::c_char) -> Class {
        // SAFETY: production calls supply static null-terminated metadata names.
        unsafe {
            match (CStr::from_ptr(namespace), CStr::from_ptr(name)) {
                (namespace, name) if namespace == c"System" && name == c"Byte" => token(BYTE_CLASS),
                (namespace, name) if namespace == c"System" && name == c"Int32" => token(INT_CLASS),
                (namespace, name) if namespace == c"" && name == c"ClassRequirement" => {
                    GLOBAL_ENUM_LOOKUPS.set(GLOBAL_ENUM_LOOKUPS.get() + 1);
                    std::ptr::null_mut()
                }
                _ => panic!("unexpected fake class lookup"),
            }
        }
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        match object as usize {
            PARENT => token(PARENT_CLASS),
            CHILD => {
                // Any child metadata query must happen after its strong root was acquired.
                assert_eq!(ROOTS.get(), 2);
                token(CHILD_CLASS)
            }
            _ => panic!("unexpected fake object"),
        }
    }
    unsafe extern "C-unwind" fn class_value(_: *const raw::Class) -> bool {
        VALUE_CLASS.get()
    }
    unsafe extern "C-unwind" fn class_field(class: Class, _: *const std::ffi::c_char) -> Ptr {
        assert_eq!(class as usize, PARENT_CLASS);
        token(0x8000)
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> super::super::Type {
        token(0x9000)
    }
    unsafe extern "C-unwind" fn type_byref(_: super::super::Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn field_flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn type_class(ty: super::super::Type) -> Class {
        token(if ty as usize == ENUM_BASE_TYPE { ENUM_BASE_CLASS.get() } else { FIELD_CLASS.get() })
    }
    unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, _: Ptr, destination: Ptr) {
        assert_eq!(object as usize, PARENT);
        FIELD_READS.set(FIELD_READS.get() + 1);
        match FIELD_CLASS.get() {
            CHILD_CLASS => {
                // SAFETY: the checked reference reader supplied one writable pointer-sized slot.
                unsafe {
                    destination.cast::<*mut raw::Object>().write(token(CHILD));
                }
            }
            ENUM_CLASS => {
                // SAFETY: the enum reader verified Int32 backing before supplying an i32 slot.
                unsafe {
                    destination.cast::<i32>().write(-123_456_789);
                }
            }
            _ => panic!("field_get called before validating destination width"),
        }
    }
    unsafe extern "C-unwind" fn assignable(expected: Class, actual: Class) -> bool {
        assert_eq!(expected, actual);
        assert!(matches!(expected as usize, CHILD_CLASS | PARENT_CLASS));
        ASSIGNABLE.get()
    }
    unsafe extern "C-unwind" fn field_parent(_: Ptr) -> Class {
        token(PARENT_CLASS)
    }
    unsafe extern "C-unwind" fn store_reference(object: *mut raw::Object, field: *mut raw::FieldInfo, value: Ptr) {
        assert_eq!(object as usize, PARENT);
        assert_eq!(field as usize, 0x8000);
        assert_eq!(value as usize, CHILD, "reference ABI requires the object, not a pointer slot");
        assert_eq!(ROOTS.get(), 2);
        STORES.set(STORES.get() + 1);
    }
    unsafe extern "C-unwind" fn rank(_: *const raw::Class) -> i32 {
        RANK.get()
    }
    unsafe extern "C-unwind" fn class_type(_: Class) -> super::super::Type {
        token(0x9000)
    }
    unsafe extern "C-unwind" fn type_code(_: super::super::Type) -> i32 {
        TYPE_CODE.get()
    }
    unsafe extern "C-unwind" fn class_element(_: Class) -> Class {
        token(ELEMENT.get())
    }
    unsafe extern "C-unwind" fn enum_name(class: Class) -> *const std::ffi::c_char {
        assert_eq!(class as usize, ENUM_CLASS);
        ENUM_NAME.get().as_ptr()
    }
    unsafe extern "C-unwind" fn is_enum(class: *const raw::Class) -> bool {
        assert_eq!(class as usize, ENUM_CLASS);
        IS_ENUM.get()
    }
    unsafe extern "C-unwind" fn enum_base(class: Class) -> super::super::Type {
        assert_eq!(class as usize, ENUM_CLASS);
        token(ENUM_BASE_TYPE)
    }
    unsafe extern "C-unwind" fn type_name(_: super::super::Type) -> *mut std::ffi::c_char {
        ENUM_TYPE_NAME.get().to_owned().into_raw()
    }
    unsafe extern "C-unwind" fn runtime_free(allocation: Ptr) {
        // SAFETY: the runtime-owned C string came from this fake type_name's CString::into_raw.
        unsafe {
            drop(std::ffi::CString::from_raw(allocation.cast()));
        }
    }

    fn fixture() -> (RuntimeAccess, ThreadContext) {
        assert_eq!(ROOTS.get(), 0);
        FREED.set(0);
        FIELD_CLASS.set(CHILD_CLASS);
        FIELD_READS.set(0);
        ASSIGNABLE.set(true);
        FAIL_CHILD_ROOT.set(false);
        RANK.set(1);
        TYPE_CODE.set(0x1d);
        ELEMENT.set(CHILD_CLASS);
        ENUM_NAME.set(c"ClassRequirement");
        ENUM_TYPE_NAME.set(c"Fixture.Owner/ClassRequirement");
        IS_ENUM.set(true);
        ENUM_BASE_CLASS.set(INT_CLASS);
        FLAGS.set(0);
        BYREF.set(false);
        GLOBAL_ENUM_LOOKUPS.set(0);
        VALUE_CLASS.set(false);
        STORES.set(0);
        let mut access = RuntimeAccess::test_stubs();
        access.corlib = corlib;
        access.class_from_name = class_from_name;
        access.object_class = object_class;
        access.class_value = class_value;
        access.class_field = class_field;
        access.field_type = field_type;
        access.type_byref = type_byref;
        access.field_flags = field_flags;
        access.field_parent = field_parent;
        access.type_class = type_class;
        access.field_get = field_get;
        access.assignable = assignable;
        access.class_rank = rank;
        access.class_type = class_type;
        access.type_code = type_code;
        access.class_element = class_element;
        access.class_name = enum_name;
        access.class_enum = is_enum;
        access.enum_base = enum_base;
        access.type_name = type_name;
        access.runtime_free = runtime_free;
        let api = raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: attach,
            thread_detach: detach,
            gchandle_new: root_new,
            gchandle_free: root_free,
            gchandle_get_target: root_target,
        };
        // SAFETY: these authored exports implement this test thread's borrowed attachment and
        // Box-backed handles. Their function code outlives all local roots and the context.
        (access, unsafe { ThreadContext::new(api) })
    }

    #[test]
    fn reference_store_checks_types_mutability_owner_and_root_origin_before_calling_export() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored object identities and Box-backed GC handles; no game memory.
        unsafe {
            let parent = attached.root(NonNull::new(token(PARENT)).unwrap()).unwrap();
            let child = attached.root(NonNull::new(token(CHILD)).unwrap()).unwrap();
            let writes = raw::FieldWriteApi::test_export(store_reference);
            for flags in [0x10, 0x20, 0x40] {
                FLAGS.set(flags);
                assert!(writes.set_reference(&access, &attached, &parent, c"child", &child).is_err());
            }
            FLAGS.set(0);
            BYREF.set(true);
            assert!(writes.set_reference(&access, &attached, &parent, c"child", &child).is_err());
            BYREF.set(false);
            VALUE_CLASS.set(true);
            TYPE_CODE.set(0x11);
            assert!(writes.set_reference(&access, &attached, &parent, c"child", &child).is_err());
            VALUE_CLASS.set(false);
            assert!(writes.set_reference(&access, &attached, &parent, c"child", &child).is_err());
            TYPE_CODE.set(0x1d);
            FIELD_CLASS.set(WORD_CLASS);
            assert!(writes.set_reference(&access, &attached, &parent, c"child", &child).is_err());
            FIELD_CLASS.set(CHILD_CLASS);
            ASSIGNABLE.set(false);
            assert!(writes.set_reference(&access, &attached, &parent, c"child", &child).is_err());
            ASSIGNABLE.set(true);
            let other_runtime = {
                // A distinct context using the same fake exports is still a different
                // provenance boundary. Construct without resetting the active roots.
                let api = raw::Api {
                    domain_get: domain,
                    thread_current: current,
                    thread_attach: attach,
                    thread_detach: detach,
                    gchandle_new: root_new,
                    gchandle_free: root_free,
                    gchandle_get_target: root_target,
                };
                ThreadContext::new(api)
            };
            let other_attached = other_runtime.attach().unwrap();
            assert!(writes.set_reference(&access, &other_attached, &parent, c"child", &child).is_err());
            assert_eq!(STORES.get(), 0);
            writes.set_reference(&access, &attached, &parent, c"child", &child).unwrap();
            assert_eq!(STORES.get(), 1);
        }
        assert_eq!(ROOTS.get(), 0);
    }

    #[test]
    fn reference_roots_child_before_metadata_and_releases_every_handle() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: fake objects are immutable identity tokens recognized by the authored exports.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        // SAFETY: all fake runtime references belong to the same attached test thread; no graph races.
        let child = unsafe { access.reference(&attached, &parent, c"child", token(CHILD_CLASS)) }.unwrap().unwrap();
        assert_eq!(ROOTS.get(), 2);
        assert_eq!(FIELD_READS.get(), 1);
        drop(child);
        assert_eq!(ROOTS.get(), 1);
        assert_eq!(FREED.get(), 1);
        drop(parent);
        assert_eq!(ROOTS.get(), 0);
        assert_eq!(FREED.get(), 2);
    }
    #[test]
    fn reference_validation_error_releases_new_child_root() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored immutable fake objects and valid test-runtime handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        ASSIGNABLE.set(false);
        // SAFETY: the fake API returns a rooted object but deliberately rejects its class contract.
        assert!(unsafe { access.reference(&attached, &parent, c"child", token(CHILD_CLASS)) }.is_err());
        assert_eq!(ROOTS.get(), 1);
        assert_eq!(FREED.get(), 1);
        drop(parent);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn failed_child_root_does_not_free_or_leak_the_parent() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored immutable fake objects and valid test-runtime handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        FAIL_CHILD_ROOT.set(true);
        // SAFETY: the fake root export deliberately returns failure, before child metadata access.
        assert!(matches!(
            unsafe { access.reference(&attached, &parent, c"child", token(CHILD_CLASS)) },
            Err(DiscoveryError::Runtime(crate::il2cpp::RuntimeError::RootFailed))
        ));
        assert_eq!(ROOTS.get(), 1);
        assert_eq!(FREED.get(), 0);
        drop(parent);
        assert_eq!(ROOTS.get(), 0);
        assert_eq!(FREED.get(), 1);
    }
    #[test]
    fn wider_scalar_metadata_is_rejected_before_writing_destination() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored immutable fake objects and valid test-runtime handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        FIELD_CLASS.set(WORD_CLASS);
        // SAFETY: metadata deliberately reports UInt16 where the reader requires exact Byte.
        assert!(unsafe { access.primitive::<u8>(&parent, c"value") }.is_err());
        assert_eq!(FIELD_READS.get(), 0);
        drop(parent);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn enum_field_resolves_nested_or_namespaced_type_without_a_global_class() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: the authored runtime exposes this immutable fake object to valid handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        FIELD_CLASS.set(ENUM_CLASS);
        // SAFETY: both arguments are valid static null-terminated strings for this fake runtime.
        assert!(unsafe { (access.class_from_name)(token(0x7000), c"".as_ptr(), c"ClassRequirement".as_ptr()) }.is_null());
        GLOBAL_ENUM_LOOKUPS.set(0);
        for name in [c"Fixture.Owner/ClassRequirement", c"Fixture.Owner+ClassRequirement", c"Fixture.Namespace.ClassRequirement"] {
            ENUM_TYPE_NAME.set(name);
            // SAFETY: the field carries a valid Int32 enum, even though no global class exists.
            assert_eq!(unsafe { access.enumeration(&parent, c"classReq", c"ClassRequirement") }.unwrap(), -123_456_789);
        }
        assert_eq!(FIELD_READS.get(), 3);
        assert_eq!(GLOBAL_ENUM_LOOKUPS.get(), 0);
        drop(parent);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn enum_name_kind_base_and_storage_are_checked_before_destination_write() {
        for invalid in 0..6 {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            // SAFETY: authored immutable fake object and valid test-runtime handle APIs.
            let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
            FIELD_CLASS.set(ENUM_CLASS);
            match invalid {
                0 => ENUM_NAME.set(c"OtherEnum"),
                1 => IS_ENUM.set(false),
                2 => ENUM_BASE_CLASS.set(WORD_CLASS),
                3 => ENUM_TYPE_NAME.set(c"Fixture.NotClassRequirement"),
                4 => FLAGS.set(0x10),
                5 => BYREF.set(true),
                _ => unreachable!(),
            }
            // SAFETY: fake metadata deliberately violates one contract. None authorizes an i32 write.
            assert!(unsafe { access.enumeration(&parent, c"classReq", c"ClassRequirement") }.is_err());
            assert_eq!(FIELD_READS.get(), 0);
            assert_eq!(GLOBAL_ENUM_LOOKUPS.get(), 0);
            drop(parent);
            assert_eq!(ROOTS.get(), 0);
        }
    }
    #[test]
    fn array_metadata_rejects_wrong_rank_array_kind_and_element() {
        let (access, _context) = fixture();
        // SAFETY: authored metadata exports accept these class identity tokens; no array data is read.
        unsafe {
            access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).unwrap();
            RANK.set(2);
            assert!(access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).is_err());
            RANK.set(1);
            TYPE_CODE.set(0x14);
            assert!(access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).is_err());
            TYPE_CODE.set(0x1d);
            ELEMENT.set(BYTE_CLASS);
            assert!(access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).is_err());
        }
    }
    #[test]
    fn rejects_bad_affix_count_and_storage_bounds() {
        assert!(bounded_count(-1, 8).is_err());
        assert!(bounded_count(33, 64).is_err());
        assert!(bounded_count(2, 1).is_err());
        assert!(bounded_count(1, 4097).is_err());
        assert_eq!(bounded_count(32, 64).unwrap(), 32);
        assert_eq!(bounded_count(0, 0).unwrap(), 0);
    }
    #[test]
    fn strict_text_checks_both_encodings() {
        assert_eq!(decode_text(&[0x41, 0xd83d, 0xde00]).unwrap(), "A😀");
        assert!(decode_text(&[0xd800]).is_err());
        assert!(decode_text(&[0xdc00]).is_err());
        assert!(decode_text(&vec![0x41; 4097]).is_err());
        assert!(decode_text(&vec![0x4e00; 1366]).is_err());
        assert!(decode_text(&vec![0x41; 4096]).is_ok());
    }
}
