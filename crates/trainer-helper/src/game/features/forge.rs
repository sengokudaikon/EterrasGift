//! Forge edits on the held item: each is checked against the game's own definitions and rules, written through the
//! store boundary, refreshed by the game's own methods and read back whole.

mod affix;

use crate::game::{
    Class, Ptr, Root, RuntimeAccess,
    accessors::item::ForgeGraph,
    apply::{WriteError, stored, uncertain},
    array_data, metadata_text, target,
};
use eg_trainer_wire::{
    forge::ForgeSnapshot,
    write::{AffixRolls, ForgeMutation, RollArray, StatusField},
};
use il2cpp_runtime::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use std::{ffi::CStr, ptr::NonNull};

impl RuntimeAccess {
    // Internal caller supplies edit_forge's full live-runtime/exclusive-write contract.
    unsafe fn status_field(&self, owner: Class, kind: StatusField) -> Result<NonNull<raw::FieldInfo>, WriteError> {
        // SAFETY: The owner is retained metadata; the selected status kind fixes the required primitive class and storage width.
        unsafe {
            let (name, primitive, code) = match kind {
                StatusField::LevelRequirement => (c"<LvlReq>k__BackingField", c"Int32", raw::type_code::I4),
                StatusField::ForgingPotential => (c"forgingPotential", c"Byte", raw::type_code::U1),
                StatusField::LegendaryPotential => (c"legendaryPotential", c"Byte", raw::type_code::U1),
                StatusField::WeaversWill => (c"weaversWill", c"Byte", raw::type_code::U1),
                StatusField::WeaversTouch => (c"weaversTouch", c"Byte", raw::type_code::U1),
                StatusField::Corrupted => (c"corrupted", c"Boolean", raw::type_code::BOOLEAN),
                StatusField::Ruined => (c"ruined", c"Boolean", raw::type_code::BOOLEAN),
            };
            self.mutable_field(owner, name, primitive, code)
        }
    }
    // Look up one definition through UniqueList.getUnique(UInt16): validate its exact
    // static signature, root the return across GC, then read only its identity
    // and LegendaryType. Never walk the full catalog during an edit.
    pub(crate) unsafe fn unique_definition<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        graph: &ForgeGraph<'_, '_>,
        api: raw::InvocationApi,
        id: u16,
    ) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: The registry metadata and exact lookup signature are checked; the returned definition is rooted before identity validation.
        unsafe {
            let owner = self.class(&graph.images, c"", c"UniqueList")?;
            let entry = self.nested(owner, c"Entry")?;
            let mut found = None;
            for method in self.methods(owner) {
                let method = method?;
                let name = (self.method_name)(method);
                if name.is_null() {
                    return Err(WriteError::Refused("Missing unique lookup method name".into()));
                }
                if metadata_text(name) != c"getUnique" || (self.method_params)(method) != 1 {
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
                    || (self.type_code)(parameter) != raw::type_code::U2
                    || (self.type_class)(parameter) != self.system_class(c"UInt16")?
                    || (self.type_code)(returns) != raw::type_code::CLASS
                    || (self.type_class)(returns) != entry
                {
                    return Err(WriteError::Refused("Unique lookup signature differs from getUnique(UInt16) -> Entry".into()));
                }
                found = NonNull::new(method.cast_mut());
            }
            let method = found.ok_or_else(|| WriteError::Refused("Missing getUnique(UInt16)".into()))?;
            let context = InvocationContext::new(attached, api);
            let method = context.bind(method).map_err(|e| WriteError::Refused(e.to_string()))?;
            let definition = match method
                .invoke(None, &mut [Argument::Scalar(Scalar::U16(id))])
                .map_err(|e| WriteError::Refused(e.to_string()))?
            {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => root,
                InvocationOutcome::Returned(ReturnValue::Reference(None)) => return Err(WriteError::NotReady("Unique definition is unavailable".into())),
                _ => return Err(WriteError::Refused("Unique lookup threw or returned an unexpected value".into())),
            };
            self.exact(&definition, entry)?;
            if self.primitive::<u16>(&definition, c"uniqueID")? != id {
                return Err(WriteError::Refused("Unique lookup returned a different identity".into()));
            }
            Ok(definition)
        }
    }

    // Only the base registry (ItemList.get) is acquired. Every
    // returned object and collection is rooted across the managed getter/GC.
    unsafe fn validate_base(&self, attached: &AttachedThread<'_>, graph: &ForgeGraph<'_, '_>, api: raw::InvocationApi, item_type: u8, sub_type: u16) -> Result<(), WriteError> {
        // SAFETY: The selected graph and registry entries remain rooted while exact destination identity and compatibility are checked.
        unsafe {
            let owner = self.class(&graph.images, c"", c"ItemList")?;
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind(self.zero_arg_method(owner, c"get", owner, false, &api)?)
                .map_err(|e| WriteError::Refused(e.to_string()))?;
            let registry = match method.invoke(None, &mut []).map_err(|e| WriteError::Refused(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => root,
                InvocationOutcome::Returned(ReturnValue::Reference(None)) => return Err(WriteError::NotReady("Base registry is unavailable".into())),
                _ => return Err(WriteError::Refused("Base registry getter threw or returned an unexpected value".into())),
            };
            self.exact(&registry, owner)?;
            let bases = self
                .child(attached, &registry, c"EquippableItems")?
                .ok_or_else(|| WriteError::NotReady("Base definitions unavailable".into()))?;
            let base_class = self.nested(owner, c"BaseEquipmentItem")?;
            let sub_class = self.nested(owner, c"EquipmentItem")?;
            for base in self.array(attached, &bases, base_class, 256)?.into_iter().flatten() {
                if self.primitive::<i32>(&base, c"baseTypeID")? != i32::from(item_type) {
                    continue;
                }
                let subs = self
                    .child(attached, &base, c"subItems")?
                    .ok_or_else(|| WriteError::NotReady("Subtype definitions unavailable".into()))?;
                for sub in self.list(attached, &subs, sub_class, 1024)? {
                    if self.primitive::<i32>(&sub, c"subTypeID")? == i32::from(sub_type) {
                        return Ok(());
                    }
                }
                break;
            }
            Err(WriteError::Refused("Base/subtype is absent from the current registry".into()))
        }
    }

    // Only already-loaded affix IDs and tier counts are checked, without
    // extracting numeric/display catalogs.
    pub(crate) unsafe fn validate_affix_definition(&self, attached: &AttachedThread<'_>, graph: &ForgeGraph<'_, '_>, id: u16, tier_value: u16) -> Result<(), WriteError> {
        // SAFETY: The selected graph and affix definition stay rooted during exact type, slot and compatibility validation.
        unsafe {
            if id > 4095 || !(1..=8).contains(&tier_value) {
                return Err(WriteError::Refused("Invalid serialized affix ID/tier".into()));
            }
            let owner = self.class(&graph.images, c"", c"AffixList")?;
            let globals = self.class(&graph.images, c"LE.AssetManagement", c"GlobalAssets")?;
            let master = self
                .static_object(attached, globals, c"_storage_MasterAffixesList", owner)?
                .ok_or_else(|| WriteError::NotReady("Affix definitions are unavailable".into()))?;
            self.exact(&master, owner)?;
            if !self.is_live(attached, &graph.images, &master)? {
                return Err(WriteError::NotReady("Affix definitions are destroyed".into()));
            }
            let tier_class = self.nested(owner, c"Tier")?;
            for (field, name) in [(c"singleAffixes", c"SingleAffix"), (c"multiAffixes", c"MultiAffix")] {
                let class = self.nested(owner, name)?;
                let array = self
                    .child(attached, &master, field)?
                    .ok_or_else(|| WriteError::NotReady("Affix definition array is unavailable".into()))?;
                for definition in self.array(attached, &array, class, 10000)?.into_iter().flatten() {
                    if self.primitive::<i32>(&definition, c"affixId")? != i32::from(id) {
                        continue;
                    }
                    let tiers = self
                        .child(attached, &definition, c"tiers")?
                        .ok_or_else(|| WriteError::Refused("Affix tiers are unavailable".into()))?;
                    if usize::from(tier_value) > self.list(attached, &tiers, tier_class, 128)?.len() {
                        return Err(WriteError::Refused("Tier exceeds current definition".into()));
                    }
                    return Ok(());
                }
            }
            Err(WriteError::Refused("Affix ID has no definition in this build".into()))
        }
    }

    /// One of the item's own zero-argument refreshes. Some are overrides (`ItemDataUnpacked.AfterIDChange` overrides
    /// `ItemData`'s): those are resolved for the item's class, as the game's own call dispatches, and the body bound is
    /// the game's.
    // Internal caller supplies edit_forge's full live-runtime/exclusive-write contract.
    unsafe fn item_refresh(&self, api: raw::InvocationApi, graph: &ForgeGraph<'_, '_>, owner: Class, name: &CStr) -> Result<crate::game::Method, WriteError> {
        // SAFETY: The signature is checked against retained metadata; the item stays rooted while its override is resolved.
        unsafe {
            let mut method = self.description_method(api, owner, name, &[], Some(self.system_class(c"Void")?), true)?;
            if (api.method_get_flags)(method, std::ptr::null_mut()) & raw::METHOD_ATTRIBUTE_VIRTUAL != 0 {
                method = (api.object_get_virtual_method)(target(&graph.item)?.as_ptr(), method);
                if method.is_null() || (self.class_image)((api.method_get_class)(method)) != graph.images.game {
                    return Err(WriteError::Refused(format!("The game's {} override is unavailable", name.to_string_lossy())));
                }
            }
            Ok(method)
        }
    }

    unsafe fn has_refresh(&self, owner: Class, name: &CStr) -> Result<bool, WriteError> {
        // SAFETY: The owner is retained class metadata; bounded method inspection checks signatures without executing refresh.
        unsafe {
            for method in self.methods(owner) {
                let method = method?;
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(WriteError::Refused("Missing refresh method name".into()));
                }
                if metadata_text(actual) == name && (self.method_params)(method) == 0 {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }

    // Uses already validated metadata; no new object is dereferenced from the current statics/edges.
    unsafe fn status_final_gate(&self, graph: &ForgeGraph<'_, '_>, manager: u64) -> Result<(), WriteError> {
        // SAFETY: The caller retains the selected graph; fresh manager/item/native tokens are compared before the first store.
        unsafe {
            let held = target(&graph.manager)?;
            if held.as_ptr() as u64 != manager {
                return Err(WriteError::Stale("Current manager changed".into()));
            }
            let mut current: Ptr = std::ptr::null_mut();
            (self.static_get)(graph.manager_field, (&mut current as *mut Ptr).cast());
            if current != held.as_ptr().cast() {
                return Err(WriteError::Stale("Current manager changed".into()));
            }
            let mut cached = 0_usize;
            (self.field_get)(held.as_ptr(), graph.cached_ptr_field, (&mut cached as *mut usize).cast());
            if cached == 0 {
                return Err(WriteError::Offline);
            }
            for &(parent, field, expected) in &graph.links {
                let mut child: Ptr = std::ptr::null_mut();
                (self.field_get)(parent.cast(), field, (&mut child as *mut Ptr).cast());
                if child != expected {
                    return Err(WriteError::Stale("Current forge item mapping changed".into()));
                }
            }
            let mut online = 1_u8;
            (self.static_get)(graph.online_field, (&mut online as *mut u8).cast());
            match online {
                0 => Ok(()),
                1 => Err(WriteError::Offline),
                _ => Err(WriteError::Refused("Noncanonical runtime online Boolean".into())),
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
    /// Affix construction authorizes object_new, the exact ItemAffix constructor and
    /// closed corlib List<ItemAffix>.Clear/Add. The constructor may allocate/GC and
    /// normalize the new object but must not modify the selected item. Clear/Add
    /// perform managed reference stores with barriers; failure after Clear is uncertain.
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
    ) -> Result<ForgeSnapshot, WriteError> {
        // SAFETY: The caller supplies exclusive main-thread graph access; all destinations are prevalidated and rooted through stores and readback.
        unsafe {
            let level = matches!(
                mutation,
                ForgeMutation::Status {
                    field: StatusField::LevelRequirement,
                    ..
                }
            );
            if expected_manager == 0 {
                return Err(WriteError::Refused("Missing expected manager".into()));
            }
            if let ForgeMutation::Status { field, value, .. } = mutation
                && ((matches!(field, StatusField::LevelRequirement | StatusField::ForgingPotential) && matches!(expected.rarity, 7..=9))
                    || (matches!(field, StatusField::Corrupted | StatusField::Ruined) && value > 1))
            {
                return Err(WriteError::Refused("Unsupported status edit".into()));
            }
            if !level
                && expected
                    .affixes
                    .iter()
                    .any(|affix| affix.id > 4095 && !matches!(mutation,ForgeMutation::RemoveAffix { slot, id, .. } if slot == affix.slot && id == affix.id))
            {
                return Err(WriteError::Refused("Rebuilding would truncate an affix ID outside the game's 12-bit save format".into()));
            }
            let graph = self.forge_graph(attached)?.ok_or_else(|| WriteError::NotReady("Forge has no current item".into()))?;
            self.status_final_gate(&graph, expected_manager)?;
            if self.forge_snapshot(attached, &graph)? != *expected {
                return Err(WriteError::Stale("Complete current item differs before definition lookup".into()));
            }
            let item_data = self.class(&graph.images, c"", c"ItemData")?;
            let unpacked = self.class(&graph.images, c"", c"ItemDataUnpacked")?;
            // All destinations are validated before the first scalar store.
            let mut stores = Vec::new();
            // Keep every original affix rooted even if a later refresh replaces
            // its list, clears it or throws. Every batch destination is checked first.
            let mut affix_roots = Vec::new();
            let mut affix_stores = Vec::new();
            // A unique or implicit roll array stays rooted through the stores and refresh, like the affixes above.
            let mut roll_array = None;
            let mut roll_stores = Vec::new();
            let mut replacement = None;
            match mutation {
                ForgeMutation::Affix { .. } | ForgeMutation::AddAffix { .. } | ForgeMutation::RemoveAffix { .. } => {
                    replacement = Some(self.prepare_affix(
                        attached,
                        &graph,
                        invoke.ok_or_else(|| WriteError::Refused("Missing invocation API".into()))?,
                        expected,
                        mutation,
                    )?);
                    // The game's rarity method covers normal to rare. On unique-backed items these stores do what the game's
                    // crafting does beside the list (`affix_list_outcome`): a unique that gains an affix becomes legendary and
                    // spends its legendary potential, a legendary that loses its last becomes a unique, and a legendary that
                    // holds a corrupted affix is corrupted.
                    if let ForgeMutation::RemoveAffix {
                        restore_potential: Some(potential),
                        ..
                    } = mutation
                        && !self.read_item_limits(attached)?.legendary_potential.contains(potential)
                    {
                        return Err(WriteError::Refused("The legendary potential to restore is outside the game's range".into()));
                    }
                    let mut projected = expected.clone();
                    mutation.project(&mut projected);
                    if matches!(expected.rarity, 7..=9) {
                        if projected.rarity != expected.rarity {
                            stores.push((self.mutable_field(item_data, c"rarity", c"Byte", raw::type_code::U1)?, Scalar::U8(projected.rarity)));
                        }
                        if projected.status.legendary_potential != expected.status.legendary_potential {
                            stores.push((
                                self.status_field(item_data, StatusField::LegendaryPotential)?,
                                Scalar::U8(projected.status.legendary_potential),
                            ));
                        }
                        if projected.status.corrupted != expected.status.corrupted {
                            stores.push((self.status_field(item_data, StatusField::Corrupted)?, Scalar::U8(u8::from(projected.status.corrupted))));
                        }
                    }
                }
                ForgeMutation::AffixRolls { rolls, .. } => {
                    let class = self.class(&graph.images, c"", c"ItemAffix")?;
                    let list = self
                        .child(attached, &graph.item, c"affixes")?
                        .ok_or_else(|| WriteError::Refused("Missing affix list".into()))?;
                    affix_roots = self.list(attached, &list, class, 32)?;
                    let field = self.mutable_field(class, c"affixRoll", c"Byte", raw::type_code::U1)?;
                    for change in rolls.changes() {
                        let index = usize::from(change.slot);
                        let before = expected.affixes.get(index).ok_or_else(|| WriteError::Refused("Invalid affix slot".into()))?;
                        let affix = affix_roots.get(index).ok_or_else(|| WriteError::Stale("Affix slot disappeared".into()))?;
                        self.exact(affix, class)?;
                        self.validate_affix_definition(attached, &graph, before.id, before.tier)?;
                        affix_stores.push((target(affix)?, field, change.value));
                    }
                }
                ForgeMutation::ModifierRolls { array, rolls, .. } => {
                    let api = invoke.ok_or_else(|| WriteError::Refused("Missing invocation API".into()))?;
                    let (storage, length) = self.modifier_rolls(attached, &graph, api, expected, array, rolls)?;
                    let base = array_data(target(&storage)?.as_ptr());
                    for change in rolls.changes() {
                        if usize::from(change.slot) >= length {
                            return Err(WriteError::Stale("Roll array changed".into()));
                        }
                        roll_stores.push((base.add(usize::from(change.slot)), change.value));
                    }
                    roll_array = Some(storage);
                }
                ForgeMutation::Status { field, value, unrestricted } => {
                    let limits = self.read_item_limits(attached)?;
                    let legendary_type = if field.requires_definition(expected) {
                        let id = expected.identity.as_ref().ok_or_else(|| WriteError::Refused("Unique identity missing".into()))?.id;
                        let definition = self.unique_definition(attached, &graph, invoke.ok_or_else(|| WriteError::Refused("Missing invocation API".into()))?, id)?;
                        Some(self.enumeration(&definition, c"legendaryType", c"LegendaryType")?)
                    } else {
                        None
                    };
                    if !field.allows(expected, &limits, i32::from(value), unrestricted, legendary_type) {
                        return Err(WriteError::Refused("Status value or item eligibility is outside the game's rules".into()));
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
                        return Err(WriteError::Refused("Base changes require a non-unique item and compatible type".into()));
                    }
                    self.validate_base(
                        attached,
                        &graph,
                        invoke.ok_or_else(|| WriteError::Refused("Missing invocation API".into()))?,
                        item_type,
                        sub_type,
                    )?;
                    stores.push((self.mutable_field(item_data, c"subType", c"UInt16", raw::type_code::U2)?, Scalar::U16(sub_type)));
                    if changed_type {
                        stores.push((self.mutable_field(item_data, c"itemType", c"Byte", raw::type_code::U1)?, Scalar::U8(item_type)));
                    }
                }
                ForgeMutation::Identity {
                    id,
                    sub_type,
                    rarity,
                    unrestricted,
                } => {
                    if !matches!(expected.rarity, 7..=9) || expected.identity.as_ref().is_none_or(|old| old.id == id) || !matches!(rarity, 7..=9) {
                        return Err(WriteError::Refused("Identity changes require a different unique/set identity".into()));
                    }
                    let definition = self.unique_definition(attached, &graph, invoke.ok_or_else(|| WriteError::Refused("Missing invocation API".into()))?, id)?;
                    let subs = self
                        .child(attached, &definition, c"subTypes")?
                        .ok_or_else(|| WriteError::Refused("Missing unique subtypes".into()))?;
                    let subtypes = self.values::<u8>(attached, &subs, self.system_class(c"Byte")?, 256)?;
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
                        return Err(WriteError::Refused("Identity is incompatible with the current item/definition".into()));
                    }
                    stores.push((self.mutable_field(item_data, c"uniqueID", c"UInt16", raw::type_code::U2)?, Scalar::U16(id)));
                    stores.push((self.mutable_field(item_data, c"subType", c"UInt16", raw::type_code::U2)?, Scalar::U16(u16::from(sub_type))));
                    stores.push((self.mutable_field(item_data, c"rarity", c"Byte", raw::type_code::U1)?, Scalar::U8(rarity)));
                }
            }
            let context = invoke.map(|api| InvocationContext::new(attached, api));
            let mut refresh = Vec::new();
            if !level {
                let context = context.as_ref().ok_or_else(|| WriteError::Refused("Missing refresh invocation API".into()))?;
                let mut methods = Vec::new();
                if matches!(mutation, ForgeMutation::Base { .. } | ForgeMutation::Identity { .. }) {
                    methods.push((unpacked, c"AfterIDChange", false));
                    if matches!(mutation, ForgeMutation::Identity { .. }) && self.has_refresh(unpacked, c"MakeFullName")? {
                        methods.push((unpacked, c"MakeFullName", false));
                    }
                }
                if matches!(mutation, ForgeMutation::AddAffix { .. } | ForgeMutation::RemoveAffix { .. }) && !matches!(expected.rarity, 7..=9) {
                    methods.push((item_data, c"setRarityFromAffixesForNormalMagicOrRareItem", false));
                }
                methods.push((item_data, c"RebuildID", false));
                methods.push((self.class(&graph.images, c"", c"ItemContainerEntry")?, c"UpdateEntry", true));
                for (owner, name, entry) in methods {
                    let method = if entry {
                        self.method(owner, name, &[], true)?
                    } else {
                        self.item_refresh(invoke.expect("checked invocation API"), &graph, owner, name)?
                    };
                    let method = context
                        .bind(NonNull::new(method.cast_mut()).ok_or_else(|| WriteError::Refused("Missing refresh method".into()))?)
                        .map_err(|e| WriteError::Refused(e.to_string()))?;
                    refresh.push((method, entry));
                }
            }
            // Metadata/limit validation has finished. Refresh all current graph edges, retaining
            // both graphs' roots until the operation ends, and compare every owned field.
            let current = self.forge_graph(attached)?.ok_or_else(|| WriteError::Stale("Current item disappeared".into()))?;
            let before = self.forge_snapshot(attached, &current)?;
            if before != *expected || target(&graph.item)? != target(&current.item)? {
                return Err(WriteError::Stale("Complete current item differs from expected".into()));
            }
            let object = target(&current.item)?;
            self.status_final_gate(&current, expected_manager)?;
            let readback = || {
                for (method, is_entry) in &refresh {
                    let root = if *is_entry {
                        current._parents.last().ok_or_else(|| WriteError::Failed("Missing retained forge entry".into()))?
                    } else {
                        &current.item
                    };
                    match method.invoke(Some(root), &mut []).map_err(uncertain)? {
                        InvocationOutcome::Returned(ReturnValue::Void) => {}
                        _ => return Err(WriteError::Failed("Game refresh raised an exception or returned an unexpected value".into())),
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
            let result = stored(
                || {
                    if let Some(replacement) = &replacement {
                        replacement.apply(attached, write_api, invoke.expect("validated invocation API"), object)?;
                    }
                    for (field, value) in stores {
                        match value {
                            Scalar::I32(value) => write_api.set_i32(object, field, value),
                            Scalar::U8(value) => write_api.set_u8(object, field, value),
                            Scalar::U16(value) => write_api.set_u16(object, field, value),
                            _ => unreachable!("validated Forge scalar destination"),
                        }
                    }
                    for (affix, field, value) in affix_stores {
                        write_api.set_u8(affix, field, value);
                    }
                    for (element, value) in roll_stores {
                        il2cpp_runtime::metadata::write_bytes(element, &[value]);
                    }
                    Ok(())
                },
                readback,
            );
            drop(affix_roots);
            drop(roll_array);
            result
        }
    }

    /// Checks every change against the game's own word on the item, and returns the live roll array
    /// (rooted) with its length. A unique roll must be read by an editable line of the game's unique tooltip; an implicit
    /// roll must belong to an implicit whose range is more than one value.
    // Internal caller supplies edit_forge's full live-runtime/exclusive-write contract.
    unsafe fn modifier_rolls<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        graph: &ForgeGraph<'_, '_>,
        api: raw::InvocationApi,
        expected: &ForgeSnapshot,
        array: RollArray,
        rolls: AffixRolls,
    ) -> Result<(Root<'a, 'r>, usize), WriteError> {
        // SAFETY: The graph is rooted by the caller; definitions, tooltip results and the array stay rooted while checked.
        unsafe {
            let (field, length) = match array {
                RollArray::Unique => {
                    let identity = expected
                        .identity
                        .as_ref()
                        .filter(|_| matches!(expected.rarity, 7..=9))
                        .ok_or_else(|| WriteError::Refused("This item has no unique rolls".into()))?;
                    let definition = self.unique_definition(attached, graph, api, identity.id)?;
                    let lines = self.unique_modifiers(attached, api, &graph.images, &graph.item, &definition, identity.rolls.len())?;
                    if rolls
                        .changes()
                        .iter()
                        .any(|change| !lines.iter().any(|line| line.editable && line.roll_ids.contains(&change.slot)))
                    {
                        return Err(WriteError::Refused("Unique roll mapping could not be confirmed".into()));
                    }
                    (c"uniqueRolls", identity.rolls.len())
                }
                RollArray::Implicit => {
                    let base = self.implicit_definitions(attached, api, &graph.images)?;
                    for change in rolls.changes() {
                        if usize::from(change.slot) >= expected.implicit_rolls.len() {
                            return Err(WriteError::Refused("Invalid implicit roll index".into()));
                        }
                        let (_, minimum, maximum) = self.implicit_range(attached, api, &base, expected, usize::from(change.slot))?;
                        if minimum == maximum {
                            return Err(WriteError::Refused("This implicit does not roll".into()));
                        }
                    }
                    (c"implicitRolls", expected.implicit_rolls.len())
                }
            };
            let storage = self.child(attached, &graph.item, field)?.ok_or_else(|| WriteError::Refused("Missing roll array".into()))?;
            let count = self.array_count(&storage, self.system_class(c"Byte")?, array.capacity() as u32)? as usize;
            if count != length {
                return Err(WriteError::Stale("Roll array changed".into()));
            }
            Ok((storage, length))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::test_fakes::{self, corlib, roots, token};
    use il2cpp_runtime::ThreadContext;
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
        static STORES: Cell<usize> = const { Cell::new(0) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static ONLINE: Cell<u8> = const { Cell::new(0) };
        static CURRENT_MANAGER: Cell<usize> = const { Cell::new(MANAGER) };
        static CURRENT_ITEM: Cell<usize> = const { Cell::new(ITEM) };
        static LIVE: Cell<usize> = const { Cell::new(1) };
    }
    unsafe extern "C-unwind" fn class_from_name(_: *const std::ffi::c_void, ns: *const c_char, name: *const c_char) -> Class {
        // SAFETY: adapter supplies static C metadata names.
        unsafe {
            assert_eq!(CStr::from_ptr(ns), c"System");
            assert_eq!(CStr::from_ptr(name), c"Int32");
        }
        token(INT32)
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        assert_eq!(object as usize, ITEM);
        assert_eq!(roots(), 2);
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
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> crate::game::Type {
        token(7)
    }
    unsafe extern "C-unwind" fn byref(_: crate::game::Type) -> bool {
        false
    }
    unsafe extern "C-unwind" fn code(_: crate::game::Type) -> i32 {
        8
    }
    unsafe extern "C-unwind" fn type_class(_: crate::game::Type) -> Class {
        token(INT32)
    }
    unsafe extern "C-unwind" fn parent(_: Ptr) -> Class {
        token(OWNER)
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        144
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
        assert_eq!(roots(), 2);
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
    unsafe extern "C-unwind" fn set(object: *mut raw::Object, field: *mut raw::FieldInfo, _: Ptr) {
        assert_eq!(object as usize, ITEM);
        assert_eq!(field as usize, FIELD);
        assert_eq!(roots(), 2);
        STORES.set(STORES.get() + 1);
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        let ctx = test_fakes::thread();
        STORES.set(0);
        FLAGS.set(0);
        ONLINE.set(0);
        CURRENT_MANAGER.set(MANAGER);
        CURRENT_ITEM.set(ITEM);
        LIVE.set(1);
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
        (a, ctx)
    }
    unsafe fn graph<'a, 'r>(attached: &'a AttachedThread<'r>) -> ForgeGraph<'a, 'r> {
        // SAFETY: both identity tokens are live authored objects recognized by these fake APIs.
        unsafe {
            ForgeGraph {
                images: crate::game::Images { all: &[], game: token(3) },
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
    fn level_requirement_is_its_own_mutable_int32_field() {
        let (a, _ctx) = fixture();
        // SAFETY: authored metadata only; the fakes check the owner, the field name and the Int32 lookup.
        unsafe {
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_ok());
            // The matrix is `mutable_field`'s (apply.rs); this is only that the status edit goes through it.
            FLAGS.set(0x20);
            assert!(a.status_field(token(OWNER), StatusField::LevelRequirement).is_err());
        }
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
            assert!(matches!(a.status_final_gate(&g, u64::MAX), Err(WriteError::Stale(_))));
            CURRENT_MANAGER.set(123);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(WriteError::Stale(_))));
            CURRENT_MANAGER.set(MANAGER);
            CURRENT_ITEM.set(321);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(WriteError::Stale(_))));
            CURRENT_ITEM.set(ITEM);
            LIVE.set(0);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(WriteError::Offline)));
            LIVE.set(1);
            ONLINE.set(1);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(WriteError::Offline)));
            ONLINE.set(2);
            assert!(matches!(a.status_final_gate(&g, MANAGER as u64), Err(WriteError::Refused(_))));
        }
        assert_eq!(STORES.get(), 0);
        drop(g);
        assert_eq!(roots(), 0);
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
            status: eg_trainer_wire::forge::ItemStatus {
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
                // SAFETY: invalid requests must return before any unprovided fake metadata export.
                unsafe {
                    a.edit_forge(
                        &attached,
                        raw::FieldWriteApi::test_export(set),
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
                Err(WriteError::Refused(_))
            ));
        }
        assert_eq!(STORES.get(), 0);
        assert_eq!(roots(), 0);
    }
}
