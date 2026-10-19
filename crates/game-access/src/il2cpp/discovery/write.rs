//! Conditional status stores, game refresh and rooted full readback.

use super::{
    Class, DiscoveryError, Ptr, RuntimeAccess,
    forge::{ForgeGraph, target},
};
use crate::{
    forge::ForgeSnapshot,
    il2cpp::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, raw},
    write::StatusField,
};
use std::ptr::NonNull;

/// Only `Failed` may describe an operation whose store was attempted. No variant permits retry.
#[derive(Debug)]
pub enum StatusWriteError {
    NotReady(String),
    Stale(String),
    Refused(String),
    Offline,
    Failed(String),
}

impl std::fmt::Display for StatusWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady(s) | Self::Stale(s) | Self::Refused(s) | Self::Failed(s) => f.write_str(s),
            Self::Offline => f.write_str("An offline character with a live item manager is required"),
        }
    }
}
impl std::error::Error for StatusWriteError {}
impl From<DiscoveryError> for StatusWriteError {
    fn from(error: DiscoveryError) -> Self {
        match error {
            DiscoveryError::OfflineRequired => Self::Offline,
            DiscoveryError::NotReady(s) => Self::NotReady(s.into()),
            other => Self::Refused(other.to_string()),
        }
    }
}
fn uncertain(error: impl std::fmt::Display) -> StatusWriteError {
    StatusWriteError::Failed(format!("Status store attempted; readback unconfirmed: {error}"))
}

// The only store boundary. The caller has validated the exact field and retains all graph roots.
unsafe fn attempt_store<T>(
    api: raw::FieldWriteApi,
    object: NonNull<raw::Object>,
    field: NonNull<raw::FieldInfo>,
    value: i32,
    readback: impl FnOnce() -> Result<T, StatusWriteError>,
) -> Result<T, StatusWriteError> {
    // STORE ATTEMPT BOUNDARY: nothing below may return a definite pre-store refusal.
    // SAFETY: the internal caller establishes FieldWriteApi::set_i32's complete contract.
    unsafe { api.set_i32(object, field, value) };
    readback().map_err(uncertain)
}

impl RuntimeAccess {
    // Internal caller supplies set_status's full live-runtime/exclusive-write contract.
    unsafe fn status_field(&self, owner: Class, kind: StatusField) -> Result<NonNull<raw::FieldInfo>, StatusWriteError> {
        unsafe {
            let (name, primitive, code) = match kind {
                StatusField::LevelRequirement => (c"<LvlReq>k__BackingField", c"Int32", 0x08),
                StatusField::ForgingPotential => (c"forgingPotential", c"Byte", 0x05),
                StatusField::Corrupted => (c"corrupted", c"Boolean", 0x02),
                StatusField::Ruined => (c"ruined", c"Boolean", 0x02),
                _ => return Err(StatusWriteError::Refused("This status field is not supported yet".into())),
            };
            let field = self.field(owner, name)?;
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.type_byref)(ty)
                || (self.type_code)(ty) != code
                || (self.type_class)(ty) != self.system_class(primitive)?
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) & 0x70 != 0
                || !(16..=16384).contains(&(self.field_offset)(field))
            {
                return Err(StatusWriteError::Refused("Status field is not the exact mutable instance field".into()));
            }
            NonNull::new(field.cast()).ok_or_else(|| StatusWriteError::Refused("Missing status field".into()))
        }
    }

    // Uses already validated metadata; no new object is dereferenced from the current statics/edges.
    unsafe fn status_final_gate(&self, graph: &ForgeGraph<'_, '_>, manager: u64) -> Result<(), StatusWriteError> {
        unsafe {
            let held = target(&graph.manager)?;
            if held.as_ptr() as u64 != manager {
                return Err(StatusWriteError::Stale("Current manager changed".into()));
            }
            let mut current: Ptr = std::ptr::null_mut();
            (self.static_get)(graph.manager_field, (&mut current as *mut Ptr).cast());
            if current != held.as_ptr().cast() {
                return Err(StatusWriteError::Stale("Current manager changed".into()));
            }
            let mut cached = 0_usize;
            (self.field_get)(held.as_ptr(), graph.cached_ptr_field, (&mut cached as *mut usize).cast());
            if cached == 0 {
                return Err(StatusWriteError::Offline);
            }
            for &(parent, field, expected) in &graph.links {
                let mut child: Ptr = std::ptr::null_mut();
                (self.field_get)(parent.cast(), field, (&mut child as *mut Ptr).cast());
                if child != expected {
                    return Err(StatusWriteError::Stale("Current forge item mapping changed".into()));
                }
            }
            let mut online = 1_u8;
            (self.static_get)(graph.online_field, (&mut online as *mut u8).cast());
            match online {
                0 => Ok(()),
                1 => Err(StatusWriteError::Offline),
                _ => Err(StatusWriteError::Refused("Noncanonical runtime online Boolean".into())),
            }
        }
    }

    /// Change one supported status field and copy the complete result. Level uses a
    /// direct Int32 store; other fields use Byte/Boolean then RebuildID and UpdateEntry.
    ///
    /// # Safety
    /// All `read_forge` runtime/main-thread/nonmoving-GC conditions apply, with exclusive WRITE
    /// access to the entire rooted graph and all relevant statics until completion. The caller
    /// must authorize this exact mutation and exclude callbacks, reentrancy, other writers,
    /// runtime shutdown, native destruction and graph replacement throughout. Both export tables
    /// and attachment must belong to the same initialized source-bound Unity 6000.4 runtime.
    /// `Field::SetValue` must implement the direct, metadata-validated scalar assignment.
    /// For rebuilding edits, `invoke` must describe this same runtime and the
    /// two refresh methods must be callable synchronously; roots survive managed
    /// calls, exceptions and GC. Refresh failures are uncertain and never retried.
    /// Expected addresses are opaque comparisons only. After entering the void setter, failures
    /// are uncertain; this API cannot recover an access violation/SEH or guarantee rollback.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn set_status(
        &self,
        attached: &AttachedThread<'_>,
        write_api: raw::FieldWriteApi,
        invoke: Option<raw::InvocationApi>,
        expected_manager: u64,
        expected: &ForgeSnapshot,
        kind: StatusField,
        value: i32,
    ) -> Result<ForgeSnapshot, StatusWriteError> {
        unsafe {
            if expected_manager == 0
                || !(0..=255).contains(&value)
                || !kind.supported()
                || (matches!(kind, StatusField::LevelRequirement | StatusField::ForgingPotential) && matches!(expected.rarity, 7..=9))
                || (matches!(kind, StatusField::Corrupted | StatusField::Ruined) && value > 1)
            {
                return Err(StatusWriteError::Refused("Unsupported status edit or invalid byte-domain value".into()));
            }
            if kind != StatusField::LevelRequirement && expected.affixes.iter().any(|affix| affix.id > 4095) {
                return Err(StatusWriteError::Refused(
                    "Rebuilding would truncate an affix ID outside the game's 12-bit save format".into(),
                ));
            }
            let limits = self.read_item_limits(attached)?;
            let valid = match kind {
                StatusField::LevelRequirement => !matches!(expected.rarity, 7..=9) && value <= i32::from(limits.level_requirement.maximum),
                StatusField::ForgingPotential => {
                    !matches!(expected.rarity, 7..=9) && (i32::from(limits.forging_potential.minimum)..=i32::from(limits.forging_potential.maximum)).contains(&value)
                }
                StatusField::Corrupted | StatusField::Ruined => matches!(value, 0 | 1),
                _ => false,
            };
            if !valid {
                return Err(StatusWriteError::Refused("Status value or item eligibility is outside the game's rules".into()));
            }
            let graph = self.forge_graph(attached)?.ok_or_else(|| StatusWriteError::NotReady("Forge has no current item".into()))?;
            let owner = self.class(&graph.images, c"", if kind == StatusField::LevelRequirement { c"ItemDataUnpacked" } else { c"ItemData" })?;
            let field = self.status_field(owner, kind)?;
            // Resolve/bind every refresh before the store. Metadata/signature
            // failures here are definite pre-store refusals.
            let context = invoke.map(|api| InvocationContext::new(attached, api));
            let refresh = if kind == StatusField::LevelRequirement {
                None
            } else {
                let context = context.as_ref().ok_or_else(|| StatusWriteError::Refused("Missing refresh invocation API".into()))?;
                let item_data = self.class(&graph.images, c"", c"ItemData")?;
                let entry = self.class(&graph.images, c"", c"ItemContainerEntry")?;
                let rebuild = context
                    .bind(NonNull::new(self.method(item_data, c"RebuildID", &[], true)?.cast_mut()).ok_or_else(|| StatusWriteError::Refused("Missing RebuildID".into()))?)
                    .map_err(|e| StatusWriteError::Refused(e.to_string()))?;
                let update = context
                    .bind(NonNull::new(self.method(entry, c"UpdateEntry", &[], true)?.cast_mut()).ok_or_else(|| StatusWriteError::Refused("Missing UpdateEntry".into()))?)
                    .map_err(|e| StatusWriteError::Refused(e.to_string()))?;
                Some((rebuild, update))
            };
            // Metadata/limit validation has finished. Refresh all current graph edges, retaining
            // both graphs' roots until the operation ends, and compare every owned field.
            let current = self.forge_graph(attached)?.ok_or_else(|| StatusWriteError::Stale("Current item disappeared".into()))?;
            let before = self.forge_snapshot(attached, &current)?;
            if before != *expected || target(&graph.item)? != target(&current.item)? {
                return Err(StatusWriteError::Stale("Complete current item differs from expected".into()));
            }
            let object = target(&current.item)?;
            self.status_final_gate(&current, expected_manager)?;
            let readback = || {
                if let Some((rebuild, update)) = &refresh {
                    let entry = current._parents.last().ok_or_else(|| StatusWriteError::Failed("Missing retained forge entry".into()))?;
                    for (method, root) in [(rebuild, &current.item), (update, entry)] {
                        match method.invoke(Some(root), &mut []).map_err(uncertain)? {
                            InvocationOutcome::Returned(ReturnValue::Void) => {}
                            _ => return Err(StatusWriteError::Failed("Game refresh raised an exception or returned an unexpected value".into())),
                        }
                        self.status_final_gate(&current, expected_manager)?;
                    }
                }
                self.status_final_gate(&current, expected_manager)?;
                let actual = self.forge_snapshot(attached, &current)?;
                self.status_final_gate(&current, expected_manager)?;
                // A valid differing result is known state. The session performs
                // its separate read and decides Confirmed versus NotKept.
                Ok(actual)
            };
            if kind == StatusField::LevelRequirement {
                attempt_store(write_api, object, field, value, readback)
            } else {
                // STORE ATTEMPT BOUNDARY: exact Byte/Boolean field, canonical u8.
                write_api.set_u8(object, field, value as u8);
                readback().map_err(uncertain)
            }
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
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_ok());
            for flag in [0x10, 0x20, 0x40, 0x70] {
                FLAGS.set(flag);
                assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
            }
            FLAGS.set(0);
            TYPE_CLASS.set(INT32 + 1);
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
            TYPE_CLASS.set(INT32);
            CODE.set(0x0a);
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
            CODE.set(8);
            BYREF.set(true);
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
            BYREF.set(false);
            DECLARING.set(OWNER + 1);
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
            DECLARING.set(OWNER);
            for p in [0, 15, 16385, usize::MAX] {
                OFFSET.set(p);
                assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
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
            assert!(a.status_final_gate(&g, MANAGER as u64).is_ok());
            assert!(matches!(a.status_final_gate(&g, u64::MAX), Err(StatusWriteError::Stale(_))));
            CURRENT_MANAGER.set(123);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(StatusWriteError::Stale(_))));
            CURRENT_MANAGER.set(MANAGER);
            CURRENT_ITEM.set(321);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(StatusWriteError::Stale(_))));
            CURRENT_ITEM.set(ITEM);
            LIVE.set(0);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(StatusWriteError::Offline)));
            LIVE.set(1);
            ONLINE.set(1);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(StatusWriteError::Offline)));
            ONLINE.set(2);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(StatusWriteError::Refused(_))));
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
            StatusWriteError::NotReady("n".into()),
            StatusWriteError::Stale("s".into()),
            StatusWriteError::Refused("r".into()),
            StatusWriteError::Offline,
            StatusWriteError::Failed("f".into()),
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
            assert!(matches!(result, Err(StatusWriteError::Failed(_))));
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
                unsafe {
                    a.set_status(
                        &attached,
                        raw::FieldWriteApi { field_set_value: set },
                        None,
                        manager,
                        e,
                        StatusField::LevelRequirement,
                        value,
                    )
                },
                Err(StatusWriteError::Refused(_))
            ));
        }
        assert_eq!(STORES.get(), 0);
        assert_eq!(ROOTS.get(), 0);
    }
}
