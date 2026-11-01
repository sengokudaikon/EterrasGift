//! Optional implicit text from the methods the game's own item tooltip uses.
use crate::game::metadata_text;
use crate::game::{Class, GameError, Images, Method, Root, RuntimeAccess, target, unsupported};
use eg_trainer_wire::mapping::{ForgeDescriptionRequest, MAX_TEXT_BYTES, ModifierSnapshot};
use il2cpp_runtime::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use std::{ffi::CStr, ptr::NonNull};

impl RuntimeAccess {
    // Exact supported-game call signatures. Closed List<T> returns require an
    // explicit class allowlist when bound below.
    pub(super) unsafe fn description_method(
        &self,
        api: raw::InvocationApi,
        owner: Class,
        name: &CStr,
        parameters: &[Class],
        returns: Option<Class>,
        instance: bool,
    ) -> Result<Method, GameError> {
        // SAFETY: Owner and method descriptors belong to retained metadata; parameter and return classes are checked before binding.
        unsafe {
            let mut found = None;
            for method in self.methods(owner) {
                let method = method?;
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("tooltip method name"));
                }
                if metadata_text(actual) != name || (self.method_params)(method) as usize != parameters.len() {
                    continue;
                }
                let parameters_match = parameters.iter().enumerate().all(|(index, expected)| {
                    let ty = (self.method_param)(method, index as u32);
                    !ty.is_null() && !(self.type_byref)(ty) && (self.type_class)(ty) == *expected
                });
                if !parameters_match {
                    continue;
                }
                let result = (self.method_return)(method);
                if found.is_some()
                    || (api.method_get_class)(method) != owner
                    || (self.method_instance)(method) != instance
                    || (self.method_generic)(method)
                    || (self.method_inflated)(method)
                    || (api.method_get_flags)(method, std::ptr::null_mut()) & (raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT) != 0
                    || result.is_null()
                    || (self.type_byref)(result)
                    || returns.is_none() && !matches!((self.type_code)(result), raw::type_code::STRING | raw::type_code::CLASS | raw::type_code::GENERICINST)
                    || returns.is_some_and(|expected| (self.type_class)(result) != expected)
                {
                    return Err(unsupported("tooltip method signature"));
                }
                for (index, expected) in parameters.iter().enumerate() {
                    let ty = (self.method_param)(method, index as u32);
                    if ty.is_null() || (self.type_byref)(ty) || (self.type_class)(ty) != *expected {
                        return Err(unsupported("tooltip parameter signature"));
                    }
                }
                found = Some(method);
            }
            found.ok_or_else(|| unsupported(format!("missing tooltip method {}", name.to_string_lossy())))
        }
    }
    pub(super) unsafe fn description_call<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        method: Method,
        receiver: Option<&Root<'_, '_>>,
        args: &mut [Argument<'_, 'a, 'r>],
    ) -> Result<Option<Root<'a, 'r>>, GameError> {
        // SAFETY: The method was signature-checked; InvocationContext roots the receiver, arguments and returned reference across the call.
        unsafe {
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind(NonNull::new(method.cast_mut()).ok_or_else(|| unsupported("tooltip method"))?)
                .map_err(|e| unsupported(e.to_string()))?;
            match method.invoke(receiver, args).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(value)) => Ok(value),
                _ => Err(unsupported("tooltip invocation failed or threw")),
            }
        }
    }
    pub(super) unsafe fn description_text(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, info: &Root<'_, '_>, name: &CStr) -> Result<String, GameError> {
        // SAFETY: The rooted receiver and checked getter stay live until its rooted string result is copied.
        unsafe {
            let string = self.system_class(c"String")?;
            let mut owner = (self.object_class)(target(info)?.as_ptr());
            // The concrete tooltip class may inherit a virtual formatter. Resolve
            // its runtime override rather than calling a base implementation.
            let mut method = None;
            for _ in 0..256 {
                if owner.is_null() {
                    break;
                }
                for candidate in self.methods(owner) {
                    let candidate = candidate?;
                    let actual = (self.method_name)(candidate);
                    if actual.is_null() {
                        return Err(unsupported("tooltip formatter name"));
                    }
                    if metadata_text(actual) == name && (self.method_params)(candidate) == 1 {
                        method = Some(owner);
                        break;
                    }
                }
                if method.is_some() {
                    break;
                }
                owner = (self.class_parent)(owner);
            }
            let owner = method.ok_or_else(|| unsupported("tooltip formatter unavailable"))?;
            let mut method = self.description_method(api, owner, name, &[self.system_class(c"Boolean")?], Some(string), true)?;
            if (api.method_get_flags)(method, std::ptr::null_mut()) & raw::METHOD_ATTRIBUTE_VIRTUAL != 0 {
                method = (api.object_get_virtual_method)(target(info)?.as_ptr(), method);
                if method.is_null() {
                    return Err(unsupported("tooltip virtual formatter unavailable"));
                }
            }
            let text = self.description_call(attached, api, method, Some(info), &mut [Argument::Scalar(Scalar::Boolean(false))])?;
            self.description_string(text.as_ref())
        }
    }
    pub(super) unsafe fn description_string(&self, text: Option<&Root<'_, '_>>) -> Result<String, GameError> {
        // SAFETY: The rooted string has validated class and bounded length; its character buffer remains live for the owned copy.
        unsafe {
            let string = self.system_class(c"String")?;
            Ok(self.owned_text(text, string, MAX_TEXT_BYTES)?)
        }
    }

    /// Calls get_Implicits, ItemList.GetItemImplicit and GetItemImplicitInfo, then
    /// the game's virtual ToModifierString(false)/ToRangeString(false). No item stores.
    /// # Safety
    /// The read_forge runtime/main-thread/graph conditions apply throughout. The
    /// supported game methods may allocate, GC and initialize tooltip/localization
    /// caches; their selected item and culture remain stable during this synchronous
    /// call. Roots retain every returned object. No parsing or value mapping is inferred.
    pub unsafe fn describe_implicits(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, request: &ForgeDescriptionRequest) -> Result<Vec<ModifierSnapshot>, GameError> {
        // SAFETY: The caller supplies the main-thread immutable graph span; selected and detached items stay rooted across tooltip calls.
        unsafe {
            let graph = self.forge_graph(attached)?.ok_or(GameError::NotReady("forge item absent"))?;
            if !self.forge_unchanged(attached, &graph, request.manager(), request.expected())? {
                return Err(unsupported("forge changed before implicit description"));
            }
            let mut modifiers = Vec::new();
            for line in self.implicit_lines(attached, api, &graph.images, &graph.item, request.expected())? {
                modifiers.push(ModifierSnapshot {
                    index: line.slot as u32,
                    text: self.description_text(attached, api, &line.info, c"ToModifierString")?,
                    range: self.description_text(attached, api, &line.info, c"ToRangeString")?,
                    roll_ids: if line.slot < request.expected().implicit_rolls.len() && line.minimum != line.maximum {
                        vec![line.slot as u8]
                    } else {
                        vec![]
                    },
                    editable: true,
                });
            }
            // Calls may run managed callbacks. Never publish text for an item or
            // graph that changed while producing it, even if its roots survived.
            let current = self.forge_graph(attached)?.ok_or(GameError::NotReady("forge item removed during formatting"))?;
            if !self.forge_unchanged(attached, &current, request.manager(), request.expected())? {
                return Err(unsupported("forge changed during implicit description"));
            }
            eg_trainer_wire::mapping::encode_modifiers(&modifiers).map_err(|e| unsupported(e.to_string()))?;
            Ok(modifiers)
        }
    }

    /// The item's implicit lines as its tooltip builds them (`get_Implicits`, `ItemList.GetItemImplicit`,
    /// `GetItemImplicitInfo`), each with its base's range. `snapshot` is the item's own read, for its base.
    /// # Safety
    /// As for [`Self::describe_implicits`]; `item` may be the selected item or a detached copy of it.
    pub(super) unsafe fn implicit_lines<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        images: &Images,
        item: &Root<'a, 'r>,
        snapshot: &eg_trainer_wire::forge::ForgeSnapshot,
    ) -> Result<Vec<ImplicitLine<'a, 'r>>, GameError> {
        // SAFETY: Checked signatures; the item, list, definitions and every tooltip result stay rooted.
        unsafe {
            let unpacked = self.class(images, c"", c"ItemDataUnpacked")?;
            let getter = self.mapping_method(api, item, c"get_Implicits", &[], None)?;
            let list_class = (self.type_class)((self.method_return)(getter));
            self.list_class(list_class)?;
            let array_class = self.field_class(self.field(list_class, c"_items")?, "description")?;
            let element = (self.class_element)(array_class);
            if element.is_null() || (self.class_value)(element) || (self.class_image)(element) != images.game {
                return Err(unsupported("implicit getter game reference element"));
            }
            self.array_class(array_class, element)?;
            let context = InvocationContext::new(attached, api);
            let getter = context
                .bind_with_closed_references(
                    NonNull::new(getter.cast_mut()).ok_or_else(|| unsupported("implicit getter"))?,
                    &[NonNull::new(list_class).ok_or_else(|| unsupported("implicit list class"))?],
                )
                .map_err(|e| unsupported(e.to_string()))?;
            let value = match getter.invoke(Some(item), &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(list)) => list,
                _ => return Err(unsupported("implicit getter threw or returned a non-reference")),
            };
            let mut lines = Vec::new();
            if let Some(list) = value {
                self.exact(&list, list_class)?;
                let implicits = self.list(attached, &list, element, eg_trainer_wire::forge::MAX_IMPLICITS as u32)?;
                if !implicits.is_empty() {
                    let base = self.implicit_definitions(attached, api, images)?;
                    let builder = self.class(images, c"LE.UI.Tooltips", c"ItemTooltipContentBuilder")?;
                    let build = self.description_method(api, builder, c"GetItemImplicitInfo", &[unpacked, element, base.data_class], None, false)?;
                    for (slot, implicit) in implicits.iter().enumerate() {
                        let (data, minimum, maximum) = self.implicit_range(attached, api, &base, snapshot, slot)?;
                        let info = self.description_call(
                            attached,
                            api,
                            build,
                            None,
                            &mut [Argument::Reference(Some(item)), Argument::Reference(Some(implicit)), Argument::Reference(Some(&data))],
                        )?;
                        if let Some(info) = info {
                            lines.push(ImplicitLine { slot, info, minimum, maximum });
                        }
                    }
                }
            }
            Ok(lines)
        }
    }

    /// The game's implicit registry (`ItemList.get`) and its per-base lookup, `GetItemImplicit(Int32, Int32, Int32)`.
    /// # Safety
    /// The describe_implicits runtime/main-thread conditions apply; the registry stays rooted in the result.
    pub(super) unsafe fn implicit_definitions<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        images: &Images,
    ) -> Result<ImplicitDefinitions<'a, 'r>, GameError> {
        // SAFETY: Checked signatures only; the registry is rooted and checked live before any lookup.
        unsafe {
            let item_list = self.class(images, c"", c"ItemList")?;
            let data_class = self.nested(item_list, c"EquipmentImplicit")?;
            let singleton = self.description_method(api, item_list, c"get", &[], Some(item_list), false)?;
            let registry = self
                .description_call(attached, api, singleton, None, &mut [])?
                .ok_or(GameError::NotReady("implicit definitions absent"))?;
            if !self.is_live(attached, images, &registry)? {
                return Err(GameError::NotReady("implicit definitions destroyed"));
            }
            let i32_class = self.system_class(c"Int32")?;
            let lookup = self.description_method(api, item_list, c"GetItemImplicit", &[i32_class; 3], Some(data_class), true)?;
            Ok(ImplicitDefinitions { registry, lookup, data_class })
        }
    }

    /// One implicit's definition for the item's base, with its range (`implicitValue` to `implicitMaxValue`). A range of
    /// one value has no roll.
    /// # Safety
    /// As for [`Self::implicit_definitions`].
    pub(super) unsafe fn implicit_range<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        base: &ImplicitDefinitions<'_, '_>,
        item: &eg_trainer_wire::forge::ForgeSnapshot,
        slot: usize,
    ) -> Result<(Root<'a, 'r>, f32, f32), GameError> {
        // SAFETY: The lookup was signature-checked and its registry is rooted; the returned definition is rooted.
        unsafe {
            let data = self
                .description_call(
                    attached,
                    api,
                    base.lookup,
                    Some(&base.registry),
                    &mut [
                        Argument::Scalar(Scalar::I32(i32::from(item.item_type))),
                        Argument::Scalar(Scalar::I32(i32::from(item.sub_type))),
                        Argument::Scalar(Scalar::I32(slot as i32)),
                    ],
                )?
                .ok_or_else(|| unsupported("implicit definition missing"))?;
            let minimum: f32 = self.primitive(&data, c"implicitValue")?;
            let maximum: f32 = self.primitive(&data, c"implicitMaxValue")?;
            if !minimum.is_finite() || !maximum.is_finite() {
                return Err(unsupported("implicit nonfinite range"));
            }
            Ok((data, minimum, maximum))
        }
    }
}

/// One implicit line: its slot, the game's tooltip info for it and its base's range.
pub(super) struct ImplicitLine<'a, 'r> {
    pub slot: usize,
    pub info: Root<'a, 'r>,
    pub minimum: f32,
    pub maximum: f32,
}

/// The game's implicit registry and lookup, from [`RuntimeAccess::implicit_definitions`].
pub(super) struct ImplicitDefinitions<'a, 'r> {
    registry: Root<'a, 'r>,
    lookup: Method,
    pub data_class: Class,
}
