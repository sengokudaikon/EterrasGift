//! One metadata-validated Int32 store, with a full guarded snapshot and readback.

use super::{
    Class, DiscoveryError, Ptr, RuntimeAccess,
    forge::{ForgeGraph, target},
};
use crate::{
    forge::ForgeSnapshot,
    il2cpp::{AttachedThread, raw},
};
use std::ptr::NonNull;

/// Only `Failed` may describe an operation whose store was attempted. No variant permits retry.
#[derive(Debug)]
pub enum LevelWriteError {
    NotReady(String),
    Stale(String),
    Refused(String),
    Offline,
    Failed(String),
}

impl std::fmt::Display for LevelWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady(s) | Self::Stale(s) | Self::Refused(s) | Self::Failed(s) => f.write_str(s),
            Self::Offline => f.write_str("An offline character with a live item manager is required"),
        }
    }
}
impl std::error::Error for LevelWriteError {}
impl From<DiscoveryError> for LevelWriteError {
    fn from(error: DiscoveryError) -> Self {
        match error {
            DiscoveryError::OfflineRequired => Self::Offline,
            DiscoveryError::NotReady(s) => Self::NotReady(s.into()),
            other => Self::Failed(other.to_string()),
        }
    }
}
fn uncertain(error: impl std::fmt::Display) -> LevelWriteError {
    LevelWriteError::Failed(format!("Level store attempted; readback unconfirmed: {error}"))
}

// The only store boundary. The caller has validated the exact field and retains all graph roots.
unsafe fn attempt_store<T>(
    api: raw::FieldWriteApi,
    object: NonNull<raw::Object>,
    field: NonNull<raw::FieldInfo>,
    value: i32,
    readback: impl FnOnce() -> Result<T, LevelWriteError>,
) -> Result<T, LevelWriteError> {
    // STORE ATTEMPT BOUNDARY: nothing below may return a definite pre-store refusal.
    // SAFETY: the internal caller establishes FieldWriteApi::set_i32's complete contract.
    unsafe { api.set_i32(object, field, value) };
    readback().map_err(uncertain)
}

impl RuntimeAccess {
    // Internal caller supplies set_level_requirement's full live-runtime/exclusive-write contract.
    unsafe fn level_field(&self, graph: &ForgeGraph<'_, '_>, owner: Class) -> Result<NonNull<raw::FieldInfo>, LevelWriteError> {
        unsafe {
            self.exact(&graph.item, owner)?;
            let field = self.field(owner, c"<LvlReq>k__BackingField")?;
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.type_byref)(ty)
                || (self.type_code)(ty) != 0x08
                || (self.type_class)(ty) != self.system_class(c"Int32")?
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) & 0x70 != 0
                || !(16..=16384).contains(&(self.field_offset)(field))
            {
                return Err(LevelWriteError::Refused(
                    "Level backing field is not the exact mutable ItemDataUnpacked.Int32 instance field".into(),
                ));
            }
            NonNull::new(field.cast()).ok_or_else(|| LevelWriteError::Refused("Missing level field".into()))
        }
    }

    // Uses already validated metadata; no new object is dereferenced from the current statics/edges.
    unsafe fn level_final_gate(&self, graph: &ForgeGraph<'_, '_>, manager: u64) -> Result<(), LevelWriteError> {
        unsafe {
            let held = target(&graph.manager)?;
            if held.as_ptr() as u64 != manager {
                return Err(LevelWriteError::Stale("Current manager changed".into()));
            }
            let mut current: Ptr = std::ptr::null_mut();
            (self.static_get)(graph.manager_field, (&mut current as *mut Ptr).cast());
            if current != held.as_ptr().cast() {
                return Err(LevelWriteError::Stale("Current manager changed".into()));
            }
            let mut cached = 0_usize;
            (self.field_get)(held.as_ptr(), graph.cached_ptr_field, (&mut cached as *mut usize).cast());
            if cached == 0 {
                return Err(LevelWriteError::Offline);
            }
            for &(parent, field, expected) in &graph.links {
                let mut child: Ptr = std::ptr::null_mut();
                (self.field_get)(parent.cast(), field, (&mut child as *mut Ptr).cast());
                if child != expected {
                    return Err(LevelWriteError::Stale("Current forge item mapping changed".into()));
                }
            }
            let mut online = 1_u8;
            (self.static_get)(graph.online_field, (&mut online as *mut u8).cast());
            match online {
                0 => Ok(()),
                1 => Err(LevelWriteError::Offline),
                _ => Err(LevelWriteError::Refused("Noncanonical runtime online Boolean".into())),
            }
        }
    }

    /// Set only the current plain item's level backing field and return a fully matching readback.
    /// No item constructor, getter, rebuild, delegate or game method is invoked.
    ///
    /// # Safety
    /// All `read_forge` runtime/main-thread/nonmoving-GC conditions apply, with exclusive WRITE
    /// access to the entire rooted graph and all relevant statics until completion. The caller
    /// must authorize this exact mutation and exclude callbacks, reentrancy, other writers,
    /// runtime shutdown, native destruction and graph replacement throughout. Both export tables
    /// and attachment must belong to the same initialized source-bound Unity 6000.4 runtime.
    /// `Field::SetValue` must implement the audited direct Int32 assignment (no managed bodies).
    /// Expected addresses are opaque comparisons only. After entering the void setter, failures
    /// are uncertain; this API cannot recover an access violation/SEH or guarantee rollback.
    pub unsafe fn set_level_requirement(
        &self,
        attached: &AttachedThread<'_>,
        write_api: raw::FieldWriteApi,
        expected_manager: u64,
        expected: &ForgeSnapshot,
        value: i32,
    ) -> Result<ForgeSnapshot, LevelWriteError> {
        unsafe {
            if expected_manager == 0 || !(0..=255).contains(&value) || matches!(expected.rarity, 7..=9) {
                return Err(LevelWriteError::Refused("Level edit requires a plain item and a byte-domain value".into()));
            }
            let limits = self.read_item_limits(attached)?;
            if value > i32::from(limits.level_requirement.maximum) {
                return Err(LevelWriteError::Refused("Level exceeds the current game's maxLevel".into()));
            }
            let graph = self.forge_graph(attached)?.ok_or_else(|| LevelWriteError::NotReady("Forge has no current item".into()))?;
            let owner = self.class(&graph.images, c"", c"ItemDataUnpacked")?;
            let field = self.level_field(&graph, owner)?;
            // Metadata/limit validation has finished. Refresh all current graph edges, retaining
            // both graphs' roots until the operation ends, and compare every owned field.
            let current = self.forge_graph(attached)?.ok_or_else(|| LevelWriteError::Stale("Current item disappeared".into()))?;
            let before = self.forge_snapshot(attached, &current)?;
            if before != *expected || target(&graph.item)? != target(&current.item)? {
                return Err(LevelWriteError::Stale("Complete current item differs from expected".into()));
            }
            let mut projected = expected.clone();
            projected.status.level_requirement = value;
            projected.fingerprint = projected.legacy_fingerprint();
            let object = target(&current.item)?;
            self.level_final_gate(&current, expected_manager)?;
            attempt_store(write_api, object, field, value, || {
                self.level_final_gate(&current, expected_manager)?;
                let actual = self.forge_snapshot(attached, &current)?;
                self.level_final_gate(&current, expected_manager)?;
                if actual != projected {
                    return Err(LevelWriteError::Failed("Complete readback differs from projected item".into()));
                }
                Ok(actual)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::il2cpp::ThreadContext;
    use std::{
        cell::Cell,
        ffi::{CStr, c_char},
    };

    const MANAGER: usize = 0x1000;
    const ITEM: usize = 0x2000;
    const OWNER: usize = 0x3000;
    const INT32: usize = 0x4000;
    const FIELD: usize = 0x5000;
    const ONLINE_FIELD: usize = 0x6000;
    const MANAGER_FIELD: usize = 0x6100;
    const CACHED_FIELD: usize = 0x6200;
    const LINK_FIELD: usize = 0x6300;
    thread_local! {
        static ROOTS: Cell<usize> = const { Cell::new(0) };
        static STORES: Cell<usize> = const { Cell::new(0) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static CODE: Cell<i32> = const { Cell::new(8) };
        static TYPE_CLASS: Cell<usize> = const { Cell::new(INT32) };
        static DECLARING: Cell<usize> = const { Cell::new(OWNER) };
        static OFFSET: Cell<usize> = const { Cell::new(144) };
        static ONLINE: Cell<u8> = const { Cell::new(0) };
        static CURRENT_MANAGER: Cell<usize> = const { Cell::new(MANAGER) };
        static CURRENT_ITEM: Cell<usize> = const { Cell::new(ITEM) };
        static LIVE: Cell<usize> = const { Cell::new(1) };
        static VALUE: Cell<i32> = const { Cell::new(0) };
    }
    fn token<T>(p: usize) -> *mut T {
        std::ptr::without_provenance_mut(p)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(1)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(2)
    }
    unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
        panic!("already attached")
    }
    unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
        panic!("borrowed")
    }
    unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        ROOTS.set(ROOTS.get() + 1);
        Box::into_raw(Box::new(object)).cast()
    }
    unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
        // SAFETY: authored handles contain one live pointer slot until root_free.
        unsafe { handle.cast::<*mut raw::Object>().read() }
    }
    unsafe extern "C-unwind" fn root_free(handle: Ptr) {
        // SAFETY: exactly one release of each authored Box-backed handle.
        unsafe { drop(Box::from_raw(handle.cast::<*mut raw::Object>())) };
        ROOTS.set(ROOTS.get() - 1);
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        token(3)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, ns: *const c_char, name: *const c_char) -> Class {
        // SAFETY: adapter supplies static C metadata names.
        unsafe {
            assert_eq!(CStr::from_ptr(ns), c"System");
            assert_eq!(CStr::from_ptr(name), c"Int32");
        }
        token(INT32)
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        assert_eq!(object as usize, ITEM);
        assert_eq!(ROOTS.get(), 2);
        token(OWNER)
    }
    unsafe extern "C-unwind" fn class_field(owner: Class, name: *const c_char) -> Ptr {
        assert_eq!(owner as usize, OWNER);
        // SAFETY: adapter supplies a static field name.
        unsafe {
            assert_eq!(CStr::from_ptr(name), c"<LvlReq>k__BackingField");
        }
        token(FIELD)
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> super::super::Type {
        token(7)
    }
    unsafe extern "C-unwind" fn byref(_: super::super::Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn code(_: super::super::Type) -> i32 {
        CODE.get()
    }
    unsafe extern "C-unwind" fn type_class(_: super::super::Type) -> Class {
        token(TYPE_CLASS.get())
    }
    unsafe extern "C-unwind" fn parent(_: Ptr) -> Class {
        token(DECLARING.get())
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        OFFSET.get()
    }
    unsafe extern "C-unwind" fn static_get(field: Ptr, dest: Ptr) {
        // SAFETY: these exact validated fake fields have the destination widths supplied here.
        unsafe {
            match field as usize {
                ONLINE_FIELD => dest.cast::<u8>().write(ONLINE.get()),
                MANAGER_FIELD => dest.cast::<Ptr>().write(token(CURRENT_MANAGER.get())),
                _ => panic!("unexpected static field"),
            }
        }
    }
    unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, field: Ptr, dest: Ptr) {
        assert_eq!(ROOTS.get(), 2);
        assert_eq!(object as usize, MANAGER);
        // SAFETY: fake validated native pointer/reference fields have these exact widths.
        unsafe {
            match field as usize {
                CACHED_FIELD => dest.cast::<usize>().write(LIVE.get()),
                LINK_FIELD => dest.cast::<Ptr>().write(token(CURRENT_ITEM.get())),
                _ => panic!("unexpected field"),
            }
        }
    }
    unsafe extern "C-unwind" fn set(object: *mut raw::Object, field: *mut raw::FieldInfo, source: Ptr) {
        assert_eq!(object as usize, ITEM);
        assert_eq!(field as usize, FIELD);
        assert_eq!(ROOTS.get(), 2);
        STORES.set(STORES.get() + 1);
        // SAFETY: set_i32 supplies one live aligned Int32 source slot, read only during this call.
        VALUE.set(unsafe { source.cast::<i32>().read() });
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        assert_eq!(ROOTS.get(), 0);
        STORES.set(0);
        FLAGS.set(0);
        BYREF.set(false);
        CODE.set(8);
        TYPE_CLASS.set(INT32);
        DECLARING.set(OWNER);
        OFFSET.set(144);
        ONLINE.set(0);
        CURRENT_MANAGER.set(MANAGER);
        CURRENT_ITEM.set(ITEM);
        LIVE.set(1);
        VALUE.set(0);
        let mut a = RuntimeAccess::test_stubs();
        a.corlib = corlib;
        a.class_from_name = class_from_name;
        a.object_class = object_class;
        a.class_field = class_field;
        a.field_type = field_type;
        a.type_byref = byref;
        a.type_code = code;
        a.type_class = type_class;
        a.field_parent = parent;
        a.field_flags = flags;
        a.field_offset = offset;
        a.static_get = static_get;
        a.field_get = field_get;
        // SAFETY: authored APIs implement one already-attached test thread and owned Box handles.
        let ctx = unsafe {
            ThreadContext::new(raw::Api {
                domain_get: domain,
                thread_current: current,
                thread_attach: attach,
                thread_detach: detach,
                gchandle_new: root,
                gchandle_free: root_free,
                gchandle_get_target: root_target,
            })
        };
        (a, ctx)
    }
    unsafe fn graph<'a, 'r>(attached: &'a AttachedThread<'r>) -> ForgeGraph<'a, 'r> {
        // SAFETY: both identity tokens are live authored objects recognized by these fake APIs.
        unsafe {
            ForgeGraph {
                images: super::super::Images { all: vec![], game: token(3) },
                manager: attached.root(NonNull::new(token(MANAGER)).unwrap()).unwrap(),
                item: attached.root(NonNull::new(token(ITEM)).unwrap()).unwrap(),
                online_field: token(ONLINE_FIELD),
                manager_field: token(MANAGER_FIELD),
                cached_ptr_field: token(CACHED_FIELD),
                links: vec![(token(MANAGER), token(LINK_FIELD), token(ITEM))],
                _parents: vec![],
            }
        }
    }
    #[test]
    fn exact_field_metadata_rejects_wrong_width_flags_owner_and_position_before_any_store() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        // SAFETY: authored immutable graph and metadata; no real object memory is accessed.
        let g = unsafe { graph(&attached) };
        // SAFETY: same authored contract throughout this metadata-only validation matrix.
        unsafe {
            assert!(a.level_field(&g, token(OWNER)).is_ok());
            for flag in [0x10, 0x20, 0x40, 0x70] {
                FLAGS.set(flag);
                assert!(a.level_field(&g, token(OWNER)).is_err());
            }
            FLAGS.set(0);
            TYPE_CLASS.set(INT32 + 1);
            assert!(a.level_field(&g, token(OWNER)).is_err());
            TYPE_CLASS.set(INT32);
            CODE.set(0x0a);
            assert!(a.level_field(&g, token(OWNER)).is_err());
            CODE.set(8);
            BYREF.set(true);
            assert!(a.level_field(&g, token(OWNER)).is_err());
            BYREF.set(false);
            DECLARING.set(OWNER + 1);
            assert!(a.level_field(&g, token(OWNER)).is_err());
            DECLARING.set(OWNER);
            for p in [0, 15, 16385, usize::MAX] {
                OFFSET.set(p);
                assert!(a.level_field(&g, token(OWNER)).is_err());
            }
        }
        assert_eq!(STORES.get(), 0);
        drop(g);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn last_gate_refuses_changed_manager_item_online_or_destroyed_wrapper_without_store() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        // SAFETY: authored graph and exact fake field widths; all parent roots stay held.
        let g = unsafe { graph(&attached) };
        // SAFETY: same authored contract for all deliberately changed identity/gate values.
        unsafe {
            assert!(a.level_final_gate(&g, MANAGER as u64).is_ok());
            assert!(matches!(a.level_final_gate(&g, u64::MAX), Err(LevelWriteError::Stale(_))));
            CURRENT_MANAGER.set(123);
            assert!(matches!(a.level_final_gate(&g, MANAGER as u64), Err(LevelWriteError::Stale(_))));
            CURRENT_MANAGER.set(MANAGER);
            CURRENT_ITEM.set(321);
            assert!(matches!(a.level_final_gate(&g, MANAGER as u64), Err(LevelWriteError::Stale(_))));
            CURRENT_ITEM.set(ITEM);
            LIVE.set(0);
            assert!(matches!(a.level_final_gate(&g, MANAGER as u64), Err(LevelWriteError::Offline)));
            LIVE.set(1);
            ONLINE.set(1);
            assert!(matches!(a.level_final_gate(&g, MANAGER as u64), Err(LevelWriteError::Offline)));
            ONLINE.set(2);
            assert!(matches!(a.level_final_gate(&g, MANAGER as u64), Err(LevelWriteError::Refused(_))));
        }
        assert_eq!(STORES.get(), 0);
        drop(g);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn every_post_store_failure_is_uncertain_with_roots_retained_and_no_replay() {
        let (_, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        // SAFETY: authored graph roots keep both fake object identities alive through each call.
        let g = unsafe { graph(&attached) };
        for error in [
            LevelWriteError::NotReady("n".into()),
            LevelWriteError::Stale("s".into()),
            LevelWriteError::Refused("r".into()),
            LevelWriteError::Offline,
            LevelWriteError::Failed("f".into()),
        ] {
            let prior = STORES.get();
            // SAFETY: exact authored field ABI/Int32 destination, with manager+item roots retained.
            let result: Result<(), _> = unsafe {
                attempt_store(
                    raw::FieldWriteApi { field_set_value: set },
                    target(&g.item).unwrap(),
                    NonNull::new(token(FIELD)).unwrap(),
                    37,
                    || Err(error),
                )
            };
            assert!(matches!(result, Err(LevelWriteError::Failed(_))));
            assert_eq!(STORES.get(), prior + 1);
            assert_eq!(VALUE.get(), 37);
        }
        // SAFETY: same validated authored field/root contract; successful readback remains owned.
        let result = unsafe {
            attempt_store(
                raw::FieldWriteApi { field_set_value: set },
                target(&g.item).unwrap(),
                NonNull::new(token(FIELD)).unwrap(),
                255,
                || Ok(VALUE.get()),
            )
        }
        .unwrap();
        assert_eq!(result, 255);
        assert_eq!(STORES.get(), 6);
        drop(g);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn invalid_request_domain_or_backed_rarity_denies_before_runtime_io() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        let mut expected = ForgeSnapshot {
            item: ITEM as u64,
            individual_id: 1,
            fingerprint: 0,
            name: String::new(),
            item_type: 1,
            sub_type: 0,
            rarity: 0,
            class_requirement: 0,
            affixes: vec![],
            implicit_rolls: vec![],
            identity: None,
            status: crate::forge::ItemStatus {
                forging_potential: 0,
                forging_potential_type: 0,
                legendary_potential: 0,
                weavers_will: 0,
                weavers_touch: 0,
                corrupted: false,
                ruined: false,
                level_requirement: 0,
            },
        };
        for (manager, value, rarity) in [
            (0, 1, 0),
            (MANAGER as u64, -1, 0),
            (MANAGER as u64, 256, 0),
            (MANAGER as u64, 1, 7),
            (MANAGER as u64, 1, 8),
            (MANAGER as u64, 1, 9),
        ] {
            let e = &mut expected;
            e.rarity = rarity;
            // SAFETY: invalid requests must return before any unprovided fake metadata export.
            assert!(matches!(
                unsafe { a.set_level_requirement(&attached, raw::FieldWriteApi { field_set_value: set }, manager, e, value) },
                Err(LevelWriteError::Refused(_))
            ));
        }
        assert_eq!(STORES.get(), 0);
        assert_eq!(ROOTS.get(), 0);
    }
}
