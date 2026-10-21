//! Optional implicit text from the same game methods used by the original tooltip.
use super::metadata_text;
use super::{
    Class, DiscoveryError, Method, RuntimeAccess,
    forge::{Root, target},
    unsupported,
};
use crate::il2cpp::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use eg_trainer_wire::mapping::{ForgeDescriptionRequest, MAX_TEXT_BYTES, ModifierSnapshot};
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
    ) -> Result<Method, DiscoveryError> {
        // SAFETY: Owner and method descriptors belong to retained metadata; parameter and return classes are checked before binding.
        unsafe {
            let mut found = None;
            for method in self.methods(owner)? {
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
                    || (api.method_get_flags)(method, std::ptr::null_mut()) & 0x2400 != 0
                    || result.is_null()
                    || (self.type_byref)(result)
                    || returns.is_none() && !matches!((self.type_code)(result), 0x0e | 0x12 | 0x15)
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
    ) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
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
    pub(super) unsafe fn description_text(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, info: &Root<'_, '_>, name: &CStr) -> Result<String, DiscoveryError> {
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
                for candidate in self.methods(owner)? {
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
            if (api.method_get_flags)(method, std::ptr::null_mut()) & 0x40 != 0 {
                method = (api.object_get_virtual_method)(target(info)?.as_ptr(), method);
                if method.is_null() {
                    return Err(unsupported("tooltip virtual formatter unavailable"));
                }
            }
            let text = self.description_call(attached, api, method, Some(info), &mut [Argument::Scalar(Scalar::Boolean(false))])?;
            self.description_string(text.as_ref())
        }
    }
    pub(super) unsafe fn description_string(&self, text: Option<&Root<'_, '_>>) -> Result<String, DiscoveryError> {
        // SAFETY: The rooted string has validated class and bounded length; its character buffer remains live for the owned copy.
        unsafe {
            let string = self.system_class(c"String")?;
            self.owned_text(text, string, MAX_TEXT_BYTES)
        }
    }

    /// Calls get_Implicits, ItemList.GetItemImplicit and GetItemImplicitInfo, then
    /// the game's virtual ToModifierString(false)/ToRangeString(false). No item stores.
    /// # Safety
    /// The read_forge runtime/main-thread/graph conditions apply throughout. The
    /// supported game methods may allocate, GC and initialize tooltip/localization
    /// caches; their selected item and culture remain stable during this synchronous
    /// call. Roots retain every returned object. No parsing or value mapping is inferred.
    pub unsafe fn describe_implicits(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        request: &ForgeDescriptionRequest,
    ) -> Result<Vec<ModifierSnapshot>, DiscoveryError> {
        // SAFETY: The caller supplies the main-thread immutable graph span; selected and detached items stay rooted across tooltip calls.
        unsafe {
            let graph = self.forge_graph(attached)?.ok_or(DiscoveryError::NotReady("forge item absent"))?;
            if target(&graph.manager)?.as_ptr() as u64 != request.manager() || self.forge_snapshot(attached, &graph)? != *request.expected() {
                return Err(unsupported("forge changed before implicit description"));
            }
            let unpacked = self.class(&graph.images, c"", c"ItemDataUnpacked")?;
            let getter = self.mapping_method(api, &graph.item, c"get_Implicits", &[], None)?;
            let list_class = (self.type_class)((self.method_return)(getter));
            self.list_class(list_class)?;
            let array_class = self.field_class(self.field(list_class, c"_items")?)?;
            let element = (self.class_element)(array_class);
            if element.is_null() || (self.class_value)(element) || (self.class_image)(element) != graph.images.game {
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
            let value = match getter.invoke(Some(&graph.item), &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(list)) => list,
                _ => return Err(unsupported("implicit getter threw or returned a non-reference")),
            };
            let mut modifiers = Vec::new();
            if let Some(list) = value {
                self.exact(&list, list_class)?;
                let implicits = self.catalog_list(attached, &list, element, eg_trainer_wire::catalog::MAX_IMPLICITS as u32)?;
                if !implicits.is_empty() {
                    let item_list = self.class(&graph.images, c"", c"ItemList")?;
                    let data_class = self.catalog_nested(item_list, c"EquipmentImplicit")?;
                    let singleton = self.description_method(api, item_list, c"get", &[], Some(item_list), false)?;
                    let definitions = self
                        .description_call(attached, api, singleton, None, &mut [])?
                        .ok_or(DiscoveryError::NotReady("implicit definitions absent"))?;
                    if !self.catalog_live(attached, &graph.images, &definitions)? {
                        return Err(DiscoveryError::NotReady("implicit definitions destroyed"));
                    }
                    let i32_class = self.system_class(c"Int32")?;
                    let get_data = self.description_method(api, item_list, c"GetItemImplicit", &[i32_class; 3], Some(data_class), true)?;
                    let builder = self.class(&graph.images, c"LE.UI.Tooltips", c"ItemTooltipContentBuilder")?;
                    let build = self.description_method(api, builder, c"GetItemImplicitInfo", &[unpacked, element, data_class], None, false)?;
                    for (slot, implicit) in implicits.iter().enumerate() {
                        let data = self
                            .description_call(
                                attached,
                                api,
                                get_data,
                                Some(&definitions),
                                &mut [
                                    Argument::Scalar(Scalar::I32(i32::from(request.expected().item_type))),
                                    Argument::Scalar(Scalar::I32(i32::from(request.expected().sub_type))),
                                    Argument::Scalar(Scalar::I32(slot as i32)),
                                ],
                            )?
                            .ok_or_else(|| unsupported("implicit definition missing"))?;
                        let minimum: f32 = self.primitive(&data, c"implicitValue")?;
                        let maximum: f32 = self.primitive(&data, c"implicitMaxValue")?;
                        if !minimum.is_finite() || !maximum.is_finite() {
                            return Err(unsupported("implicit nonfinite range"));
                        }
                        let info = self.description_call(
                            attached,
                            api,
                            build,
                            None,
                            &mut [
                                Argument::Reference(Some(&graph.item)),
                                Argument::Reference(Some(implicit)),
                                Argument::Reference(Some(&data)),
                            ],
                        )?;
                        let Some(info) = info else {
                            continue;
                        };
                        modifiers.push(ModifierSnapshot {
                            index: slot as u32,
                            text: self.description_text(attached, api, &info, c"ToModifierString")?,
                            range: self.description_text(attached, api, &info, c"ToRangeString")?,
                            roll_ids: if slot < request.expected().implicit_rolls.len() && minimum != maximum {
                                vec![slot as u8]
                            } else {
                                vec![]
                            },
                            editable: true,
                        });
                    }
                }
            }
            // Calls may run managed callbacks. Never publish text for an item or
            // graph that changed while producing it, even if its roots survived.
            let current = self.forge_graph(attached)?.ok_or(DiscoveryError::NotReady("forge item removed during formatting"))?;
            if target(&current.manager)?.as_ptr() as u64 != request.manager() || self.forge_snapshot(attached, &current)? != *request.expected() {
                return Err(unsupported("forge changed during implicit description"));
            }
            eg_trainer_wire::mapping::encode_modifiers(&modifiers).map_err(|e| unsupported(e.to_string()))?;
            Ok(modifiers)
        }
    }
}
