//! Bounded, rooted catalog graph copies. Singleton getters and formatter bodies are not invoked.

use super::{
    Class, DiscoveryError, Images, RuntimeAccess,
    forge::{Primitive, Root, target},
    unsupported,
};
use crate::{
    catalog::{
        Availability, CatalogEntries, CatalogKind, CatalogPage, DefinitionName, DefinitionNameSource, ImplicitDefinition, ItemTypeDefinition, MAX_IMPLICITS, MAX_PAGE_ENTRIES,
        MAX_SUB_TYPES, SubTypeDefinition,
    },
    il2cpp::{AttachedThread, raw},
};
use std::{ffi::CStr, ptr::NonNull};

fn collection_count(count: i32, capacity: u32, maximum: u32) -> Result<usize, DiscoveryError> {
    // A retained List backing store may be larger than its active population.
    if count < 0 || count as u32 > maximum || count as u32 > capacity || capacity > 65536 {
        return Err(unsupported("catalog collection count/capacity"));
    }
    Ok(count as usize)
}

pub(super) fn page_bounds(total: u32, cursor: u32) -> Result<std::ops::Range<usize>, DiscoveryError> {
    if cursor > total {
        return Err(unsupported("catalog cursor exceeds definitions"));
    }
    Ok(cursor as usize..cursor.saturating_add(MAX_PAGE_ENTRIES as u32).min(total) as usize)
}

const IMPLICIT_TEXT_REASON: &str = "Game implicit formatter has not been audited for this capture";

struct CopyBudget {
    used: usize,
}
impl CopyBudget {
    fn new() -> Self {
        // Covers the page envelope, both maximum-sized locale strings and fixed framing.
        Self { used: 16384 }
    }
    fn charge(&mut self, bytes: usize) -> Result<(), DiscoveryError> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|used| *used <= crate::protocol::MAX_RESPONSE_SIZE)
            .ok_or_else(|| unsupported("catalog base page exceeds conservative 1 MiB copy budget"))?;
        Ok(())
    }
}

fn unavailable_text() -> Availability<String> {
    Availability::Unavailable {
        reason: IMPLICIT_TEXT_REASON.into(),
    }
}

fn subtype_name(display: String, internal: String) -> DefinitionName {
    if display.is_empty() {
        DefinitionName {
            text: internal,
            source: DefinitionNameSource::InternalName,
        }
    } else {
        DefinitionName {
            text: display,
            source: DefinitionNameSource::StoredDisplay,
        }
    }
}

impl RuntimeAccess {
    /// Capture one bounded catalog page from already loaded game definition objects.
    /// Names retain their actual source. Implicit numbers are raw game definitions; formatted
    /// tooltip text remains explicitly unavailable. Singleton getters and game formatters are
    /// never invoked, so absent or destroyed definition storage reports `NotReady`.
    ///
    /// # Safety
    /// The complete `read_game_locale` contract applies, including the exact audited locale and
    /// Application.version framework getter bodies and their permitted initialization/allocation
    /// effects. SP43 capture also authorizes this artifact's audited corlib
    /// System.Array.GetValue(Int32) body: rank/bounds/reflection checks, runtime type caches,
    /// metadata setup, allocation/boxing and GC/profiler callbacks. Dictionary Entry class
    /// initializers are rejected before boxing; no game dictionary getter or cache fill is called.
    /// The current thread must be the Unity main thread after its original dispatcher.
    /// Runtime code and metadata remain initialized/loaded, and every definition graph, collection,
    /// string and Unity native object's lifetime must remain unchanged throughout this copy.
    /// Definition singleton static references must already be populated by the game. No gameplay
    /// state write is authorized. All raw metadata pointers belong to this admitted runtime;
    /// the validated nonmoving x64 array data offset is 32. Returned values own their contents.
    pub unsafe fn read_catalog_page(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, kind: CatalogKind, cursor: u32) -> Result<CatalogPage, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let status = self.status(attached)?;
            if status.online || status.manager == 0 {
                return Err(DiscoveryError::OfflineRequired);
            }
            if cursor > kind.maximum_total() {
                return Err(unsupported("catalog cursor bound"));
            }
            let locale = self.read_game_locale(attached, api)?;
            let images = self.images()?;
            let (total, entries) = match kind {
                CatalogKind::Bases => self.catalog_bases(attached, &images, cursor)?,
                CatalogKind::Affixes => self.catalog_affixes(attached, &images, cursor, api)?,
                CatalogKind::Uniques => self.catalog_uniques(attached, &images, cursor, api)?,
                CatalogKind::Rarities => self.catalog_rarities(cursor)?,
            };
            if total == 0 {
                return Err(DiscoveryError::NotReady("game definitions are empty"));
            }
            if total > kind.maximum_total() {
                return Err(unsupported("catalog total bound"));
            }
            Ok(CatalogPage {
                locale,
                kind,
                cursor,
                total,
                entries,
            })
        }
    }

    unsafe fn catalog_bases(&self, attached: &AttachedThread<'_>, images: &Images, cursor: u32) -> Result<(u32, CatalogEntries), DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let list_class = self.class(images, c"", c"ItemList")?;
            let base_class = self.catalog_nested(list_class, c"BaseEquipmentItem")?;
            let base_parent = self.catalog_nested(list_class, c"BaseItem")?;
            let sub_class = self.catalog_nested(list_class, c"EquipmentItem")?;
            let sub_parent = self.catalog_nested(list_class, c"SubItem")?;
            let implicit_class = self.catalog_nested(list_class, c"EquipmentImplicit")?;
            if (self.class_parent)(base_class) != base_parent || (self.class_parent)(sub_class) != sub_parent {
                return Err(unsupported("catalog equipment class hierarchy"));
            }
            let storage = self
                .catalog_static(
                    attached,
                    self.class(images, c"LE.AssetManagement", c"GlobalAssets")?,
                    c"_storage_MasterItemsList",
                    list_class,
                )?
                .ok_or(DiscoveryError::NotReady("master item definitions are not initialized"))?;
            self.exact(&storage, list_class)?;
            if !self.catalog_live(attached, images, &storage)? {
                return Err(DiscoveryError::NotReady("master item definitions are destroyed"));
            }
            let array = self
                .catalog_reference(attached, &storage, c"EquippableItems")?
                .ok_or(DiscoveryError::NotReady("equipment definitions are not initialized"))?;
            let bases = self.catalog_array(attached, &array, base_class, 256)?.into_iter().flatten().collect::<Vec<_>>();
            let total = bases.len() as u32;
            let mut entries = Vec::new();
            let mut budget = CopyBudget::new();
            for index in page_bounds(total, cursor)? {
                let base = &bases[index];
                self.exact(base, base_class)?;
                let id: i32 = self.primitive(base, c"baseTypeID")?;
                let item_type = u8::try_from(id).map_err(|_| unsupported("catalog base type ID range"))?;
                let name = DefinitionName {
                    text: self.text(attached, base, c"displayName")?,
                    source: DefinitionNameSource::StoredDisplay,
                };
                budget.charge(16 + name.text.len())?;
                let mut sub_types = Vec::new();
                if let Some(list) = self.catalog_reference(attached, base, c"subItems")? {
                    for sub in self.catalog_list(attached, &list, sub_class, MAX_SUB_TYPES as u32)? {
                        self.exact(&sub, sub_class)?;
                        let id: i32 = self.primitive(&sub, c"subTypeID")?;
                        let sub_type = u16::try_from(id).map_err(|_| unsupported("catalog subtype ID range"))?;
                        let display = self.text(attached, &sub, c"displayName")?;
                        let internal = if display.is_empty() { self.text(attached, &sub, c"name")? } else { String::new() };
                        let name = subtype_name(display, internal);
                        budget.charge(32 + name.text.len())?;
                        let mut implicits = Vec::new();
                        if let Some(list) = self.catalog_reference(attached, &sub, c"implicits")? {
                            for implicit in self.catalog_list(attached, &list, implicit_class, MAX_IMPLICITS as u32)? {
                                // Charge before copying or allocating this entry's two reason strings.
                                budget.charge(64 + 2 * IMPLICIT_TEXT_REASON.len())?;
                                self.exact(&implicit, implicit_class)?;
                                let minimum: f32 = self.primitive(&implicit, c"implicitValue")?;
                                let maximum: f32 = self.primitive(&implicit, c"implicitMaxValue")?;
                                if !minimum.is_finite() || !maximum.is_finite() {
                                    return Err(unsupported("catalog nonfinite implicit range"));
                                }
                                implicits.push(ImplicitDefinition {
                                    property: self.catalog_enum_u8(&implicit, c"property", c"SP")?,
                                    tags: self.enumeration(&implicit, c"tags", c"AT")?,
                                    special_tag: self.primitive(&implicit, c"specialTag")?,
                                    extra_tag: self.primitive(&implicit, c"extraTag")?,
                                    modifier_type: self.enumeration(&implicit, c"type", c"ModType")?,
                                    minimum,
                                    maximum,
                                    minimum_text: unavailable_text(),
                                    maximum_text: unavailable_text(),
                                });
                            }
                        }
                        sub_types.push(SubTypeDefinition {
                            sub_type,
                            name,
                            level_requirement: self.primitive(&sub, c"levelRequirement")?,
                            class_requirement: self.enumeration(&sub, c"classRequirement", c"ClassRequirement")?,
                            cannot_drop: self.boolean(&sub, c"cannotDrop")?,
                            is_corrupted: self.boolean(&sub, c"isCorruptedSubtype")?,
                            implicits: Availability::Available(implicits),
                        });
                    }
                }
                entries.push(ItemTypeDefinition { item_type, name, sub_types });
            }
            Ok((total, CatalogEntries::Bases(entries)))
        }
    }

    // Every helper inherits the admitted main-thread, initialized metadata and exclusive graph
    // access contract of its catalog operation. All child references remain in rooted parents
    // until their own roots have been acquired; Unity native object lifetime is checked separately.
    pub(super) unsafe fn catalog_nested(&self, parent: Class, name: &CStr) -> Result<Class, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            if parent.is_null() {
                return Err(unsupported("catalog nested owner"));
            }
            let mut iterator = std::ptr::null_mut();
            let mut found = std::ptr::null_mut();
            for _ in 0..4096 {
                let class = (self.class_nested)(parent, &mut iterator);
                if class.is_null() {
                    return NonNull::new(found)
                        .map(NonNull::as_ptr)
                        .ok_or_else(|| unsupported(format!("catalog nested {}", name.to_string_lossy())));
                }
                let actual = (self.class_name)(class);
                if actual.is_null() {
                    return Err(unsupported("catalog nested name"));
                }
                if CStr::from_ptr(actual) == name {
                    if !found.is_null() && found != class {
                        return Err(unsupported("catalog ambiguous nested class"));
                    }
                    found = class;
                }
            }
            Err(unsupported("catalog nested enumeration limit"))
        }
    }

    pub(super) unsafe fn catalog_static<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        owner: Class,
        name: &CStr,
        expected: Class,
    ) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            if expected.is_null() || (self.class_value)(expected) {
                return Err(unsupported("catalog static reference class"));
            }
            let field = self.field(owner, name)?;
            let flags = (self.field_flags)(field);
            if flags & 0x10 == 0 || flags & 0x40 != 0 || (self.field_offset)(field) == usize::MAX || self.field_class(field)? != expected {
                return Err(unsupported("catalog static reference storage/identity"));
            }
            let mut pointer = std::ptr::null_mut::<raw::Object>();
            // Static value reads do not initialize the owning class or load assets.
            (self.static_get)(field, (&mut pointer as *mut *mut raw::Object).cast());
            let root = NonNull::new(pointer).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = &root {
                self.check_object(expected, target(root)?)?;
            }
            Ok(root)
        }
    }

    pub(super) unsafe fn catalog_reference<'a, 'r>(&self, attached: &'a AttachedThread<'r>, parent: &Root<'_, '_>, name: &CStr) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let owner = (self.object_class)(target(parent)?.as_ptr());
            if owner.is_null() {
                return Err(unsupported("catalog reference owner"));
            }
            let field = self.field(owner, name)?;
            self.reference(attached, parent, name, self.field_class(field)?)
        }
    }

    pub(super) unsafe fn catalog_array<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        array: &Root<'_, '_>,
        element: Class,
        maximum: u32,
    ) -> Result<Vec<Option<Root<'a, 'r>>>, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            if element.is_null() || (self.class_value)(element) || maximum > 65536 {
                return Err(unsupported("catalog reference array element/bound"));
            }
            let count = self.array_count(array, element, maximum)?;
            let mut entries = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                let pointer = target(array)?
                    .as_ptr()
                    .cast::<u8>()
                    .add(32 + index * std::mem::size_of::<usize>())
                    .cast::<*mut raw::Object>()
                    .read();
                let root = NonNull::new(pointer).map(|pointer| attached.root(pointer)).transpose()?;
                if let Some(root) = &root {
                    self.check_object(element, target(root)?)?;
                }
                entries.push(root);
            }
            Ok(entries)
        }
    }

    unsafe fn catalog_list_storage<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        list: &Root<'_, '_>,
        element: Class,
        maximum: u32,
    ) -> Result<(Root<'a, 'r>, usize), DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            if element.is_null() || maximum > 65536 {
                return Err(unsupported("catalog list element/bound"));
            }
            let class = (self.object_class)(target(list)?.as_ptr());
            if class.is_null() || (self.class_value)(class) || (self.class_image)(class) != (self.corlib)() || (self.type_code)((self.class_type)(class)) != 0x15 {
                return Err(unsupported("catalog closed List class"));
            }
            let name = (self.class_name)(class);
            let namespace = (self.class_namespace)(class);
            if name.is_null() || namespace.is_null() || CStr::from_ptr(name) != c"List`1" || CStr::from_ptr(namespace) != c"System.Collections.Generic" {
                return Err(unsupported("catalog List definition"));
            }
            let count: i32 = self.primitive(list, c"_size")?;
            let array_field = self.field(class, c"_items")?;
            let array_class = self.field_class(array_field)?;
            self.array_class(array_class, element)?;
            let array = self
                .reference(attached, list, c"_items", array_class)?
                .ok_or_else(|| unsupported("catalog List storage is missing"))?;
            self.exact(&array, array_class)?;
            let capacity = self.array_count(&array, element, 65536)?;
            Ok((array, collection_count(count, capacity, maximum)?))
        }
    }

    pub(super) unsafe fn catalog_list<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        list: &Root<'_, '_>,
        element: Class,
        maximum: u32,
    ) -> Result<Vec<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            if element.is_null() || (self.class_value)(element) {
                return Err(unsupported("catalog reference List element"));
            }
            let (array, count) = self.catalog_list_storage(attached, list, element, maximum)?;
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                let pointer = target(&array)?
                    .as_ptr()
                    .cast::<u8>()
                    .add(32 + index * std::mem::size_of::<usize>())
                    .cast::<*mut raw::Object>()
                    .read();
                let root = attached.root(NonNull::new(pointer).ok_or_else(|| unsupported("catalog null List entry"))?)?;
                self.check_object(element, target(&root)?)?;
                values.push(root);
            }
            Ok(values)
        }
    }

    pub(super) unsafe fn catalog_values<T: Primitive>(&self, attached: &AttachedThread<'_>, list: &Root<'_, '_>, element: Class, maximum: u32) -> Result<Vec<T>, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let expected = self.system_class(T::NAME)?;
            if element.is_null() {
                return Err(unsupported("catalog value List element"));
            }
            if element != expected {
                if !(self.class_enum)(element) {
                    return Err(unsupported("catalog value List storage"));
                }
                let base = (self.enum_base)(element);
                if base.is_null() || (self.type_byref)(base) || (self.type_class)(base) != expected {
                    return Err(unsupported("catalog enum List storage"));
                }
            }
            let (array, count) = self.catalog_list_storage(attached, list, element, maximum)?;
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(target(&array)?.as_ptr().cast::<u8>().add(32 + index * std::mem::size_of::<T>()).cast::<T>().read());
            }
            Ok(values)
        }
    }

    pub(super) unsafe fn catalog_enum_u8(&self, object: &Root<'_, '_>, name: &CStr, enum_name: &CStr) -> Result<u8, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let owner = (self.object_class)(target(object)?.as_ptr());
            if owner.is_null() {
                return Err(unsupported("catalog enum owner"));
            }
            let field = self.field(owner, name)?;
            let class = self.field_class(field)?;
            let actual = (self.class_name)(class);
            if (self.field_flags)(field) & 0x10 != 0 || actual.is_null() || CStr::from_ptr(actual) != enum_name || !(self.class_enum)(class) {
                return Err(unsupported("catalog Byte enum identity/storage"));
            }
            let base = (self.enum_base)(class);
            if base.is_null() || (self.type_byref)(base) || (self.type_code)(base) != 0x05 || (self.type_class)(base) != self.system_class(c"Byte")? {
                return Err(unsupported("catalog Byte enum underlying type"));
            }
            let mut value = 0_u8;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut u8).cast());
            Ok(value)
        }
    }

    pub(super) unsafe fn catalog_live(&self, attached: &AttachedThread<'_>, images: &Images, object: &Root<'_, '_>) -> Result<bool, DiscoveryError> {
        // SAFETY: inherits read_catalog_page's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let _ = attached;
            let unity_object = self.class(images, c"UnityEngine", c"Object")?;
            self.check_object(unity_object, target(object)?)?;
            let field = self.field(unity_object, c"m_CachedPtr")?;
            if self.field_class(field)? != self.system_class(c"IntPtr")? || (self.field_flags)(field) & 0x10 != 0 {
                return Err(unsupported("catalog Unity object native pointer"));
            }
            let mut value = 0_usize;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut usize).cast());
            Ok(value != 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ptr, Type};
    use super::*;
    use crate::il2cpp::ThreadContext;
    use std::cell::Cell;

    const CHILD: usize = 0x1000;
    const SECOND: usize = 0x2000;
    const CHILD_CLASS: usize = 0x3000;
    const ARRAY_CLASS: usize = 0x4000;
    const BYTE_CLASS: usize = 0x5000;
    const ENUM_CLASS: usize = 0x6000;
    const BASE_TYPE: usize = 0x7000;
    thread_local! {
        static ROOTS: Cell<usize> = const { Cell::new(0) };
        static FREES: Cell<usize> = const { Cell::new(0) };
        static ARRAY: Cell<usize> = const { Cell::new(0) };
        static FAIL_SECOND: Cell<bool> = const { Cell::new(false) };
        static RANK: Cell<i32> = const { Cell::new(1) };
        static ELEMENT: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static READS: Cell<usize> = const { Cell::new(0) };
        static FLAGS: Cell<i32> = const { Cell::new(0x10) };
        static FIELD_CLASS: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static IS_ENUM: Cell<bool> = const { Cell::new(true) };
        static BASE_CLASS: Cell<usize> = const { Cell::new(BYTE_CLASS) };
        static ENUM_BASE_CALLS: Cell<usize> = const { Cell::new(0) };
    }
    fn token<T>(value: usize) -> *mut T {
        std::ptr::without_provenance_mut(value)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(1)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(2)
    }
    unsafe extern "C-unwind" fn root_new(object: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        if object as usize == SECOND && FAIL_SECOND.get() {
            return std::ptr::null_mut();
        }
        ROOTS.set(ROOTS.get() + 1);
        Box::into_raw(Box::new(object)).cast()
    }
    unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
        // SAFETY: the live handle is a Box allocated by root_new.
        unsafe { handle.cast::<*mut raw::Object>().read() }
    }
    unsafe extern "C-unwind" fn root_free(handle: Ptr) {
        // SAFETY: StrongRoot frees this fake handle once after its final target access.
        unsafe {
            drop(Box::from_raw(handle.cast::<*mut raw::Object>()));
        }
        ROOTS.set(ROOTS.get() - 1);
        FREES.set(FREES.get() + 1);
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        if object as usize == ARRAY.get() {
            token(ARRAY_CLASS)
        } else {
            assert!(ROOTS.get() >= if ARRAY.get() == 0 { 1 } else { 2 }, "children must be rooted before metadata");
            token(CHILD_CLASS)
        }
    }
    unsafe extern "C-unwind" fn class_value(class: *const raw::Class) -> bool {
        matches!(class as usize, BYTE_CLASS | ENUM_CLASS)
    }
    unsafe extern "C-unwind" fn rank(_: *const raw::Class) -> i32 {
        RANK.get()
    }
    unsafe extern "C-unwind" fn class_type(_: Class) -> Type {
        token(0x8000)
    }
    unsafe extern "C-unwind" fn type_code(ty: Type) -> i32 {
        if ty as usize == BASE_TYPE { 0x05 } else { 0x1d }
    }
    unsafe extern "C-unwind" fn element(_: Class) -> Class {
        token(ELEMENT.get())
    }
    unsafe extern "C-unwind" fn header() -> u32 {
        32
    }
    unsafe extern "C-unwind" fn length_offset() -> u32 {
        24
    }
    unsafe extern "C-unwind" fn array_length(_: *mut raw::Object) -> u32 {
        3
    }
    unsafe extern "C-unwind" fn assignable(expected: Class, actual: Class) -> bool {
        expected == actual
    }
    unsafe extern "C-unwind" fn field(_: Class, _: *const std::ffi::c_char) -> Ptr {
        token(0x9000)
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> Type {
        token(0xa000)
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        0
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        false
    }
    unsafe extern "C-unwind" fn type_class(ty: Type) -> Class {
        token(if ty as usize == BASE_TYPE { BASE_CLASS.get() } else { FIELD_CLASS.get() })
    }
    unsafe extern "C-unwind" fn static_get(_: Ptr, destination: Ptr) {
        READS.set(READS.get() + 1);
        // SAFETY: checked reference metadata supplies one writable pointer slot.
        unsafe {
            destination.cast::<*mut raw::Object>().write(token(CHILD));
        }
    }
    unsafe extern "C-unwind" fn field_get(_: *mut raw::Object, _: Ptr, destination: Ptr) {
        READS.set(READS.get() + 1);
        // SAFETY: the Byte enum reader verified the exact byte storage before this call.
        unsafe {
            destination.cast::<u8>().write(255);
        }
    }
    unsafe extern "C-unwind" fn name(_: Class) -> *const std::ffi::c_char {
        c"SP".as_ptr()
    }
    unsafe extern "C-unwind" fn class_enum(_: *const raw::Class) -> bool {
        IS_ENUM.get()
    }
    unsafe extern "C-unwind" fn enum_base(_: Class) -> Type {
        ENUM_BASE_CALLS.set(ENUM_BASE_CALLS.get() + 1);
        token(BASE_TYPE)
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        token(0xb000)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, _: *const std::ffi::c_char, name: *const std::ffi::c_char) -> Class {
        // SAFETY: production supplies a static, nul-terminated primitive name.
        assert_eq!(unsafe { CStr::from_ptr(name) }, c"Byte");
        token(BYTE_CLASS)
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        ROOTS.set(0);
        FREES.set(0);
        ARRAY.set(0);
        FAIL_SECOND.set(false);
        RANK.set(1);
        ELEMENT.set(CHILD_CLASS);
        READS.set(0);
        FLAGS.set(0x10);
        FIELD_CLASS.set(CHILD_CLASS);
        IS_ENUM.set(true);
        BASE_CLASS.set(BYTE_CLASS);
        ENUM_BASE_CALLS.set(0);
        let mut access = RuntimeAccess::test_stubs();
        access.object_class = object_class;
        access.class_value = class_value;
        access.class_rank = rank;
        access.class_type = class_type;
        access.type_code = type_code;
        access.class_element = element;
        access.array_header_size = header;
        access.array_length_offset = length_offset;
        access.array_length = array_length;
        access.assignable = assignable;
        access.class_field = field;
        access.field_type = field_type;
        access.field_flags = flags;
        access.field_offset = offset;
        access.type_byref = byref;
        access.type_class = type_class;
        access.static_get = static_get;
        access.field_get = field_get;
        access.class_name = name;
        access.class_enum = class_enum;
        access.enum_base = enum_base;
        access.corlib = corlib;
        access.class_from_name = class_from_name;
        let ownership = raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: <raw::ThreadAttach as super::super::TestExportStub>::test_stub(),
            thread_detach: <raw::ThreadDetach as super::super::TestExportStub>::test_stub(),
            gchandle_new: root_new,
            gchandle_free: root_free,
            gchandle_get_target: root_target,
        };
        // SAFETY: authored functions implement this borrowed test thread and Box-backed handles.
        (access, unsafe { ThreadContext::new(ownership) })
    }

    #[test]
    fn retained_capacity_and_active_count_have_independent_bounds() {
        assert_eq!(collection_count(3, 4096, 8).unwrap(), 3);
        for (count, capacity, maximum) in [(-1, 4, 8), (9, 16, 8), (4, 3, 8), (0, 65537, 8)] {
            assert!(collection_count(count, capacity, maximum).is_err());
        }
    }

    #[test]
    fn pages_cover_filtered_definitions_without_cursor_wrap() {
        assert_eq!(page_bounds(17, 0).unwrap(), 0..16);
        assert_eq!(page_bounds(17, 16).unwrap(), 16..17);
        assert_eq!(page_bounds(17, 17).unwrap(), 17..17);
        assert!(page_bounds(17, 18).is_err());
        assert!(page_bounds(17, u32::MAX).is_err());
    }

    #[test]
    fn aggregate_budget_stops_before_large_graph_materialization_or_integer_wrap() {
        let mut budget = CopyBudget::new();
        budget.charge(crate::protocol::MAX_RESPONSE_SIZE - 16384).unwrap();
        assert!(budget.charge(1).is_err());
        assert!(CopyBudget::new().charge(usize::MAX).is_err());
        let mut budget = CopyBudget::new();
        let mut accepted = 0;
        while budget.charge(4096 + 32).is_ok() {
            accepted += 1;
        }
        assert!(accepted < 256, "a full maximum-sized subtype list must stop early");
    }

    #[test]
    fn subtype_selection_matches_game_getter_including_whitespace() {
        assert_eq!(subtype_name(" ".into(), "internal".into()).source, DefinitionNameSource::StoredDisplay);
        let selected = subtype_name(String::new(), "internal".into());
        assert_eq!(selected.text, "internal");
        assert_eq!(selected.source, DefinitionNameSource::InternalName);
    }

    #[test]
    fn static_metadata_refuses_literal_instance_and_wrong_width_before_copy() {
        for case in 0..3 {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            match case {
                0 => FLAGS.set(0),
                1 => FLAGS.set(0x50),
                _ => FIELD_CLASS.set(BYTE_CLASS),
            }
            // SAFETY: deliberately malformed authored metadata must fail before a static field read.
            assert!(unsafe { access.catalog_static(&attached, token(0xc000), c"storage", token(CHILD_CLASS)) }.is_err());
            assert_eq!(READS.get(), 0);
            assert_eq!(ROOTS.get(), 0);
        }
    }

    #[test]
    fn byte_enum_requires_enum_and_exact_underlying_width_before_write() {
        for case in 0..4 {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            FIELD_CLASS.set(ENUM_CLASS);
            FLAGS.set(0);
            // SAFETY: this immutable fake identity is recognized by our Box-backed root API.
            let object = unsafe { attached.root(NonNull::new(token(CHILD)).unwrap()) }.unwrap();
            match case {
                0 => IS_ENUM.set(false),
                1 => BASE_CLASS.set(CHILD_CLASS),
                2 => FLAGS.set(0x10),
                _ => {}
            }
            // SAFETY: fake metadata either proves Byte storage or deliberately rejects it before writing.
            let result = unsafe { access.catalog_enum_u8(&object, c"property", c"SP") };
            if case == 3 {
                assert_eq!(result.unwrap(), 255);
                assert_eq!(READS.get(), 1);
            } else {
                assert!(result.is_err());
                assert_eq!(READS.get(), 0);
            }
            if case == 0 || case == 2 {
                assert_eq!(ENUM_BASE_CALLS.get(), 0);
            }
        }
    }

    #[test]
    fn numeric_list_element_width_is_proved_before_any_storage_query() {
        for non_enum in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            // SAFETY: immutable fake identity is rooted through this test's Box-backed handle API.
            let list = unsafe { attached.root(NonNull::new(token(CHILD)).unwrap()) }.unwrap();
            IS_ENUM.set(!non_enum);
            BASE_CLASS.set(CHILD_CLASS);
            // SAFETY: deliberately wrong enum storage must fail before class/List/array metadata
            // or slot access. Unprovided exports panic if the reader reaches that later stage.
            assert!(unsafe { access.catalog_values::<u8>(&attached, &list, token(ENUM_CLASS), 8) }.is_err());
            assert_eq!(READS.get(), 0);
            assert_eq!(ENUM_BASE_CALLS.get(), usize::from(!non_enum));
            assert_eq!(ROOTS.get(), 1);
        }
    }

    #[test]
    fn nullable_array_entries_are_rooted_and_partial_failure_releases_children() {
        for fail in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            let mut storage = [0_usize, 0, 0, 3, CHILD, 0, SECOND];
            let pointer = NonNull::new(storage.as_mut_ptr().cast::<raw::Object>()).unwrap();
            ARRAY.set(pointer.as_ptr() as usize);
            // SAFETY: storage is a live immutable authored x64 reference array for this operation.
            let array = unsafe { attached.root(pointer) }.unwrap();
            FAIL_SECOND.set(fail);
            // SAFETY: exports validate this exact authored array layout and rooted identity graph.
            let result = unsafe { access.catalog_array(&attached, &array, token(CHILD_CLASS), 3) };
            if fail {
                assert!(result.is_err());
                assert_eq!(ROOTS.get(), 1);
                assert_eq!(FREES.get(), 1);
            } else {
                let entries = result.unwrap();
                assert_eq!(entries.len(), 3);
                assert!(entries[1].is_none());
                assert_eq!(ROOTS.get(), 3);
                drop(entries);
            }
            drop(array);
            assert_eq!(ROOTS.get(), 0);
        }
    }

    #[test]
    fn wrong_array_shape_is_rejected_before_touching_unavailable_storage() {
        for wrong_rank in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            ARRAY.set(0xd000);
            // SAFETY: the fake API recognizes this identity; its array bytes are deliberately unavailable.
            let array = unsafe { attached.root(NonNull::new(token(0xd000)).unwrap()) }.unwrap();
            if wrong_rank {
                RANK.set(2);
            } else {
                ELEMENT.set(BYTE_CLASS);
            }
            // SAFETY: invalid metadata must fail before attempting any raw data or header read.
            assert!(unsafe { access.catalog_array(&attached, &array, token(CHILD_CLASS), 3) }.is_err());
            assert_eq!(ROOTS.get(), 1);
        }
    }
}
