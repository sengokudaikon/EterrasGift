//! Conditional Forge stores, game refresh and rooted full readback.

use super::{
    Class, DiscoveryError, Ptr, RuntimeAccess,
    forge::{ForgeGraph, Root, target},
};
use crate::{
    forge::ForgeSnapshot,
    il2cpp::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw},
    write::{ForgeMutation, StatusField},
};
use std::{ffi::CStr, ptr::NonNull};

/// Only `Failed` may describe an operation whose store was attempted. No variant permits retry.
#[derive(Debug)]
pub enum ForgeWriteError {
    NotReady(String),
    Stale(String),
    Refused(String),
    Offline,
    Failed(String),
}

impl std::fmt::Display for ForgeWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady(s) | Self::Stale(s) | Self::Refused(s) | Self::Failed(s) => f.write_str(s),
            Self::Offline => f.write_str("An offline character with a live item manager is required"),
        }
    }
}
impl std::error::Error for ForgeWriteError {}
impl From<DiscoveryError> for ForgeWriteError {
    fn from(error: DiscoveryError) -> Self {
        match error {
            DiscoveryError::OfflineRequired => Self::Offline,
            DiscoveryError::NotReady(s) => Self::NotReady(s.into()),
            other => Self::Refused(other.to_string()),
        }
    }
}
fn uncertain(error: impl std::fmt::Display) -> ForgeWriteError {
    ForgeWriteError::Failed(format!("Forge store attempted; readback unconfirmed: {error}"))
}

// The only store boundary. The caller has validated the exact field and retains all graph roots.
unsafe fn attempt_store<T>(
    api: raw::FieldWriteApi,
    object: NonNull<raw::Object>,
    field: NonNull<raw::FieldInfo>,
    value: i32,
    readback: impl FnOnce() -> Result<T, ForgeWriteError>,
) -> Result<T, ForgeWriteError> {
    // STORE ATTEMPT BOUNDARY: nothing below may return a definite pre-store refusal.
    // SAFETY: the internal caller establishes FieldWriteApi::set_i32's complete contract.
    unsafe { api.set_i32(object, field, value) };
    readback().map_err(uncertain)
}

impl RuntimeAccess {
    // Internal caller supplies edit_forge's full live-runtime/exclusive-write contract.
    unsafe fn status_field(&self, owner: Class, kind: StatusField) -> Result<NonNull<raw::FieldInfo>, ForgeWriteError> {
        unsafe {
            let (name, primitive, code) = match kind {
                StatusField::LevelRequirement => (c"<LvlReq>k__BackingField", c"Int32", 0x08),
                StatusField::ForgingPotential => (c"forgingPotential", c"Byte", 0x05),
                StatusField::LegendaryPotential => (c"legendaryPotential", c"Byte", 0x05),
                StatusField::WeaversWill => (c"weaversWill", c"Byte", 0x05),
                StatusField::WeaversTouch => (c"weaversTouch", c"Byte", 0x05),
                StatusField::Corrupted => (c"corrupted", c"Boolean", 0x02),
                StatusField::Ruined => (c"ruined", c"Boolean", 0x02),
            };
            self.mutable_field(owner, name, primitive, code)
        }
    }
    unsafe fn mutable_field(&self, owner: Class, name: &CStr, primitive: &CStr, code: i32) -> Result<NonNull<raw::FieldInfo>, ForgeWriteError> {
        unsafe {
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
                return Err(ForgeWriteError::Refused("Not the exact mutable instance field".into()));
            }
            NonNull::new(field.cast()).ok_or_else(|| ForgeWriteError::Refused("Missing mutable field".into()))
        }
    }

    // The original helper calls UniqueList.getUnique(UInt16). Validate its exact
    // static signature, root the return across GC, then read only its identity
    // and LegendaryType. Never walk the full catalog during an edit.
    unsafe fn unique_definition<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        graph: &ForgeGraph<'_, '_>,
        api: raw::InvocationApi,
        id: u16,
    ) -> Result<Root<'a, 'r>, ForgeWriteError> {
        unsafe {
            let owner = self.class(&graph.images, c"", c"UniqueList")?;
            let entry = self.catalog_nested(owner, c"Entry")?;
            let mut iterator = std::ptr::null_mut();
            let mut found = None;
            let mut complete = false;
            for _ in 0..100_000 {
                let method = (self.class_methods)(owner, &mut iterator);
                if method.is_null() {
                    complete = true;
                    break;
                }
                let name = (self.method_name)(method);
                if name.is_null() {
                    return Err(ForgeWriteError::Refused("Missing unique lookup method name".into()));
                }
                if CStr::from_ptr(name) != c"getUnique" || (self.method_params)(method) != 1 {
                    continue;
                }
                let parameter = (self.method_param)(method, 0);
                let returns = (self.method_return)(method);
                if found.is_some()
                    || (self.method_instance)(method)
                    || (api.method_get_class)(method) != owner
                    || parameter.is_null()
                    || returns.is_null()
                    || (self.type_byref)(parameter)
                    || (self.type_byref)(returns)
                    || (self.type_code)(parameter) != 0x07
                    || (self.type_class)(parameter) != self.system_class(c"UInt16")?
                    || (self.type_code)(returns) != 0x12
                    || (self.type_class)(returns) != entry
                {
                    return Err(ForgeWriteError::Refused("Unique lookup signature differs from getUnique(UInt16) -> Entry".into()));
                }
                found = NonNull::new(method.cast_mut());
            }
            if !complete {
                return Err(ForgeWriteError::Refused("Unique lookup method enumeration limit".into()));
            }
            let method = found.ok_or_else(|| ForgeWriteError::Refused("Missing getUnique(UInt16)".into()))?;
            let context = InvocationContext::new(attached, api);
            let method = context.bind(method).map_err(|e| ForgeWriteError::Refused(e.to_string()))?;
            let definition = match method
                .invoke(None, &mut [Argument::Scalar(Scalar::U16(id))])
                .map_err(|e| ForgeWriteError::Refused(e.to_string()))?
            {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => root,
                InvocationOutcome::Returned(ReturnValue::Reference(None)) => return Err(ForgeWriteError::NotReady("Unique definition is unavailable".into())),
                _ => return Err(ForgeWriteError::Refused("Unique lookup threw or returned an unexpected value".into())),
            };
            self.exact(&definition, entry)?;
            if self.primitive::<u16>(&definition, c"uniqueID")? != id {
                return Err(ForgeWriteError::Refused("Unique lookup returned a different identity".into()));
            }
            Ok(definition)
        }
    }

    // Only the base registry used by the original op 11 is acquired. Every
    // returned object and collection is rooted across the managed getter/GC.
    unsafe fn validate_base(
        &self,
        attached: &AttachedThread<'_>,
        graph: &ForgeGraph<'_, '_>,
        api: raw::InvocationApi,
        item_type: u8,
        sub_type: u16,
    ) -> Result<(), ForgeWriteError> {
        unsafe {
            let owner = self.class(&graph.images, c"", c"ItemList")?;
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind(self.zero_arg_method(owner, c"get", owner, false, &api)?)
                .map_err(|e| ForgeWriteError::Refused(e.to_string()))?;
            let registry = match method.invoke(None, &mut []).map_err(|e| ForgeWriteError::Refused(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => root,
                InvocationOutcome::Returned(ReturnValue::Reference(None)) => return Err(ForgeWriteError::NotReady("Base registry is unavailable".into())),
                _ => return Err(ForgeWriteError::Refused("Base registry getter threw or returned an unexpected value".into())),
            };
            self.exact(&registry, owner)?;
            let bases = self
                .catalog_reference(attached, &registry, c"EquippableItems")?
                .ok_or_else(|| ForgeWriteError::NotReady("Base definitions unavailable".into()))?;
            let base_class = self.catalog_nested(owner, c"BaseEquipmentItem")?;
            let sub_class = self.catalog_nested(owner, c"EquipmentItem")?;
            for base in self.catalog_array(attached, &bases, base_class, 256)?.into_iter().flatten() {
                if self.primitive::<i32>(&base, c"baseTypeID")? != i32::from(item_type) {
                    continue;
                }
                let subs = self
                    .catalog_reference(attached, &base, c"subItems")?
                    .ok_or_else(|| ForgeWriteError::NotReady("Subtype definitions unavailable".into()))?;
                for sub in self.catalog_list(attached, &subs, sub_class, 1024)? {
                    if self.primitive::<i32>(&sub, c"subTypeID")? == i32::from(sub_type) {
                        return Ok(());
                    }
                }
                break;
            }
            Err(ForgeWriteError::Refused("Base/subtype is absent from the current registry".into()))
        }
    }

    // Mirrors the original validateDefinition: only already-loaded affix IDs
    // and tier counts are needed, without extracting numeric/display catalogs.
    unsafe fn validate_affix_definition(&self, attached: &AttachedThread<'_>, graph: &ForgeGraph<'_, '_>, id: u16, tier_value: u16) -> Result<(), ForgeWriteError> {
        unsafe {
            if id > 4095 || !(1..=8).contains(&tier_value) {
                return Err(ForgeWriteError::Refused("Invalid serialized affix ID/tier".into()));
            }
            let owner = self.class(&graph.images, c"", c"AffixList")?;
            let globals = self.class(&graph.images, c"LE.AssetManagement", c"GlobalAssets")?;
            let master = self
                .catalog_static(attached, globals, c"_storage_MasterAffixesList", owner)?
                .ok_or_else(|| ForgeWriteError::NotReady("Affix definitions are unavailable".into()))?;
            self.exact(&master, owner)?;
            if !self.catalog_live(attached, &graph.images, &master)? {
                return Err(ForgeWriteError::NotReady("Affix definitions are destroyed".into()));
            }
            let tier_class = self.catalog_nested(owner, c"Tier")?;
            for (field, name) in [(c"singleAffixes", c"SingleAffix"), (c"multiAffixes", c"MultiAffix")] {
                let class = self.catalog_nested(owner, name)?;
                let array = self
                    .catalog_reference(attached, &master, field)?
                    .ok_or_else(|| ForgeWriteError::NotReady("Affix definition array is unavailable".into()))?;
                for definition in self.catalog_array(attached, &array, class, 10000)?.into_iter().flatten() {
                    if self.primitive::<i32>(&definition, c"affixId")? != i32::from(id) {
                        continue;
                    }
                    let tiers = self
                        .catalog_reference(attached, &definition, c"tiers")?
                        .ok_or_else(|| ForgeWriteError::Refused("Affix tiers are unavailable".into()))?;
                    if usize::from(tier_value) > self.catalog_list(attached, &tiers, tier_class, 128)?.len() {
                        return Err(ForgeWriteError::Refused("Tier exceeds current definition".into()));
                    }
                    return Ok(());
                }
            }
            Err(ForgeWriteError::Refused("Affix ID has no definition in this build".into()))
        }
    }

    unsafe fn has_refresh(&self, owner: Class, name: &CStr) -> Result<bool, ForgeWriteError> {
        unsafe {
            let mut iterator = std::ptr::null_mut();
            for _ in 0..100_000 {
                let method = (self.class_methods)(owner, &mut iterator);
                if method.is_null() {
                    return Ok(false);
                }
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(ForgeWriteError::Refused("Missing refresh method name".into()));
                }
                if CStr::from_ptr(actual) == name && (self.method_params)(method) == 0 {
                    return Ok(true);
                }
            }
            Err(ForgeWriteError::Refused("Refresh method enumeration limit".into()))
        }
    }

    // Uses already validated metadata; no new object is dereferenced from the current statics/edges.
    unsafe fn status_final_gate(&self, graph: &ForgeGraph<'_, '_>, manager: u64) -> Result<(), ForgeWriteError> {
        unsafe {
            let held = target(&graph.manager)?;
            if held.as_ptr() as u64 != manager {
                return Err(ForgeWriteError::Stale("Current manager changed".into()));
            }
            let mut current: Ptr = std::ptr::null_mut();
            (self.static_get)(graph.manager_field, (&mut current as *mut Ptr).cast());
            if current != held.as_ptr().cast() {
                return Err(ForgeWriteError::Stale("Current manager changed".into()));
            }
            let mut cached = 0_usize;
            (self.field_get)(held.as_ptr(), graph.cached_ptr_field, (&mut cached as *mut usize).cast());
            if cached == 0 {
                return Err(ForgeWriteError::Offline);
            }
            for &(parent, field, expected) in &graph.links {
                let mut child: Ptr = std::ptr::null_mut();
                (self.field_get)(parent.cast(), field, (&mut child as *mut Ptr).cast());
                if child != expected {
                    return Err(ForgeWriteError::Stale("Current forge item mapping changed".into()));
                }
            }
            let mut online = 1_u8;
            (self.static_get)(graph.online_field, (&mut online as *mut u8).cast());
            match online {
                0 => Ok(()),
                1 => Err(ForgeWriteError::Offline),
                _ => Err(ForgeWriteError::Refused("Noncanonical runtime online Boolean".into())),
            }
        }
    }

    /// Apply a supported Forge mutation and copy the complete result. Level uses a
    /// direct Int32 store; rebuilding edits call the game refresh methods.
    ///
    /// # Safety
    /// All `read_forge` runtime/main-thread/nonmoving-GC conditions apply, with exclusive WRITE
    /// access to the entire rooted graph and all relevant statics until completion. The caller
    /// must authorize this exact mutation and exclude callbacks, reentrancy, other writers,
    /// runtime shutdown, native destruction and graph replacement throughout. Both export tables
    /// and attachment must belong to the same initialized source-bound Unity 6000.4 runtime.
    /// `Field::SetValue` must implement the direct, metadata-validated scalar assignment.
    /// For rebuilding edits, `invoke` must describe this same runtime and the
    /// refresh methods must be callable synchronously; roots survive managed
    /// calls, exceptions and GC. Refresh failures are uncertain and never retried.
    /// Unique eligibility additionally calls getUnique(UInt16) before the store;
    /// its result must be valid game definition data in this runtime.
    /// Expected addresses are opaque comparisons only. After entering the void setter, failures
    /// are uncertain; this API cannot recover an access violation/SEH or guarantee rollback.
    pub unsafe fn edit_forge(
        &self,
        attached: &AttachedThread<'_>,
        write_api: raw::FieldWriteApi,
        invoke: Option<raw::InvocationApi>,
        expected_manager: u64,
        expected: &ForgeSnapshot,
        mutation: ForgeMutation,
    ) -> Result<ForgeSnapshot, ForgeWriteError> {
        unsafe {
            let level = matches!(
                mutation,
                ForgeMutation::Status {
                    field: StatusField::LevelRequirement,
                    ..
                }
            );
            if expected_manager == 0 {
                return Err(ForgeWriteError::Refused("Missing expected manager".into()));
            }
            if let ForgeMutation::Status { field, value, .. } = mutation
                && ((matches!(field, StatusField::LevelRequirement | StatusField::ForgingPotential) && matches!(expected.rarity, 7..=9))
                    || (matches!(field, StatusField::Corrupted | StatusField::Ruined) && value > 1))
            {
                return Err(ForgeWriteError::Refused("Unsupported status edit".into()));
            }
            if !level && expected.affixes.iter().any(|affix| affix.id > 4095) {
                return Err(ForgeWriteError::Refused(
                    "Rebuilding would truncate an affix ID outside the game's 12-bit save format".into(),
                ));
            }
            let graph = self.forge_graph(attached)?.ok_or_else(|| ForgeWriteError::NotReady("Forge has no current item".into()))?;
            self.status_final_gate(&graph, expected_manager)?;
            if self.forge_snapshot(attached, &graph)? != *expected {
                return Err(ForgeWriteError::Stale("Complete current item differs before definition lookup".into()));
            }
            let item_data = self.class(&graph.images, c"", c"ItemData")?;
            let unpacked = self.class(&graph.images, c"", c"ItemDataUnpacked")?;
            // All destinations are validated before the first scalar store.
            let mut stores = Vec::new();
            // Keep every original affix rooted even if a later refresh replaces
            // its list, clears it or throws. Every batch destination is checked first.
            let mut affix_roots = Vec::new();
            let mut affix_stores = Vec::new();
            match mutation {
                ForgeMutation::AffixRolls { rolls, .. } => {
                    let class = self.class(&graph.images, c"", c"ItemAffix")?;
                    let list = self
                        .catalog_reference(attached, &graph.item, c"affixes")?
                        .ok_or_else(|| ForgeWriteError::Refused("Missing affix list".into()))?;
                    affix_roots = self.catalog_list(attached, &list, class, 32)?;
                    let field = self.mutable_field(class, c"affixRoll", c"Byte", 0x05)?;
                    for change in rolls.changes() {
                        let index = usize::from(change.slot);
                        let before = expected.affixes.get(index).ok_or_else(|| ForgeWriteError::Refused("Invalid affix slot".into()))?;
                        let affix = affix_roots.get(index).ok_or_else(|| ForgeWriteError::Stale("Affix slot disappeared".into()))?;
                        self.exact(affix, class)?;
                        self.validate_affix_definition(attached, &graph, before.id, before.tier)?;
                        affix_stores.push((target(affix)?, field, change.value));
                    }
                }
                ForgeMutation::Status { field, value, unrestricted } => {
                    let limits = self.read_item_limits(attached)?;
                    let legendary_type = if field.requires_definition(expected) {
                        let id = expected.identity.as_ref().ok_or_else(|| ForgeWriteError::Refused("Unique identity missing".into()))?.id;
                        let definition = self.unique_definition(attached, &graph, invoke.ok_or_else(|| ForgeWriteError::Refused("Missing invocation API".into()))?, id)?;
                        Some(self.enumeration(&definition, c"legendaryType", c"LegendaryType")?)
                    } else {
                        None
                    };
                    if !field.allows(expected, &limits, i32::from(value), unrestricted, legendary_type) {
                        return Err(ForgeWriteError::Refused("Status value or item eligibility is outside the game's rules".into()));
                    }
                    stores.push((
                        self.status_field(if level { unpacked } else { item_data }, field)?,
                        if level { Scalar::I32(i32::from(value)) } else { Scalar::U8(value) },
                    ));
                }
                ForgeMutation::Base {
                    item_type,
                    sub_type,
                    unrestricted,
                } => {
                    let changed_type = item_type != expected.item_type;
                    if matches!(expected.rarity, 7..=9)
                        || (!changed_type && sub_type == expected.sub_type)
                        || (changed_type && (!unrestricted || !(25..=33).contains(&item_type) || !(25..=33).contains(&expected.item_type)))
                    {
                        return Err(ForgeWriteError::Refused("Base changes require a non-unique item and compatible type".into()));
                    }
                    self.validate_base(
                        attached,
                        &graph,
                        invoke.ok_or_else(|| ForgeWriteError::Refused("Missing invocation API".into()))?,
                        item_type,
                        sub_type,
                    )?;
                    stores.push((self.mutable_field(item_data, c"subType", c"UInt16", 0x07)?, Scalar::U16(sub_type)));
                    if changed_type {
                        stores.push((self.mutable_field(item_data, c"itemType", c"Byte", 0x05)?, Scalar::U8(item_type)));
                    }
                }
                ForgeMutation::Identity {
                    id,
                    sub_type,
                    rarity,
                    unrestricted,
                } => {
                    if !matches!(expected.rarity, 7..=9) || expected.identity.as_ref().is_none_or(|old| old.id == id) || !matches!(rarity, 7..=9) {
                        return Err(ForgeWriteError::Refused("Identity changes require a different unique/set identity".into()));
                    }
                    let definition = self.unique_definition(attached, &graph, invoke.ok_or_else(|| ForgeWriteError::Refused("Missing invocation API".into()))?, id)?;
                    let subs = self
                        .catalog_reference(attached, &definition, c"subTypes")?
                        .ok_or_else(|| ForgeWriteError::Refused("Missing unique subtypes".into()))?;
                    let subtypes = self.catalog_values::<u8>(attached, &subs, self.system_class(c"Byte")?, 256)?;
                    let set = self.boolean(&definition, c"isSetItem")?;
                    let legendary = self.boolean(&definition, c"canDropAsLegendary")?;
                    let legendary_type = self.enumeration(&definition, c"legendaryType", c"LegendaryType")?;
                    let computed = if set {
                        8
                    } else if !expected.affixes.is_empty() || expected.rarity == 9 {
                        9
                    } else {
                        7
                    };
                    let potential_fits = if set {
                        expected.status.legendary_potential == 0 && expected.status.weavers_will == 0
                    } else if legendary_type == 1 {
                        expected.status.legendary_potential == 0
                    } else {
                        expected.status.weavers_will == 0
                    };
                    if self.primitive::<u8>(&definition, c"baseType")? != expected.item_type
                        || !subtypes.contains(&sub_type)
                        || rarity != computed
                        || (set && !expected.affixes.is_empty())
                        || (!unrestricted && ((rarity == 9 && !legendary) || !potential_fits))
                    {
                        return Err(ForgeWriteError::Refused("Identity is incompatible with the current item/definition".into()));
                    }
                    stores.push((self.mutable_field(item_data, c"uniqueID", c"UInt16", 0x07)?, Scalar::U16(id)));
                    stores.push((self.mutable_field(item_data, c"subType", c"UInt16", 0x07)?, Scalar::U16(u16::from(sub_type))));
                    stores.push((self.mutable_field(item_data, c"rarity", c"Byte", 0x05)?, Scalar::U8(rarity)));
                }
            }
            let context = invoke.map(|api| InvocationContext::new(attached, api));
            let mut refresh = Vec::new();
            if !level {
                let context = context.as_ref().ok_or_else(|| ForgeWriteError::Refused("Missing refresh invocation API".into()))?;
                let mut methods = Vec::new();
                if matches!(mutation, ForgeMutation::Base { .. } | ForgeMutation::Identity { .. }) {
                    methods.push((unpacked, c"AfterIDChange", false));
                    if matches!(mutation, ForgeMutation::Identity { .. }) && self.has_refresh(unpacked, c"MakeFullName")? {
                        methods.push((unpacked, c"MakeFullName", false));
                    }
                }
                methods.push((item_data, c"RebuildID", false));
                methods.push((self.class(&graph.images, c"", c"ItemContainerEntry")?, c"UpdateEntry", true));
                for (owner, name, entry) in methods {
                    let method = context
                        .bind(NonNull::new(self.method(owner, name, &[], true)?.cast_mut()).ok_or_else(|| ForgeWriteError::Refused("Missing refresh method".into()))?)
                        .map_err(|e| ForgeWriteError::Refused(e.to_string()))?;
                    refresh.push((method, entry));
                }
            }
            // Metadata/limit validation has finished. Refresh all current graph edges, retaining
            // both graphs' roots until the operation ends, and compare every owned field.
            let current = self.forge_graph(attached)?.ok_or_else(|| ForgeWriteError::Stale("Current item disappeared".into()))?;
            let before = self.forge_snapshot(attached, &current)?;
            if before != *expected || target(&graph.item)? != target(&current.item)? {
                return Err(ForgeWriteError::Stale("Complete current item differs from expected".into()));
            }
            let object = target(&current.item)?;
            self.status_final_gate(&current, expected_manager)?;
            let readback = || {
                for (method, is_entry) in &refresh {
                    let root = if *is_entry {
                        current._parents.last().ok_or_else(|| ForgeWriteError::Failed("Missing retained forge entry".into()))?
                    } else {
                        &current.item
                    };
                    match method.invoke(Some(root), &mut []).map_err(uncertain)? {
                        InvocationOutcome::Returned(ReturnValue::Void) => {}
                        _ => return Err(ForgeWriteError::Failed("Game refresh raised an exception or returned an unexpected value".into())),
                    }
                    self.status_final_gate(&current, expected_manager)?;
                }
                self.status_final_gate(&current, expected_manager)?;
                let actual = self.forge_snapshot(attached, &current)?;
                self.status_final_gate(&current, expected_manager)?;
                // A valid differing result is known state. The session performs
                // its separate read and decides Confirmed versus NotKept.
                Ok(actual)
            };
            if let [(field, Scalar::I32(value))] = stores.as_slice() {
                attempt_store(write_api, object, *field, *value, readback)
            } else {
                // STORE ATTEMPT BOUNDARY: every later failure is uncertain.
                for (field, value) in stores {
                    match value {
                        Scalar::U8(value) => write_api.set_u8(object, field, value),
                        Scalar::U16(value) => write_api.set_u16(object, field, value),
                        _ => unreachable!("validated Forge scalar destination"),
                    }
                }
                for (affix, field, value) in affix_stores {
                    write_api.set_u8(affix, field, value);
                }
                let result = readback().map_err(uncertain);
                drop(affix_roots);
                result
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
            assert!(matches!(a.status_final_gate(&g, u64::MAX), Err(ForgeWriteError::Stale(_))));
            CURRENT_MANAGER.set(123);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(ForgeWriteError::Stale(_))));
            CURRENT_MANAGER.set(MANAGER);
            CURRENT_ITEM.set(321);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(ForgeWriteError::Stale(_))));
            CURRENT_ITEM.set(ITEM);
            LIVE.set(0);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(ForgeWriteError::Offline)));
            LIVE.set(1);
            ONLINE.set(1);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(ForgeWriteError::Offline)));
            ONLINE.set(2);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(ForgeWriteError::Refused(_))));
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
            ForgeWriteError::NotReady("n".into()),
            ForgeWriteError::Stale("s".into()),
            ForgeWriteError::Refused("r".into()),
            ForgeWriteError::Offline,
            ForgeWriteError::Failed("f".into()),
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
            assert!(matches!(result, Err(ForgeWriteError::Failed(_))));
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
        for (manager, value, rarity) in [(0, 1, 0), (MANAGER as u64, 1, 7), (MANAGER as u64, 1, 8), (MANAGER as u64, 1, 9)] {
            let e = &mut expected;
            e.rarity = rarity;
            // SAFETY: invalid requests must return before any unprovided fake metadata export.
            assert!(matches!(
                unsafe {
                    a.edit_forge(
                        &attached,
                        raw::FieldWriteApi { field_set_value: set },
                        None,
                        manager,
                        e,
                        ForgeMutation::Status {
                            field: StatusField::LevelRequirement,
                            value: value as u8,
                            unrestricted: false,
                        },
                    )
                },
                Err(ForgeWriteError::Refused(_))
            ));
        }
        assert_eq!(STORES.get(), 0);
        assert_eq!(ROOTS.get(), 0);
    }
}
