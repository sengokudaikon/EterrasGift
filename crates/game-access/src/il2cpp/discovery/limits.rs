//! Literal metadata only: item status bounds from the original helper's appendItemConstants.

use super::{Class, DiscoveryError, RuntimeAccess, unsupported};
use crate::{
    definitions::{ItemLimits, ValueRange},
    il2cpp::AttachedThread,
};
use std::ffi::CStr;

fn range(minimum: u8, maximum: u8) -> Result<ValueRange, DiscoveryError> {
    if minimum > maximum {
        return Err(unsupported("item constant minimum exceeds maximum"));
    }
    Ok(ValueRange { minimum, maximum })
}

impl RuntimeAccess {
    /// Read an immutable literal through its metadata default, never through guessed field storage.
    /// Internal caller must hold read_item_limits's live-runtime/metadata contract.
    unsafe fn limit_constant(&self, owner: Class, name: &CStr) -> Result<u8, DiscoveryError> {
        unsafe {
            if owner.is_null() {
                return Err(unsupported("item constant owner"));
            }
            let field = self.field(owner, name)?;
            let ty = (self.field_type)(field);
            if ty.is_null() || (self.type_byref)(ty) || (self.field_flags)(field) & 0x50 != 0x50 {
                return Err(unsupported(format!("item constant {} type/flags", name.to_string_lossy())));
            }
            let corlib = (self.corlib)();
            if corlib.is_null() {
                return Err(unsupported("item constant corlib"));
            }
            let int32 = (self.class_from_name)(corlib, c"System".as_ptr(), c"Int32".as_ptr());
            if int32.is_null() || (self.type_class)(ty) != int32 {
                return Err(unsupported("item constant Int32 identity"));
            }
            let declaring = (self.field_parent)(field);
            if declaring.is_null() {
                return Err(unsupported("item constant declaring class"));
            }
            let mut ancestor = owner;
            let mut declared_here = false;
            for _ in 0..256 {
                if ancestor.is_null() {
                    break;
                }
                if ancestor == declaring {
                    declared_here = true;
                    break;
                }
                ancestor = (self.class_parent)(ancestor);
            }
            if !declared_here {
                return Err(unsupported("item constant owner hierarchy"));
            }
            let mut value = 0_i32;
            // For a validated literal, Field::StaticGetValue reads the metadata default and
            // returns before class static storage setup. No class initializer/body is invoked.
            (self.static_get)(field, (&mut value as *mut i32).cast());
            u8::try_from(value).map_err(|_| unsupported(format!("item constant {} outside 0..255", name.to_string_lossy())))
        }
    }

    /// Read the game's own bounds for status fields, without invoking managed/game methods.
    /// This capability excludes rarity names, localization and the full game-data catalog.
    ///
    /// # Safety
    /// All runtime/attachment/initialized-static/main-thread conditions of `status` apply.
    /// Call after the original ExecuteTasks returns, with runtime shutdown, metadata mutation,
    /// concurrent static replacement and native manager destruction excluded for this operation.
    /// The attachment must belong to these live exports. Literal defaults must obey the public
    /// runtime contract for valid metadata. Offline/native-manager admission is independently
    /// checked before any item-limit class or constant is read. The eight ItemData constants and
    /// ExperienceTracker.maxLevel are exact System.Int32 literals; level minimum 0 is the original
    /// helper's defined domain bound, rather than a nonexistent guessed metadata constant.
    pub unsafe fn read_item_limits(&self, attached: &AttachedThread<'_>) -> Result<ItemLimits, DiscoveryError> {
        unsafe {
            let status = self.status(attached)?;
            if status.online || status.manager == 0 {
                return Err(DiscoveryError::OfflineRequired);
            }
            let images = self.images()?;
            let item = self.class(&images, c"", c"ItemData")?;
            let experience = self.class(&images, c"", c"ExperienceTracker")?;
            Ok(ItemLimits {
                forging_potential: range(self.limit_constant(item, c"MinForgingPotential")?, self.limit_constant(item, c"MaxForgingPotential")?)?,
                legendary_potential: range(self.limit_constant(item, c"MinLegendaryPotential")?, self.limit_constant(item, c"MaxLegendaryPotential")?)?,
                weavers_will: range(self.limit_constant(item, c"MinWeaversWill")?, self.limit_constant(item, c"MaxWeaversWill")?)?,
                weavers_touch: range(self.limit_constant(item, c"MinWeaversTouch")?, self.limit_constant(item, c"MaxWeaversTouch")?)?,
                level_requirement: range(0, self.limit_constant(experience, c"maxLevel")?)?,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ptr, TestExportStub, Type};
    use super::*;
    use crate::il2cpp::{ThreadContext, raw};
    use std::{
        cell::Cell,
        ffi::{CString, c_char},
    };

    const OWNER: usize = 0x1000;
    const BASE: usize = 0x1100;
    const FIELD: usize = 0x2000;
    const INT32: usize = 0x3000;
    const OTHER_TYPE: usize = 0x3100;
    const TY: usize = 0x4000;
    const IMAGE: usize = 0x5000;
    const FINDER: usize = 0x6000;
    const ENVIRONMENT: usize = 0x6100;
    const GLOBAL_ASSETS: usize = 0x6200;
    const MANAGER: usize = 0x6300;
    const AFFIX_LIST: usize = 0x6400;
    const ONLINE_FIELD: usize = 0x7000;
    const MANAGER_FIELD: usize = 0x7100;
    const MASTER_FIELD: usize = 0x7200;
    thread_local! {
        static VALUE: Cell<i32> = const { Cell::new(17) };
        static FLAGS: Cell<i32> = const { Cell::new(0x50) };
        static CLASS: Cell<usize> = const { Cell::new(INT32) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static MISSING: Cell<bool> = const { Cell::new(false) };
        static DECLARING: Cell<usize> = const { Cell::new(OWNER) };
        static PARENT: Cell<usize> = const { Cell::new(0) };
        static READS: Cell<usize> = const { Cell::new(0) };
        static CONSTANT_LOOKUPS: Cell<usize> = const { Cell::new(0) };
        static ONLINE: Cell<u8> = const { Cell::new(0) };
        static ASSEMBLIES: [Ptr; 1] = [token(0x5100)];
    }
    fn token<T>(address: usize) -> *mut T {
        std::ptr::without_provenance_mut(address)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(0x5200)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(0x5300)
    }
    unsafe extern "C-unwind" fn assemblies(_: *const raw::Domain, count: *mut usize) -> *const Ptr {
        // SAFETY: the production query supplies one writable count; immutable TLS storage outlives it.
        unsafe {
            count.write(1);
        }
        ASSEMBLIES.with(|values| values.as_ptr())
    }
    unsafe extern "C-unwind" fn assembly_image(_: Ptr) -> Ptr {
        token(IMAGE)
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        token(IMAGE)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, namespace: *const c_char, name: *const c_char) -> Class {
        // SAFETY: the production adapter supplies valid null-terminated class names.
        let (namespace, name) = unsafe { (CStr::from_ptr(namespace), CStr::from_ptr(name)) };
        match (namespace, name) {
            (namespace, name) if namespace == c"System" && name == c"Int32" => token(INT32),
            (_, name) if name == c"PlayerFinder" => token(FINDER),
            (_, name) if name == c"GameplayEnvironment" => token(ENVIRONMENT),
            (_, name) if name == c"GlobalAssets" => token(GLOBAL_ASSETS),
            (_, name) if name == c"ItemContainersManager" => token(MANAGER),
            (_, name) if name == c"AffixList" => token(AFFIX_LIST),
            (_, name) if name == c"ItemData" || name == c"ExperienceTracker" => {
                CONSTANT_LOOKUPS.set(CONSTANT_LOOKUPS.get() + 1);
                token(OWNER)
            }
            _ => panic!("unexpected literal-fixture class lookup"),
        }
    }
    unsafe extern "C-unwind" fn field(class: Class, _: *const c_char) -> Ptr {
        match class as usize {
            OWNER if MISSING.get() => std::ptr::null_mut(),
            OWNER => token(FIELD),
            ENVIRONMENT => token(ONLINE_FIELD),
            FINDER => token(MANAGER_FIELD),
            GLOBAL_ASSETS => token(MASTER_FIELD),
            _ => panic!("unexpected literal-fixture field lookup"),
        }
    }
    unsafe extern "C-unwind" fn parent(class: Class) -> Class {
        if class as usize == OWNER { token(PARENT.get()) } else { std::ptr::null_mut() }
    }
    unsafe extern "C-unwind" fn field_parent(_: Ptr) -> Class {
        token(DECLARING.get())
    }
    unsafe extern "C-unwind" fn field_type(field: Ptr) -> Type {
        token(if field as usize == FIELD { TY } else { field as usize + 8 })
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn flags(field: Ptr) -> i32 {
        if field as usize == FIELD { FLAGS.get() } else { 0x10 }
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        0
    }
    unsafe extern "C-unwind" fn type_class(_: Type) -> Class {
        token(CLASS.get())
    }
    unsafe extern "C-unwind" fn type_name(ty: Type) -> *mut c_char {
        let name = match ty as usize {
            address if address == ONLINE_FIELD + 8 => c"System.Boolean",
            address if address == MANAGER_FIELD + 8 => c"ItemContainersManager",
            address if address == MASTER_FIELD + 8 => c"AffixList",
            _ => panic!("unexpected literal-fixture type name"),
        };
        name.to_owned().into_raw()
    }
    unsafe extern "C-unwind" fn free(allocation: Ptr) {
        // SAFETY: type_name handed ownership of exactly this CString allocation to the runtime caller.
        unsafe {
            drop(CString::from_raw(allocation.cast()));
        }
    }
    unsafe extern "C-unwind" fn get(field: Ptr, destination: Ptr) {
        match field as usize {
            FIELD => {
                READS.set(READS.get() + 1);
                // SAFETY: the production constant reader validated an exact Int32 before this slot.
                unsafe {
                    destination.cast::<i32>().write(VALUE.get());
                }
            }
            ONLINE_FIELD => {
                // SAFETY: the status reader supplied one Boolean storage byte after checking metadata.
                unsafe {
                    destination.cast::<u8>().write(ONLINE.get());
                }
            }
            MANAGER_FIELD | MASTER_FIELD => {
                // SAFETY: these checked reference fields target one writable pointer-sized slot.
                unsafe {
                    destination.cast::<*mut raw::Object>().write(std::ptr::null_mut());
                }
            }
            _ => panic!("unexpected literal-fixture static getter"),
        }
    }
    fn fixture() -> RuntimeAccess {
        VALUE.set(17);
        FLAGS.set(0x50);
        CLASS.set(INT32);
        BYREF.set(false);
        MISSING.set(false);
        DECLARING.set(OWNER);
        PARENT.set(0);
        READS.set(0);
        CONSTANT_LOOKUPS.set(0);
        ONLINE.set(0);
        let mut api = RuntimeAccess::test_stubs();
        api.domain_get = domain;
        api.assemblies = assemblies;
        api.assembly_image = assembly_image;
        api.corlib = corlib;
        api.class_from_name = class_from_name;
        api.class_field = field;
        api.class_parent = parent;
        api.field_parent = field_parent;
        api.field_type = field_type;
        api.type_byref = byref;
        api.field_flags = flags;
        api.field_offset = offset;
        api.type_class = type_class;
        api.type_name = type_name;
        api.runtime_free = free;
        api.static_get = get;
        api
    }
    #[test]
    fn literal_reads_metadata_default_including_an_inherited_constant() {
        let api = fixture();
        // SAFETY: this fake class/field table exposes one immutable, exact Int32 literal.
        assert_eq!(unsafe { api.limit_constant(token(OWNER), c"MinForgingPotential") }.unwrap(), 17);
        DECLARING.set(BASE);
        PARENT.set(BASE);
        VALUE.set(255);
        // SAFETY: the same literal is now declared on a known ancestor of the requested owner.
        assert_eq!(unsafe { api.limit_constant(token(OWNER), c"MinForgingPotential") }.unwrap(), 255);
        assert_eq!(READS.get(), 2);
    }
    #[test]
    fn invalid_width_flags_byref_missing_and_owner_never_call_static_getter() {
        for invalid in 0..7 {
            let api = fixture();
            match invalid {
                0 => CLASS.set(OTHER_TYPE),
                1 => FLAGS.set(0x10),
                2 => FLAGS.set(0x40),
                3 => BYREF.set(true),
                4 => MISSING.set(true),
                5 => DECLARING.set(OTHER_TYPE),
                6 => DECLARING.set(0),
                _ => unreachable!(),
            }
            // SAFETY: deliberately invalid fake metadata must be refused before any default-value copy.
            assert!(unsafe { api.limit_constant(token(OWNER), c"MinForgingPotential") }.is_err());
            assert_eq!(READS.get(), 0);
        }
    }
    #[test]
    fn literal_and_pair_bounds_are_checked_without_truncation() {
        for value in [-1, 256, i32::MIN, i32::MAX] {
            let api = fixture();
            VALUE.set(value);
            // SAFETY: valid Int32 metadata stores an out-of-domain value in a correctly sized slot.
            assert!(unsafe { api.limit_constant(token(OWNER), c"MinForgingPotential") }.is_err());
            assert_eq!(READS.get(), 1);
        }
        assert!(range(2, 1).is_err());
        assert_eq!(range(0, 255).unwrap(), ValueRange { minimum: 0, maximum: 255 });
    }
    #[test]
    fn runtime_offline_gate_denies_before_any_constant_lookup_or_getter() {
        for online in [0, 1] {
            let api = fixture();
            ONLINE.set(online);
            let ownership = raw::Api {
                domain_get: domain,
                thread_current: current,
                thread_attach: <raw::ThreadAttach as TestExportStub>::test_stub(),
                thread_detach: <raw::ThreadDetach as TestExportStub>::test_stub(),
                gchandle_new: <raw::GcHandleNew as TestExportStub>::test_stub(),
                gchandle_free: <raw::GcHandleFree as TestExportStub>::test_stub(),
                gchandle_get_target: <raw::GcHandleTarget as TestExportStub>::test_stub(),
            };
            // SAFETY: the fake engine owns this test thread's existing attachment; status returns
            // null manager/master references, so no handle creation or object access is permitted.
            let runtime = unsafe { ThreadContext::new(ownership) };
            let attached = runtime.attach().unwrap();
            // SAFETY: immutable authored status metadata supplies online or absent-manager admission.
            assert!(matches!(unsafe { api.read_item_limits(&attached) }, Err(DiscoveryError::OfflineRequired)));
            assert_eq!(CONSTANT_LOOKUPS.get(), 0);
            assert_eq!(READS.get(), 0);
        }
    }
}
