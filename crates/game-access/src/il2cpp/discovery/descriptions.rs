//! Optional implicit text from the same game methods used by the original tooltip.
use super::{
    Class, DiscoveryError, Method, RuntimeAccess,
    forge::{Root, target},
    unsupported,
};
use crate::{
    il2cpp::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw},
    mapping::{ImplicitDescriptionRequest, MAX_TEXT_BYTES, ModifierSnapshot},
};
use std::{ffi::CStr, ptr::NonNull};

impl RuntimeAccess {
    // Exact supported-game call signatures. Closed List<T> is admitted only by the
    // get_Implicits adapter below, without broadening the shared invocation API.
    unsafe fn description_method(
        &self,
        api: raw::InvocationApi,
        owner: Class,
        name: &CStr,
        parameters: &[Class],
        returns: Option<Class>,
        instance: bool,
    ) -> Result<Method, DiscoveryError> {
        unsafe {
            let mut iterator = std::ptr::null_mut();
            let mut found = None;
            for _ in 0..100_000 {
                let method = (self.class_methods)(owner, &mut iterator);
                if method.is_null() {
                    return found.ok_or_else(|| unsupported(format!("missing tooltip method {}", name.to_string_lossy())));
                }
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("tooltip method name"));
                }
                if CStr::from_ptr(actual) != name || (self.method_params)(method) as usize != parameters.len() {
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
                    || !matches!((self.type_code)(result), 0x0e | 0x12 | 0x15)
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
            Err(unsupported("tooltip method enumeration bound"))
        }
    }
    unsafe fn description_call<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        method: Method,
        receiver: Option<&Root<'_, '_>>,
        args: &mut [Argument<'_, 'a, 'r>],
    ) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
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
    unsafe fn description_text(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, info: &Root<'_, '_>, name: &CStr) -> Result<String, DiscoveryError> {
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
                let mut iterator = std::ptr::null_mut();
                for _ in 0..100_000 {
                    let candidate = (self.class_methods)(owner, &mut iterator);
                    if candidate.is_null() {
                        break;
                    }
                    let actual = (self.method_name)(candidate);
                    if actual.is_null() {
                        return Err(unsupported("tooltip formatter name"));
                    }
                    if CStr::from_ptr(actual) == name && (self.method_params)(candidate) == 1 {
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
            let Some(text) = text else {
                return Ok(String::new());
            };
            self.exact(&text, string)?;
            let object = target(&text)?;
            let length = (self.string_length)(object.as_ptr());
            if !(0..=MAX_TEXT_BYTES as i32).contains(&length) {
                return Err(unsupported("tooltip text length"));
            }
            let chars = (self.string_chars)(object.as_ptr());
            if chars.is_null() && length != 0 {
                return Err(unsupported("tooltip characters"));
            }
            let units = (0..length as usize).map(|index| chars.add(index).read()).collect::<Vec<_>>();
            let text = String::from_utf16(&units).map_err(|_| unsupported("tooltip UTF-16"))?;
            if text.len() > MAX_TEXT_BYTES {
                return Err(unsupported("tooltip UTF-8 length"));
            }
            Ok(text)
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
        request: &ImplicitDescriptionRequest,
    ) -> Result<Vec<ModifierSnapshot>, DiscoveryError> {
        unsafe {
            let graph = self.forge_graph(attached)?.ok_or(DiscoveryError::NotReady("forge item absent"))?;
            if target(&graph.manager)?.as_ptr() as u64 != request.manager || self.forge_snapshot(attached, &graph)? != *request.expected {
                return Err(unsupported("forge changed before implicit description"));
            }
            let unpacked = self.class(&graph.images, c"", c"ItemDataUnpacked")?;
            let getter = self.description_method(api, unpacked, c"get_Implicits", &[], None, true)?;
            let list_class = (self.type_class)((self.method_return)(getter));
            if list_class.is_null()
                || (self.class_value)(list_class)
                || (self.type_code)((self.class_type)(list_class)) != 0x15
                || (self.class_image)(list_class) != (self.corlib)()
            {
                return Err(unsupported("implicit getter closed List return"));
            }
            let name = (self.class_name)(list_class);
            let namespace = (self.class_namespace)(list_class);
            if name.is_null() || namespace.is_null() || CStr::from_ptr(name) != c"List`1" || CStr::from_ptr(namespace) != c"System.Collections.Generic" {
                return Err(unsupported("implicit getter List identity"));
            }
            let array_class = self.field_class(self.field(list_class, c"_items")?)?;
            let element = (self.class_element)(array_class);
            if element.is_null() || (self.class_value)(element) || (self.class_image)(element) != graph.images.game {
                return Err(unsupported("implicit getter game reference element"));
            }
            self.array_class(array_class, element)?;
            // The only raw call: exact zero-argument instance getter returning a
            // validated closed List<game-reference>. Root result/exception immediately.
            let mut exception = std::ptr::null_mut();
            let value = (api.runtime_invoke)(getter, target(&graph.item)?.as_ptr().cast(), std::ptr::null_mut(), &mut exception);
            if let Some(exception) = NonNull::new(exception) {
                let _exception = attached.root(exception.cast())?;
                return Err(unsupported("implicit getter threw"));
            }
            let mut modifiers = Vec::new();
            if let Some(value) = NonNull::new(value) {
                let list = attached.root(value)?;
                self.exact(&list, list_class)?;
                let implicits = self.catalog_list(attached, &list, element, crate::catalog::MAX_IMPLICITS as u32)?;
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
                                    Argument::Scalar(Scalar::I32(i32::from(request.expected.item_type))),
                                    Argument::Scalar(Scalar::I32(i32::from(request.expected.sub_type))),
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
                            roll_ids: if slot < request.expected.implicit_rolls.len() && minimum != maximum {
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
            if target(&current.manager)?.as_ptr() as u64 != request.manager || self.forge_snapshot(attached, &current)? != *request.expected {
                return Err(unsupported("forge changed during implicit description"));
            }
            crate::mapping::encode_modifiers(&modifiers).map_err(|e| unsupported(e.to_string()))?;
            Ok(modifiers)
        }
    }
}
