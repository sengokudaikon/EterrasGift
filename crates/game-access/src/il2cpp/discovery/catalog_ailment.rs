//! Ready ailment cache snapshot through System.Array.GetValue boxes.
//! No generic/outref invocation, native struct layout, singleton getter or dictionary mutation.

use super::metadata_text;
use super::{
    Class, DiscoveryError, Images, RuntimeAccess,
    forge::{Root, target},
    unsupported,
};
use crate::il2cpp::{Argument, AttachedThread, CheckedMethod, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use eg_trainer_wire::catalog::Availability;
use std::{collections::BTreeMap, ffi::CStr, ptr::NonNull};

fn unavailable<T>(reason: impl Into<String>) -> Availability<T> {
    Availability::Unavailable { reason: reason.into() }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    hash: i32,
    next: i32,
    key: u8,
}

// This validates the whole admitted ready dictionary, not a current source-list approximation.
fn coherent_dictionary(entries: &[Entry], buckets: &[i32], free_count: i32, free_list: i32) -> Result<(), DiscoveryError> {
    if entries.len() > 4096 || buckets.len() > 4096 || free_count < 0 || free_count as usize > entries.len() {
        return Err(unsupported("ailment dictionary counts"));
    }
    if buckets.is_empty() && !entries.is_empty() {
        return Err(unsupported("ailment dictionary buckets absent"));
    }
    let mut visited = vec![false; entries.len()];
    let mut keys = [false; 256];
    let mut active = 0;
    for (bucket, first) in buckets.iter().enumerate() {
        let mut index = first.checked_sub(1).ok_or_else(|| unsupported("ailment dictionary bucket overflow"))?;
        while index != -1 {
            let index_usize = usize::try_from(index).map_err(|_| unsupported("ailment dictionary bucket/next index"))?;
            let entry = entries.get(index_usize).ok_or_else(|| unsupported("ailment dictionary bucket/next bound"))?;
            if visited[index_usize] || entry.hash < 0 || entry.hash != i32::from(entry.key) || entry.hash as usize % buckets.len() != bucket || keys[usize::from(entry.key)] {
                return Err(unsupported("ailment dictionary cycle/duplicate/hash coherence"));
            }
            visited[index_usize] = true;
            keys[usize::from(entry.key)] = true;
            active += 1;
            index = entry.next;
        }
    }
    let mut next = free_list;
    for _ in 0..free_count {
        let index = usize::try_from(next).map_err(|_| unsupported("ailment dictionary free index"))?;
        let entry = entries.get(index).ok_or_else(|| unsupported("ailment dictionary free bound"))?;
        if visited[index] || entry.hash >= 0 {
            return Err(unsupported("ailment dictionary free chain"));
        }
        visited[index] = true;
        next = entry.next;
    }
    if next != -1 || active + free_count as usize != entries.len() || visited.iter().any(|visited| !visited) {
        return Err(unsupported("ailment dictionary active/free coverage"));
    }
    Ok(())
}

pub(super) struct AilmentSnapshot<'a, 'r> {
    _dictionary: Root<'a, 'r>,
    values: BTreeMap<u8, Option<Root<'a, 'r>>>,
    ailment_class: Class,
    owner: Class,
    effects: BTreeMap<u8, Availability<u8>>,
}

impl RuntimeAccess {
    unsafe fn ailment_key_class(&self, class: Class) -> Result<(), DiscoveryError> {
        // SAFETY: The descriptor comes from the retained dictionary metadata; generic class identity is checked before use.
        unsafe {
            // EnumBase must not be called on an arbitrary value type (it may assert).
            if class.is_null() || !(self.class_enum)(class) {
                return Err(unsupported("ailment Byte enum key"));
            }
            let base = (self.enum_base)(class);
            if base.is_null() || (self.type_byref)(base) || (self.type_code)(base) != 0x05 || (self.type_class)(base) != self.system_class(c"Byte")? {
                return Err(unsupported("ailment Byte enum key"));
            }
            Ok(())
        }
    }
    unsafe fn ailment_corlib_class(&self, class: Class, namespace: &CStr, name: &CStr) -> Result<(), DiscoveryError> {
        // SAFETY: The class descriptor is live metadata; its nonnull name and namespace identify the expected corlib type.
        unsafe {
            if class.is_null() || (self.class_image)(class) != (self.corlib)() {
                return Err(unsupported("ailment corlib class image"));
            }
            let actual_name = (self.class_name)(class);
            let actual_namespace = (self.class_namespace)(class);
            if actual_name.is_null() || actual_namespace.is_null() || metadata_text(actual_name) != name || metadata_text(actual_namespace) != namespace {
                return Err(unsupported("ailment corlib class name"));
            }
            Ok(())
        }
    }
    unsafe fn ailment_no_cctor(&self, class: Class) -> Result<(), DiscoveryError> {
        // SAFETY: The retained class's bounded method enumeration is inspected without invoking an initializer.
        unsafe {
            for method in self.methods(class)? {
                let name = (self.method_name)(method);
                if name.is_null() || metadata_text(name) == c".cctor" {
                    return Err(unsupported("ailment Entry boxing initializer"));
                }
            }
            Ok(())
        }
    }
    unsafe fn ailment_array_get(&self, api: &raw::InvocationApi) -> Result<NonNull<raw::MethodInfo>, DiscoveryError> {
        // SAFETY: Array and primitive descriptors are retained metadata; the exact GetValue(Int32) signature is validated before binding.
        unsafe {
            let array = self.system_class(c"Array")?;
            let object = self.system_class(c"Object")?;
            let integer = self.system_class(c"Int32")?;
            let mut found = None;
            for method in self.methods(array)? {
                let name = (self.method_name)(method);
                if name.is_null() {
                    return Err(unsupported("array method name"));
                }
                if metadata_text(name) != c"GetValue" || (api.method_get_param_count)(method) != 1 {
                    continue;
                }
                let parameter = (api.method_get_param)(method, 0);
                if parameter.is_null() || (api.type_is_byref)(parameter) || (api.type_get_type)(parameter) != 0x08 || (api.class_from_type)(parameter) != integer {
                    continue;
                }
                let returns = (api.method_get_return_type)(method);
                let mut impl_flags = 0;
                if found.is_some()
                    || !(api.method_is_instance)(method)
                    || (api.method_get_class)(method) != array
                    || returns.is_null()
                    || (api.type_is_byref)(returns)
                    || (api.class_from_type)(returns) != object
                    || (api.type_get_type)(returns) != 0x1c
                    || (api.method_get_flags)(method, &mut impl_flags) & 0x0440 != 0
                {
                    return Err(unsupported("System.Array.GetValue exact signature"));
                }
                found = NonNull::new(method.cast_mut());
            }
            found.ok_or_else(|| unsupported("System.Array.GetValue(Int32) missing"))
        }
    }
    unsafe fn ailment_array_box<'a, 'r>(&self, method: &CheckedMethod<'_, 'a, 'r>, array: &Root<'_, '_>, index: u32, expected: Class) -> Result<Root<'a, 'r>, DiscoveryError> {
        // SAFETY: The checked Array getter roots its boxed result; exact class validation precedes reading it.
        unsafe {
            let mut arguments = [Argument::Scalar(Scalar::I32(index.try_into().map_err(|_| unsupported("array index"))?))];
            let result = method.invoke(Some(array), &mut arguments).map_err(|error| unsupported(error.to_string()))?;
            let InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) = result else {
                return Err(unsupported("array box result/exception"));
            };
            // CheckedMethod rooted the opaque generic box before any result metadata query.
            self.exact(&root, expected)?;
            Ok(root)
        }
    }

    /// In addition to the catalog main-thread/immutable nonmoving graph
    /// contract, the caller authorizes this exact corlib Array.GetValue(Int32) body and its
    /// rank/bounds/reflection checks, runtime type caches, allocation/boxing and GC callbacks.
    /// Generic Entry .cctor is rejected. No item/gameplay constructor or getter is authorized.
    pub(super) unsafe fn catalog_ailments<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        images: &Images,
        api: raw::InvocationApi,
    ) -> Result<Availability<AilmentSnapshot<'a, 'r>>, DiscoveryError> {
        // SAFETY: The caller excludes definition changes; registry, arrays and boxed entries stay rooted throughout bounded extraction.
        unsafe {
            let Some(owner) = self.catalog_optional_class(images, c"", c"AilmentList")? else {
                return Ok(unavailable("Optional ailment dictionary capability is absent"));
            };
            let ready = self.static_field(owner, c"dictionaryInitialised", "System.Boolean")?;
            if self.field_class(ready)? != self.system_class(c"Boolean")? {
                return Err(unsupported("ailment ready Boolean identity"));
            }
            let mut ready_value = 0_u8;
            (self.static_get)(ready, (&mut ready_value as *mut u8).cast());
            match ready_value {
                0 => return Ok(unavailable("Ailment dictionary is not already initialized")),
                1 => {}
                _ => return Err(unsupported("ailment ready Boolean")),
            }
            let dictionary_field = self.field(owner, c"cachedAilmentLookup")?;
            let dictionary_class = self.field_class(dictionary_field)?;
            self.ailment_corlib_class(dictionary_class, c"System.Collections.Generic", c"Dictionary`2")?;
            if (self.class_value)(dictionary_class) || (api.class_is_generic)(dictionary_class) || !(api.class_is_inflated)(dictionary_class) {
                return Err(unsupported("closed ailment dictionary class"));
            }
            let Some(dictionary) = self.catalog_static(attached, owner, c"cachedAilmentLookup", dictionary_class)? else {
                return Ok(unavailable("Ailment cache dictionary is absent"));
            };
            self.exact(&dictionary, dictionary_class)?;
            if self.catalog_reference(attached, &dictionary, c"_comparer")?.is_some() {
                return Ok(unavailable("Custom ailment dictionary comparer has not been admitted"));
            }
            let version: i32 = self.primitive(&dictionary, c"_version")?;
            let count: i32 = self.primitive(&dictionary, c"_count")?;
            let free_count: i32 = self.primitive(&dictionary, c"_freeCount")?;
            let free_list: i32 = self.primitive(&dictionary, c"_freeList")?;
            if !(0..=4096).contains(&count) || free_count < 0 || free_count > count {
                return Err(unsupported("ailment dictionary count/freeCount"));
            }
            let ailment_class = self.class(images, c"", c"Ailment")?;
            let key_class = self.class(images, c"", c"AilmentID")?;
            self.ailment_key_class(key_class)?;
            let integer = self.system_class(c"Int32")?;
            let entries_field = self.field(dictionary_class, c"_entries")?;
            if (self.field_parent)(entries_field) != dictionary_class {
                return Err(unsupported("ailment dictionary entries declaring class"));
            }
            let entries_class = self.field_class(entries_field)?;
            let entry_class = (self.class_element)(entries_class);
            if entry_class.is_null() {
                return Err(unsupported("closed corlib ailment Entry"));
            }
            let entry_name = (self.class_name)(entry_class);
            if (self.class_image)(entry_class) != (self.corlib)()
                || entry_name.is_null()
                || metadata_text(entry_name) != c"Entry"
                || !(self.class_value)(entry_class)
                || (self.class_enum)(entry_class)
                || (api.class_is_generic)(entry_class)
                || !(api.class_is_inflated)(entry_class)
            {
                return Err(unsupported("closed corlib ailment Entry"));
            }
            self.ailment_no_cctor(entry_class)?;
            for (field, expected) in [(c"hashCode", integer), (c"next", integer), (c"key", key_class), (c"value", ailment_class)] {
                let field = self.field(entry_class, field)?;
                if (self.field_parent)(field) != entry_class || (self.field_flags)(field) & 0x10 != 0 || self.field_class(field)? != expected {
                    return Err(unsupported("ailment Entry exact field identity/storage"));
                }
            }
            self.array_class(entries_class, entry_class)?;
            let buckets_field = self.field(dictionary_class, c"_buckets")?;
            self.array_class(self.field_class(buckets_field)?, integer)?;
            let buckets = self.catalog_reference(attached, &dictionary, c"_buckets")?;
            let entries = self.catalog_reference(attached, &dictionary, c"_entries")?;
            let (buckets, entries) = match (buckets, entries) {
                (Some(buckets), Some(entries)) => (buckets, entries),
                (None, None) if count == 0 && free_count == 0 && matches!(free_list, -1 | 0) => {
                    if self.primitive::<i32>(&dictionary, c"_version")? != version {
                        return Err(unsupported("ailment dictionary changed during snapshot"));
                    }
                    return Ok(Availability::Available(AilmentSnapshot {
                        _dictionary: dictionary,
                        values: BTreeMap::new(),
                        ailment_class,
                        owner,
                        effects: BTreeMap::new(),
                    }));
                }
                _ => return Err(unsupported("ailment empty dictionary state")),
            };
            let bucket_count = self.array_count(&buckets, integer, 4096)?;
            if self.array_count(&entries, entry_class, 4096)? != bucket_count || count as u32 > bucket_count {
                return Err(unsupported("ailment Entry/bucket capacity"));
            }
            let context = InvocationContext::new(attached, api);
            let method = context.bind(self.ailment_array_get(&api)?).map_err(|error| unsupported(error.to_string()))?;
            let mut raw_entries = Vec::new();
            let mut values = BTreeMap::new();
            for index in 0..count as u32 {
                let boxed = self.ailment_array_box(&method, &entries, index, entry_class)?;
                let hash = self.primitive::<i32>(&boxed, c"hashCode")?;
                let next = self.primitive::<i32>(&boxed, c"next")?;
                let key = if hash >= 0 { self.catalog_enum_u8(&boxed, c"key", c"AilmentID")? } else { 0 };
                if hash >= 0 {
                    let value = self.reference(attached, &boxed, c"value", ailment_class)?;
                    if let Some(value) = &value {
                        self.exact(value, ailment_class)?;
                    }
                    if values.insert(key, value).is_some() {
                        return Err(unsupported("duplicate active ailment dictionary key"));
                    }
                }
                raw_entries.push(Entry { hash, next, key });
            }
            let mut raw_buckets = Vec::new();
            for index in 0..bucket_count {
                let boxed = self.ailment_array_box(&method, &buckets, index, integer)?;
                raw_buckets.push(self.primitive::<i32>(&boxed, c"m_value")?);
            }
            coherent_dictionary(&raw_entries, &raw_buckets, free_count, free_list)?;
            if self.primitive::<i32>(&dictionary, c"_version")? != version {
                return Err(unsupported("ailment dictionary changed during snapshot"));
            }
            Ok(Availability::Available(AilmentSnapshot {
                _dictionary: dictionary,
                values,
                ailment_class,
                owner,
                effects: BTreeMap::new(),
            }))
        }
    }

    pub(super) unsafe fn catalog_ailment_effect(
        &self,
        attached: &AttachedThread<'_>,
        images: &Images,
        snapshot: &mut AilmentSnapshot<'_, '_>,
        key: u8,
    ) -> Result<Availability<u8>, DiscoveryError> {
        // SAFETY: The rooted entry's field metadata determines exact primitive and reference storage before copying.
        unsafe {
            if let Some(value) = snapshot.effects.get(&key) {
                return Ok(value.clone());
            }
            let root = if let Some(value) = snapshot.values.get(&key) {
                let Some(value) = value else {
                    return Ok(unavailable("Cached ailment value is null"));
                };
                attached.root(target(value)?)?
            } else {
                // Actual cache miss logs and returns current GlobalAssets AilmentList.list[0].
                // Bypass logging/resource getters, but reproduce that selected object exactly.
                let globals = self.class(images, c"LE.AssetManagement", c"GlobalAssets")?;
                let Some(master) = self.catalog_static(attached, globals, c"_storage_AilmentList", snapshot.owner)? else {
                    return Ok(unavailable("Cache-miss ailment fallback asset is not already ready"));
                };
                self.exact(&master, snapshot.owner)?;
                if !self.catalog_live(attached, images, &master)? {
                    return Ok(unavailable("Cache-miss ailment fallback asset is destroyed"));
                }
                let Some(list) = self.catalog_reference(attached, &master, c"list")? else {
                    return Ok(unavailable("Cache-miss ailment fallback list is absent"));
                };
                let entries = self.catalog_list(attached, &list, snapshot.ailment_class, 4096)?;
                let Some(value) = entries.first() else {
                    return Ok(unavailable("Cache-miss ailment fallback list is empty"));
                };
                self.exact(value, snapshot.ailment_class)?;
                attached.root(target(value)?)?
            };
            if !self.catalog_live(attached, images, &root)? {
                return Ok(unavailable("Selected ailment native object is destroyed"));
            }
            let effect = self.enumeration(&root, c"effectOfIncreasedEffectiveness", c"EffectOfIncreasedEffectiveness")?;
            let selected = Availability::Available(if effect == 1 { 59 } else { 43 });
            snapshot.effects.insert(key, selected.clone());
            Ok(selected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Method, Ptr, Type};
    use super::*;
    use std::cell::Cell;
    thread_local! {
        static ENUM: Cell<bool> = const {Cell::new(true)};
        static BYREF: Cell<bool> = const {Cell::new(false)};
        static CODE: Cell<i32> = const {Cell::new(5)};
        static BASE: Cell<usize> = const {Cell::new(0x4000)};
        static ENUM_BASE_CALLS: Cell<usize> = const {Cell::new(0)};
        static CCTOR: Cell<bool> = const {Cell::new(false)};
    }
    fn pointer<T>(value: usize) -> *mut T {
        std::ptr::without_provenance_mut(value)
    }
    unsafe extern "C-unwind" fn is_enum(_: *const raw::Class) -> bool {
        ENUM.get()
    }
    unsafe extern "C-unwind" fn enum_base(_: Class) -> Type {
        ENUM_BASE_CALLS.set(ENUM_BASE_CALLS.get() + 1);
        pointer(0x2000)
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn code(_: Type) -> i32 {
        CODE.get()
    }
    unsafe extern "C-unwind" fn class(_: Type) -> Class {
        pointer(BASE.get())
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        pointer(0x3000)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, _: *const std::ffi::c_char, name: *const std::ffi::c_char) -> Class {
        // SAFETY: the runtime adapter supplies one static NUL-terminated class name.
        assert_eq!(unsafe { CStr::from_ptr(name) }, c"Byte");
        pointer(0x4000)
    }
    unsafe extern "C-unwind" fn methods(_: Class, iterator: *mut Ptr) -> Method {
        // SAFETY: caller provides one initialized writable metadata iterator.
        unsafe {
            if (*iterator).is_null() {
                *iterator = pointer(1);
                pointer(0x5000)
            } else {
                std::ptr::null()
            }
        }
    }
    unsafe extern "C-unwind" fn method_name(_: Method) -> *const std::ffi::c_char {
        if CCTOR.get() { c".cctor".as_ptr() } else { c"Read".as_ptr() }
    }
    #[test]
    fn key_metadata_rejects_nonenum_and_wrong_width_before_any_value_copy() {
        for (enumeration, by_ref, code_value, base, accepted) in [
            (true, false, 5, 0x4000, true),
            (false, false, 5, 0x4000, false),
            (true, true, 5, 0x4000, false),
            (true, false, 8, 0x4000, false),
            (true, false, 5, 0x4001, false),
        ] {
            ENUM.set(enumeration);
            BYREF.set(by_ref);
            CODE.set(code_value);
            BASE.set(base);
            ENUM_BASE_CALLS.set(0);
            let mut access = RuntimeAccess::test_stubs();
            access.class_enum = is_enum;
            access.enum_base = enum_base;
            access.type_byref = byref;
            access.type_code = code;
            access.type_class = class;
            access.corlib = corlib;
            access.class_from_name = class_from_name;
            // SAFETY: fake metadata has no object/field-copy export; only exact key metadata is inspected.
            assert_eq!(unsafe { access.ailment_key_class(pointer(0x1000)) }.is_ok(), accepted);
            assert_eq!(ENUM_BASE_CALLS.get(), usize::from(enumeration));
        }
    }
    #[test]
    fn generic_entry_initializer_is_refused_before_any_box_allocation() {
        let mut access = RuntimeAccess::test_stubs();
        access.class_methods = methods;
        access.method_name = method_name;
        for cctor in [false, true] {
            CCTOR.set(cctor);
            // SAFETY: fake method enumeration is bounded and no allocation/invoke export is provided.
            assert_eq!(unsafe { access.ailment_no_cctor(pointer(0x1000)) }.is_ok(), !cctor);
        }
    }
    fn fixture() -> (Vec<Entry>, Vec<i32>) {
        // Two colliding live keys, plus a removed slot in the free chain.
        (
            vec![
                Entry { hash: 1, next: -1, key: 1 },
                Entry { hash: -1, next: -1, key: 0 },
                Entry { hash: 8, next: 0, key: 8 },
            ],
            vec![0, 3, 0, 0, 0, 0, 0],
        )
    }
    #[test]
    fn real_dictionary_shape_preserves_collision_order_and_removed_slots() {
        let (entries, buckets) = fixture();
        coherent_dictionary(&entries, &buckets, 1, 1).unwrap();
        coherent_dictionary(&[], &[], 0, -1).unwrap();
        coherent_dictionary(&[], &[0, 0, 0], 0, -1).unwrap();
    }
    #[test]
    fn duplicate_keys_incoherent_hashes_cycles_and_wrong_bucket_membership_are_refused() {
        for case in 0..7 {
            let (mut entries, mut buckets) = fixture();
            match case {
                0 => {
                    entries[2].key = 1;
                    entries[2].hash = 1;
                }
                1 => entries[2].hash = 123,
                2 => entries[0].next = 2,
                3 => {
                    buckets[1] = 0;
                    buckets[2] = 3;
                }
                4 => entries[2].next = 99,
                5 => buckets[1] = i32::MIN,
                _ => buckets[0] = 1,
            }
            assert!(coherent_dictionary(&entries, &buckets, 1, 1).is_err(), "case {case}");
        }
    }
    #[test]
    fn free_chain_and_active_count_must_cover_every_allocated_entry_once() {
        let (mut entries, buckets) = fixture();
        for (count, head) in [(0, -1), (1, -1), (1, 0), (2, 1), (4, 1), (-1, 1)] {
            assert!(coherent_dictionary(&entries, &buckets, count, head).is_err());
        }
        entries[1].next = 1;
        assert!(coherent_dictionary(&entries, &buckets, 1, 1).is_err());
        let (entries, _) = fixture();
        assert!(coherent_dictionary(&entries, &[], 1, 1).is_err());
        assert!(coherent_dictionary(&entries, &vec![0; 4097], 1, 1).is_err());
    }
}
