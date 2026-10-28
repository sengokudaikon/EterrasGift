//! Detached affix sampling through the game's own tooltip producer and formatter.
use crate::game::metadata_text;
use crate::game::{Class, GameError, Images, Method, Root, RuntimeAccess, accessors::item::ForgeGraph, target, unsupported};
use eg_il2cpp_runtime::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, ValueTypeContract, raw};
use eg_trainer_wire::mapping::{AffixMappingQuery, MAX_CONTEXT_BYTES, MappingAddress, MappingStart, MappingTarget, RawMappingPage, RawValueSample};
use std::{collections::BTreeSet, ffi::CStr, ptr::NonNull};
#[derive(PartialEq)]
struct Effect {
    value: f64,
    display: String,
    unit: String,
    text: String,
    varying: bool,
}
fn contract(error: impl std::fmt::Display) -> GameError {
    unsupported(error.to_string())
}
fn context_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

impl RuntimeAccess {
    // These helpers inherit read_affix_mapping's single attached-runtime and private-copy span.
    pub(super) unsafe fn mapping_method(
        &self,
        api: raw::InvocationApi,
        object: &Root<'_, '_>,
        name: &CStr,
        parameters: &[Class],
        returns: Option<Class>,
    ) -> Result<Method, GameError> {
        // SAFETY: The retained owner metadata is scanned with exact parameter/return classes before binding a method.
        unsafe {
            let mut owner = (self.object_class)(target(object)?.as_ptr());
            for _ in 0..256 {
                if owner.is_null() {
                    break;
                }
                let mut found = false;
                for method in self.methods(owner) {
                    let method = method?;
                    let actual = (self.method_name)(method);
                    if actual.is_null() {
                        return Err(unsupported("mapping method name"));
                    }
                    if metadata_text(actual) == name && (self.method_params)(method) as usize == parameters.len() {
                        found = true;
                        break;
                    }
                }
                if found {
                    let method = self.description_method(api, owner, name, parameters, returns, true)?;
                    if (api.method_get_flags)(method, std::ptr::null_mut()) & raw::METHOD_ATTRIBUTE_VIRTUAL != 0 {
                        return NonNull::new((api.object_get_virtual_method)(target(object)?.as_ptr(), method).cast_mut())
                            .map(|m| m.as_ptr().cast_const())
                            .ok_or_else(|| unsupported("mapping virtual method"));
                    }
                    return Ok(method);
                }
                owner = (self.class_parent)(owner);
            }
            Err(unsupported("mapping method missing"))
        }
    }
    unsafe fn mapping_float(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, object: &Root<'_, '_>, name: &CStr) -> Result<f32, GameError> {
        // SAFETY: The checked method and rooted receiver remain live while the exact scalar result is unboxed.
        unsafe {
            let method = self.mapping_method(api, object, name, &[], Some(self.system_class(c"Single")?))?;
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind(NonNull::new(method.cast_mut()).ok_or_else(|| unsupported("mapping scalar method"))?)
                .map_err(contract)?;
            match method.invoke(Some(object), &mut []).map_err(contract)? {
                InvocationOutcome::Returned(ReturnValue::Scalar { value: Scalar::F32(value), .. }) if value.is_finite() => Ok(value),
                _ => Err(unsupported("mapping scalar failed, threw or was nonfinite")),
            }
        }
    }
    unsafe fn mapping_void(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        object: &Root<'_, '_>,
        method: Method,
        args: &mut [Argument<'_, '_, '_>],
    ) -> Result<(), GameError> {
        // SAFETY: The method was signature-checked and the rooted receiver remains live throughout this synchronous invocation.
        unsafe {
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind(NonNull::new(method.cast_mut()).ok_or_else(|| unsupported("mapping void method"))?)
                .map_err(contract)?;
            match method.invoke(Some(object), args).map_err(contract)? {
                InvocationOutcome::Returned(ReturnValue::Void) => Ok(()),
                _ => Err(unsupported("detached constructor/refresh failed or threw")),
            }
        }
    }
    unsafe fn mapping_bytes(&self, array: &Root<'_, '_>, maximum: u32) -> Result<Vec<u8>, GameError> {
        // SAFETY: The caller supplies the main-thread copy span; array allocation uses the exact retained Byte class and is immediately rooted.
        unsafe {
            let count = self.array_count(array, self.system_class(c"Byte")?, maximum)?;
            let start = target(array)?.as_ptr().cast::<u8>().add(32);
            Ok(crate::game::copy_values(start, count as usize))
        }
    }
    unsafe fn mapping_clone_bytes<'a, 'r>(&self, attached: &'a AttachedThread<'r>, source: &Root<'_, '_>, maximum: u32) -> Result<Root<'a, 'r>, GameError> {
        // SAFETY: The source array is rooted and validated; the new array is independently rooted before copying its bounded bytes.
        unsafe {
            let bytes = self.mapping_bytes(source, maximum)?;
            let array = NonNull::new((self.array_new)(self.system_class(c"Byte")?, bytes.len())).ok_or_else(|| unsupported("detached byte allocation"))?;
            let array = attached.root(array)?;
            if self.array_count(&array, self.system_class(c"Byte")?, maximum)? as usize != bytes.len() {
                return Err(unsupported("detached byte array length"));
            }
            crate::game::write_bytes(target(&array)?.as_ptr().cast::<u8>().add(32), &bytes);
            Ok(array)
        }
    }
    pub(super) unsafe fn mapping_copy<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        graph: &ForgeGraph<'_, '_>,
    ) -> Result<Root<'a, 'r>, GameError> {
        // SAFETY: The selected graph stays rooted; constructor/copy methods are checked and every mutable array is independently cloned.
        unsafe {
            let serial = self.child(attached, &graph.item, c"id")?.ok_or_else(|| unsupported("item serialization absent"))?;
            let serial = self.mapping_clone_bytes(attached, &serial, 4096)?;
            let unpacked = self.class(&graph.images, c"", c"ItemDataUnpacked")?;
            let constructor = self.description_method(
                api,
                unpacked,
                c".ctor",
                &[(self.object_class)(target(&serial)?.as_ptr())],
                Some(self.system_class(c"Void")?),
                true,
            )?;
            let object = NonNull::new((self.object_new)(unpacked)).ok_or_else(|| unsupported("detached item allocation"))?;
            let copy = attached.root(object)?;
            self.mapping_void(attached, api, &copy, constructor, &mut [Argument::Reference(Some(&serial))])?;
            if target(&copy)? == target(&graph.item)? {
                return Err(unsupported("detached receiver aliases live item"));
            }
            let class = self.class(&graph.images, c"", c"ItemAffix")?;
            let live = self.child(attached, &graph.item, c"affixes")?.ok_or_else(|| unsupported("live affixes absent"))?;
            let copied = self.child(attached, &copy, c"affixes")?.ok_or_else(|| unsupported("detached affixes absent"))?;
            let live_values = self.list(attached, &live, class, 32)?;
            let copy_values = self.list(attached, &copied, class, 32)?;
            if live_values.len() != copy_values.len() || target(&live)? == target(&copied)? {
                return Err(unsupported("detached affix list is not independent"));
            }
            let live_array = self.child(attached, &live, c"_items")?.ok_or_else(|| unsupported("live affix storage absent"))?;
            let copy_array = self.child(attached, &copied, c"_items")?.ok_or_else(|| unsupported("detached affix storage absent"))?;
            if !live_values.is_empty() && target(&live_array)? == target(&copy_array)? {
                return Err(unsupported("detached affix array aliases live storage"));
            }
            let live_addresses = live_values.iter().map(|v| target(v).map(|v| v.as_ptr() as usize)).collect::<Result<BTreeSet<_>, _>>()?;
            for value in &copy_values {
                if live_addresses.contains(&(target(value)?.as_ptr() as usize)) {
                    return Err(unsupported("detached affix object aliases live storage"));
                }
            }
            let count_field = self.mutable_field(class, c"affixCountForSetProps", c"Int32", 0x08).map_err(contract)?;
            for (live, copy) in live_values.iter().zip(&copy_values) {
                writes.set_i32(target(copy)?, count_field, self.primitive::<i32>(live, c"affixCountForSetProps")?);
            }
            for name in [c"uniqueRolls", c"implicitRolls"] {
                if let Some(array) = self.child(attached, &graph.item, name)? {
                    let cloned = self.mapping_clone_bytes(attached, &array, 8)?;
                    self.set_reference(&writes, attached, &copy, name, &cloned)?;
                }
            }
            Ok(copy)
        }
    }
    unsafe fn mapping_effects(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, images: &Images, item: &Root<'_, '_>, slot: u8) -> Result<Vec<Effect>, GameError> {
        // SAFETY: The detached item, builder and resulting tooltip objects remain rooted throughout checked invocations and owned copying.
        unsafe {
            let affix_class = self.class(images, c"", c"ItemAffix")?;
            let affixes = self.child(attached, item, c"affixes")?.ok_or_else(|| unsupported("mapping affixes absent"))?;
            let values = self.list(attached, &affixes, affix_class, 32)?;
            let affix = values.get(usize::from(slot)).ok_or_else(|| unsupported("mapping slot absent"))?;
            let builder = self.class(images, c"LE.UI.Tooltips", c"ItemTooltipContentBuilder")?;
            let mut slot_type = None;
            for method in self.methods(builder) {
                let method = method?;
                let name = (self.method_name)(method);
                if !name.is_null() && metadata_text(name) == c"GetItemAffixInfo" && (self.method_params)(method) == 3 {
                    if slot_type.is_some() {
                        return Err(unsupported("ambiguous affix tooltip builder"));
                    }
                    slot_type = NonNull::new((self.type_class)((self.method_param)(method, 2)));
                }
            }
            let slot_type = slot_type.ok_or_else(|| unsupported("affix tooltip slot enum"))?;
            let method = self.description_method(
                api,
                builder,
                c"GetItemAffixInfo",
                &[self.class(images, c"", c"ItemDataUnpacked")?, affix_class, slot_type.as_ptr()],
                None,
                false,
            )?;
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind_with_value_types(
                    NonNull::new(method.cast_mut()).ok_or_else(|| unsupported("affix tooltip method"))?,
                    &[ValueTypeContract::I32Enum { class: slot_type }],
                )
                .map_err(contract)?;
            let info = match method
                .invoke(
                    None,
                    &mut [
                        Argument::Reference(Some(item)),
                        Argument::Reference(Some(affix)),
                        Argument::EnumI32 { class: slot_type, value: 0 },
                    ],
                )
                .map_err(contract)?
            {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(info))) => info,
                _ => return Err(unsupported("affix tooltip builder failed or threw")),
            };
            let mut effects = Vec::new();
            for getter in [c"get_Modifier1", c"get_Modifier2"] {
                let method = self.mapping_method(api, &info, getter, &[], None)?;
                let modifier = self.description_call(attached, api, method, Some(&info), &mut [])?;
                let Some(modifier) = modifier else {
                    continue;
                };
                let value = self.mapping_float(attached, api, &modifier, c"get_Value")?;
                let minimum = self.mapping_float(attached, api, &modifier, c"get_MinValue")?;
                let maximum = self.mapping_float(attached, api, &modifier, c"get_MaxValue")?;
                let convert = self.mapping_method(
                    api,
                    &modifier,
                    c"ConvertValueToString",
                    &[self.system_class(c"Single")?],
                    Some(self.system_class(c"String")?),
                )?;
                let display = self.description_call(attached, api, convert, Some(&modifier), &mut [Argument::Scalar(Scalar::F32(value))])?;
                let display = self.description_string(display.as_ref())?;
                let unit = if display.ends_with('%') { "%" } else { "" }.to_owned();
                effects.push(Effect {
                    value: f64::from(value),
                    unit,
                    display,
                    text: self.description_text(attached, api, &modifier, c"ToModifierString")?,
                    varying: minimum != maximum,
                });
            }
            if effects.is_empty() {
                return Err(unsupported("no visible numeric effect maps to this affix"));
            }
            if let Some(primary) = effects.iter().position(|effect| effect.varying) {
                effects[..=primary].rotate_right(1);
            }
            Ok(effects)
        }
    }
    unsafe fn mapping_assign_roll(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        images: &Images,
        item: &Root<'_, '_>,
        slot: u8,
        roll: u8,
    ) -> Result<(), GameError> {
        // SAFETY: The caller supplies a detached rooted item; exact Byte field metadata is checked before storing its sample roll.
        unsafe {
            let class = self.class(images, c"", c"ItemAffix")?;
            let list = self.child(attached, item, c"affixes")?.ok_or_else(|| unsupported("detached affix list absent"))?;
            let values = self.list(attached, &list, class, 32)?;
            let affix = values.get(usize::from(slot)).ok_or_else(|| unsupported("detached affix slot absent"))?;
            let field = self.mutable_field(class, c"affixRoll", c"Byte", 0x05).map_err(contract)?;
            writes.set_u8(target(affix)?, field, roll);
            Ok(())
        }
    }
    unsafe fn mapping_project(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        graph: &ForgeGraph<'_, '_>,
        copy: &Root<'_, '_>,
        query: &AffixMappingQuery,
    ) -> Result<(), GameError> {
        // SAFETY: Only the validated detached copy is changed; the selected item remains rooted for independent stale checks.
        unsafe {
            let Some(draft) = query.draft() else {
                return Ok(());
            };
            let original = &query.expected().affixes[usize::from(query.slot())];
            if original.id == draft.id && original.tier == u16::from(draft.tier) && original.seal == i32::from(draft.seal) {
                return Ok(());
            }
            self.validate_affix_definition(attached, graph, draft.id, u16::from(draft.tier)).map_err(contract)?;
            let class = self.class(&graph.images, c"", c"ItemAffix")?;
            let list = self.child(attached, copy, c"affixes")?.ok_or_else(|| unsupported("detached affixes absent"))?;
            let original = self.list(attached, &list, class, 32)?;
            let constructor = self.method(class, c".ctor", &["System.UInt16", "System.Byte", "System.Byte", "System.Byte", "SealedAffixType"], true)?;
            let seal = NonNull::new(self.field_class(self.field(class, c"sealedAffixType")?, "mapping")?).ok_or_else(|| unsupported("seal enum absent"))?;
            let context = InvocationContext::new(attached, api);
            let constructor = context
                .bind_with_value_types(
                    NonNull::new(constructor.cast_mut()).ok_or_else(|| unsupported("affix constructor"))?,
                    &[ValueTypeContract::I32Enum { class: seal }],
                )
                .map_err(contract)?;
            let replacement = attached.root(NonNull::new((self.object_new)(class)).ok_or_else(|| unsupported("draft affix allocation"))?)?;
            match constructor
                .invoke(
                    Some(&replacement),
                    &mut [
                        Argument::Scalar(Scalar::U16(draft.id)),
                        Argument::Scalar(Scalar::U8(draft.tier - 1)),
                        Argument::Scalar(Scalar::U8(0)),
                        Argument::Scalar(Scalar::U8(query.expected().item_type)),
                        Argument::EnumI32 {
                            class: seal,
                            value: i32::from(draft.seal),
                        },
                    ],
                )
                .map_err(contract)?
            {
                InvocationOutcome::Returned(ReturnValue::Void) => {}
                _ => return Err(unsupported("draft affix construction failed or threw")),
            }
            if self.primitive::<u16>(&replacement, c"affixId")? != draft.id
                || self.primitive::<u8>(&replacement, c"affixTier")? != draft.tier - 1
                || self.enumeration(&replacement, c"sealedAffixType", c"SealedAffixType")? != i32::from(draft.seal)
            {
                return Err(unsupported("draft affix constructor normalized requested shape"));
            }
            if query.expected().affixes[usize::from(query.slot())].id == draft.id {
                let field = self.mutable_field(class, c"affixCountForSetProps", c"Int32", 0x08).map_err(contract)?;
                writes.set_i32(
                    target(&replacement)?,
                    field,
                    self.primitive::<i32>(&original[usize::from(query.slot())], c"affixCountForSetProps")?,
                );
            }
            let list_class = (self.object_class)(target(&list)?.as_ptr());
            let clear = self.affix_list_method(list_class, class, c"Clear", false, api).map_err(contract)?;
            let add = self.affix_list_method(list_class, class, c"Add", true, api).map_err(contract)?;
            let list_class = NonNull::new(list_class).ok_or_else(|| unsupported("detached list class"))?;
            let clear = context
                .bind_with_closed_references(NonNull::new(clear.cast_mut()).ok_or_else(|| unsupported("detached Clear"))?, &[list_class])
                .map_err(contract)?;
            let add = context
                .bind_with_closed_references(NonNull::new(add.cast_mut()).ok_or_else(|| unsupported("detached Add"))?, &[list_class])
                .map_err(contract)?;
            if !matches!(clear.invoke(Some(&list), &mut []).map_err(contract)?, InvocationOutcome::Returned(ReturnValue::Void)) {
                return Err(unsupported("detached list Clear threw"));
            }
            for (index, object) in original.iter().enumerate() {
                let object = if index == usize::from(query.slot()) { &replacement } else { object };
                if !matches!(
                    add.invoke(Some(&list), &mut [Argument::Reference(Some(object))]).map_err(contract)?,
                    InvocationOutcome::Returned(ReturnValue::Void)
                ) {
                    return Err(unsupported("detached list Add threw"));
                }
            }
            let mut flags = [false; 3];
            for (index, object) in original.iter().enumerate() {
                let object = if index == usize::from(query.slot()) { &replacement } else { object };
                let seal = self.enumeration(object, c"sealedAffixType", c"SealedAffixType")?;
                if (1..=3).contains(&seal) {
                    flags[(seal - 1) as usize] = true;
                }
            }
            let owner = self.class(&graph.images, c"", c"ItemData")?;
            for (name, value) in [c"hasSealedRegularAffix", c"hasSealedPrimordialAffix", c"hasSealedAffixFromCorruption"]
                .into_iter()
                .zip(flags)
            {
                let field = self.mutable_field(owner, name, c"Boolean", 0x02).map_err(contract)?;
                writes.set_u8(target(copy)?, field, u8::from(value));
            }
            Ok(())
        }
    }

    /// Samples only a proven independent deserialized item, through game tooltip
    /// methods. The forge graph is read and compared, never stored through.
    /// # Safety
    /// The read_forge supported-runtime/main-thread/graph conditions apply for the
    /// whole synchronous operation. Authorize the game's byte-array constructor,
    /// detached RebuildID and tooltip/culture closure (including allocations, GC,
    /// caches and barriers). These bodies must not mutate the selected live item;
    /// other threads must not change the graph or active culture during sampling.
    /// The nonmoving runtime owns all arrays; Field::SetValue performs reference
    /// barriers. No pointer token from the request is dereferenced.
    pub unsafe fn read_affix_mapping(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        query: &AffixMappingQuery,
    ) -> Result<RawMappingPage, GameError> {
        // SAFETY: The caller supplies the main-thread immutable graph span; selected and detached items stay rooted across each sampled call.
        unsafe {
            let graph = self.forge_graph(attached)?.ok_or(GameError::NotReady("forge item absent"))?;
            if target(&graph.manager)?.as_ptr() as u64 != query.manager() || self.forge_snapshot(attached, &graph)? != *query.expected() {
                return Err(unsupported("forge changed before mapping"));
            }
            let locale = self.read_game_locale(attached, api)?;
            let copy = self.mapping_copy(attached, writes, api, &graph)?;
            if self.mapping_effects(attached, api, &graph.images, &graph.item, query.slot())? != self.mapping_effects(attached, api, &graph.images, &copy, query.slot())? {
                return Err(unsupported("detached tooltip context differs from live item"));
            }
            self.mapping_project(attached, writes, api, &graph, &copy, query)?;
            self.mapping_assign_roll(attached, writes, &graph.images, &copy, query.slot(), 0)?;
            let rebuild = self.method(self.class(&graph.images, c"", c"ItemData")?, c"RebuildID", &[], true)?;
            self.mapping_void(attached, api, &copy, rebuild, &mut [])?;
            let serial = self
                .child(attached, &copy, c"id")?
                .ok_or_else(|| unsupported("detached serialization absent after refresh"))?;
            let serial = self.mapping_bytes(&serial, 4096)?;
            let mut opaque = b"affix-map-v1".to_vec();
            opaque.extend_from_slice(&(serial.len() as u32).to_le_bytes());
            opaque.extend(serial);
            for endpoint in [0, 255] {
                self.mapping_assign_roll(attached, writes, &graph.images, &copy, query.slot(), endpoint)?;
                let effects = self.mapping_effects(attached, api, &graph.images, &copy, query.slot())?;
                opaque.extend_from_slice(&(effects.len() as u32).to_le_bytes());
                for effect in effects {
                    opaque.extend_from_slice(&effect.value.to_le_bytes());
                    context_text(&mut opaque, &effect.display);
                    context_text(&mut opaque, &effect.unit);
                    context_text(&mut opaque, &effect.text);
                }
            }
            if opaque.len() > MAX_CONTEXT_BYTES {
                return Err(unsupported("mapping context exceeds bound"));
            }
            let mut samples = Vec::new();
            if let MappingStart::Samples(start) = query.start() {
                for roll in usize::from(start)..(usize::from(start) + 16).min(256) {
                    self.mapping_assign_roll(attached, writes, &graph.images, &copy, query.slot(), roll as u8)?;
                    let effects = self.mapping_effects(attached, api, &graph.images, &copy, query.slot())?;
                    samples.push(RawValueSample {
                        raw_quantity: effects[0].value,
                        display: effects[0].display.clone(),
                        unit: effects[0].unit.clone(),
                        effects: effects.into_iter().map(|effect| effect.text).collect(),
                    });
                }
            }
            let current = self.forge_graph(attached)?.ok_or(GameError::NotReady("forge removed during mapping"))?;
            if target(&current.manager)?.as_ptr() as u64 != query.manager()
                || self.forge_snapshot(attached, &current)? != *query.expected()
                || self.read_game_locale(attached, api)? != locale
            {
                return Err(unsupported("forge or locale changed during mapping"));
            }
            let page = RawMappingPage {
                language: locale.language,
                context: opaque,
                address: MappingAddress {
                    target: MappingTarget::Affix,
                    index: query.slot(),
                    line: None,
                },
                start: query.start(),
                samples,
            };
            eg_trainer_wire::mapping::encode_mapping(&page).map_err(contract)?;
            Ok(page)
        }
    }
}
