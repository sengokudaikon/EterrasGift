//! Unique tooltip lines and private-copy draft previews, using the game's producers.
use crate::game::metadata_text;
use crate::game::{GameError, Images, Root, RuntimeAccess, target, unsupported};
use eg_trainer_wire::mapping::{ForgeDescriptionRequest, MAX_MODIFIERS, ModifierSnapshot, UniquePreviewRequest};
use il2cpp_runtime::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, ValueTypeContract, raw};
use std::{collections::BTreeSet, ffi::CStr, ptr::NonNull};

impl RuntimeAccess {
    // Inherits describe_unique's attached, synchronous supported-runtime span.
    pub(super) unsafe fn unique_scalar(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        object: &Root<'_, '_>,
        name: &CStr,
        returns: &CStr,
    ) -> Result<Scalar, GameError> {
        // SAFETY: The rooted modifier and checked zero-argument getter retain the exact boxed scalar class through unboxing.
        unsafe {
            let method = self.mapping_method(api, object, name, &[], Some(self.system_class(returns)?))?;
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind(NonNull::new(method.cast_mut()).ok_or_else(|| unsupported("unique scalar method"))?)
                .map_err(|e| unsupported(e.to_string()))?;
            match method.invoke(Some(object), &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Scalar { value, .. }) => Ok(value),
                _ => Err(unsupported("unique scalar getter failed or threw")),
            }
        }
    }
    // The preformatted producer returns a closed List<game reference>. Validate
    // its layout/element before opting this exact class into checked binding.
    pub(super) unsafe fn unique_replacements<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        images: &Images,
        parsed: &Root<'_, '_>,
    ) -> Result<Vec<Root<'a, 'r>>, GameError> {
        // SAFETY: The validated property definition remains rooted while its exact List and boxed replacement entries are inspected.
        unsafe {
            let method = self.mapping_method(api, parsed, c"get_Replacements", &[], None)?;
            let list_class = (self.type_class)((self.method_return)(method));
            self.list_class(list_class)?;
            let array = self.field_class(self.field(list_class, c"_items")?, "unique")?;
            let element = (self.class_element)(array);
            if element.is_null() || (self.class_value)(element) || (self.class_image)(element) != images.game {
                return Err(unsupported("unique replacement element"));
            }
            self.array_class(array, element)?;
            let context = InvocationContext::new(attached, api);
            let method = context
                .bind_with_closed_references(
                    NonNull::new(method.cast_mut()).ok_or_else(|| unsupported("unique replacement getter"))?,
                    &[NonNull::new(list_class).ok_or_else(|| unsupported("unique replacement list"))?],
                )
                .map_err(|e| unsupported(e.to_string()))?;
            let list = match method.invoke(Some(parsed), &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(list))) => list,
                _ => return Err(unsupported("unique replacement getter threw or returned no list")),
            };
            self.exact(&list, list_class)?;
            self.list(attached, &list, element, MAX_MODIFIERS as u32)
        }
    }
    /// The game's unique tooltip for an item, line by line: each tooltip entry with what `GetUniqueModifierInfo` builds for
    /// it (nothing where the line shows nothing), and the definition's modifiers the plain lines read.
    /// # Safety
    /// As for [`Self::describe_unique`]; `item` may be the selected item or a detached copy of it.
    pub(super) unsafe fn unique_lines<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        images: &Images,
        item: &Root<'a, 'r>,
        definition: &Root<'a, 'r>,
    ) -> Result<UniqueLines<'a, 'r>, GameError> {
        // SAFETY: The item, definition and validated builder stay rooted across calls; every list is bounded before copying.
        unsafe {
            let entry_class = (self.object_class)(target(definition)?.as_ptr());
            let tooltip_class = self.class(images, c"", c"UniqueModDisplayListEntry")?;
            let mod_class = self.class(images, c"", c"UniqueItemMod")?;
            let entries = self
                .child(attached, definition, c"tooltipEntries")?
                .ok_or_else(|| unsupported("unique tooltip entries absent"))?;
            let mods = self.child(attached, definition, c"mods")?.ok_or_else(|| unsupported("unique modifiers absent"))?;
            let entries = self.list(attached, &entries, tooltip_class, MAX_MODIFIERS as u32)?;
            let mods = self.list(attached, &mods, mod_class, MAX_MODIFIERS as u32)?;
            let builder = self.class(images, c"LE.UI.Tooltips", c"ItemTooltipContentBuilder")?;
            let mut slot_type = None;
            for method in self.methods(builder) {
                let method = method?;
                let name = (self.method_name)(method);
                if !name.is_null() && metadata_text(name) == c"GetUniqueModifierInfo" && (self.method_params)(method) == 4 {
                    if slot_type.is_some() {
                        return Err(unsupported("ambiguous unique tooltip builder"));
                    }
                    slot_type = NonNull::new((self.type_class)((self.method_param)(method, 3)));
                }
            }
            let slot_type = slot_type.ok_or_else(|| unsupported("unique tooltip slot enum"))?;
            let builder = self.description_method(
                api,
                builder,
                c"GetUniqueModifierInfo",
                &[self.class(images, c"", c"ItemDataUnpacked")?, entry_class, tooltip_class, slot_type.as_ptr()],
                None,
                false,
            )?;
            let context = InvocationContext::new(attached, api);
            let builder = context
                .bind_with_value_types(
                    NonNull::new(builder.cast_mut()).ok_or_else(|| unsupported("unique builder"))?,
                    &[ValueTypeContract::I32Enum { class: slot_type }],
                )
                .map_err(|e| unsupported(e.to_string()))?;
            let mut infos = Vec::new();
            for entry in &entries {
                infos.push(
                    match builder
                        .invoke(
                            None,
                            &mut [
                                Argument::Reference(Some(item)),
                                Argument::Reference(Some(definition)),
                                Argument::Reference(Some(entry)),
                                Argument::EnumI32 { class: slot_type, value: 0 },
                            ],
                        )
                        .map_err(|e| unsupported(e.to_string()))?
                    {
                        InvocationOutcome::Returned(ReturnValue::Reference(info)) => info,
                        _ => return Err(unsupported("unique tooltip producer failed or threw")),
                    },
                );
            }
            Ok(UniqueLines { entries, mods, infos })
        }
    }
    pub(super) unsafe fn unique_modifiers<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        images: &Images,
        item: &Root<'a, 'r>,
        definition: &Root<'a, 'r>,
        roll_count: usize,
    ) -> Result<Vec<ModifierSnapshot>, GameError> {
        // SAFETY: The item, definition and every tooltip result stay rooted across calls; owned text is bounded.
        unsafe {
            let lines = self.unique_lines(attached, api, images, item, definition)?;
            let mut result = Vec::new();
            for (index, (entry, info)) in lines.entries.iter().zip(&lines.infos).enumerate() {
                let Some(info) = info else {
                    continue;
                };
                let class = (self.object_class)(target(info)?.as_ptr());
                let name = (self.class_name)(class);
                if name.is_null() || (self.class_image)(class) != images.game {
                    return Err(unsupported("unique tooltip runtime class"));
                }
                let mut ids = BTreeSet::new();
                let mut editable = true;
                if metadata_text(name) == c"ItemTooltipPreformattedUniqueModifierInfo" {
                    let parsed = self
                        .child(attached, info, c"_parsedUniqueMod")?
                        .ok_or_else(|| unsupported("parsed unique modifier absent"))?;
                    for replacement in self.unique_replacements(attached, api, images, &parsed)? {
                        let class = (self.object_class)(target(&replacement)?.as_ptr());
                        let name = (self.class_name)(class);
                        if name.is_null() {
                            return Err(unsupported("unique replacement runtime class"));
                        }
                        if metadata_text(name) != c"UniqueModRollRangeReplacement" {
                            continue;
                        }
                        match self.unique_scalar(attached, api, &replacement, c"get_HasRange", c"Boolean")? {
                            Scalar::Boolean(true) => {
                                let Scalar::I32(id) = self.unique_scalar(attached, api, &replacement, c"get_RollId", c"Int32")? else {
                                    return Err(unsupported("unique roll ID getter"));
                                };
                                ids.insert(id);
                            }
                            Scalar::Boolean(false) => {}
                            _ => return Err(unsupported("unique range getter")),
                        }
                    }
                } else {
                    let ordinal = usize::from(self.primitive::<u8>(entry, c"modDisplay")?);
                    if let Some(modifier) = lines.mods.get(ordinal) {
                        let minimum = self.primitive::<f32>(modifier, c"value")?;
                        let maximum = self.primitive::<f32>(modifier, c"maxValue")?;
                        if !minimum.is_finite() || !maximum.is_finite() {
                            return Err(unsupported("unique nonfinite range"));
                        }
                        if self.boolean(modifier, c"canRoll")? && minimum != maximum {
                            ids.insert(i32::from(self.primitive::<u8>(modifier, c"rollID")?));
                        }
                    } else {
                        editable = false;
                    }
                }
                if ids.iter().any(|id| *id < 0 || *id as usize >= roll_count) {
                    editable = false;
                }
                result.push(ModifierSnapshot {
                    index: index as u32,
                    text: self.description_text(attached, api, info, c"ToModifierString")?,
                    range: self.description_text(attached, api, info, c"ToRangeString")?,
                    roll_ids: if editable { ids.into_iter().map(|id| id as u8).collect() } else { vec![] },
                    editable,
                });
            }
            eg_trainer_wire::mapping::encode_modifiers(&result).map_err(|e| unsupported(e.to_string()))?;
            Ok(result)
        }
    }
    /// Copies unique tooltip text and roll associations from the game's builder.
    /// # Safety
    /// The read_forge and describe_implicits runtime/graph/main-thread conditions
    /// apply to all producer, lookup, parsing-cache and virtual formatter calls.
    /// Game bodies and active culture must remain stable; roots cover GC, not
    /// concurrent external mutation or arbitrary producer side effects.
    pub unsafe fn describe_unique(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, request: &ForgeDescriptionRequest) -> Result<Vec<ModifierSnapshot>, GameError> {
        // SAFETY: The caller supplies the documented main-thread tooltip span; this path performs no item stores.
        unsafe { self.unique_presentation(attached, None, api, request, None) }
    }
    /// Formats drafted rolls on an independent item copy. No live item store.
    /// # Safety
    /// describe_unique's conditions plus mapping_copy's constructor/barrier and
    /// detached graph assumptions apply throughout this synchronous operation.
    pub unsafe fn preview_unique(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        request: &UniquePreviewRequest,
    ) -> Result<Vec<ModifierSnapshot>, GameError> {
        // SAFETY: The caller supplies the documented preview span; stores target only the validated independent copy.
        unsafe { self.unique_presentation(attached, Some(writes), api, request.target(), Some(request)) }
    }
    unsafe fn unique_presentation(
        &self,
        attached: &AttachedThread<'_>,
        writes: Option<raw::FieldWriteApi>,
        api: raw::InvocationApi,
        request: &ForgeDescriptionRequest,
        preview: Option<&UniquePreviewRequest>,
    ) -> Result<Vec<ModifierSnapshot>, GameError> {
        // SAFETY: The caller excludes graph changes; roots retain the selected item, private copy and tooltip results across calls.
        unsafe {
            let graph = self.forge_graph(attached)?.ok_or(GameError::NotReady("forge item absent"))?;
            if !self.forge_unchanged(attached, &graph, request.manager(), request.expected())? {
                return Err(unsupported("forge changed before unique description"));
            }
            if !matches!(request.expected().rarity, 7..=9) {
                return Err(unsupported("item has no supported unique modifiers"));
            }
            let identity = request.expected().identity.as_ref().ok_or_else(|| unsupported("unique identity absent"))?;
            let locale = self.read_game_locale(attached, api)?;
            let definition = self.unique_definition(attached, &graph, api, identity.id).map_err(|e| unsupported(e.to_string()))?;
            let original = self.unique_modifiers(attached, api, &graph.images, &graph.item, &definition, identity.rolls.len())?;
            let result = if let Some(preview) = preview {
                if preview
                    .changes()
                    .iter()
                    .any(|change| !original.iter().any(|line| line.editable && line.roll_ids.contains(&change.index)))
                {
                    return Err(unsupported("unique roll has no editable tooltip evidence"));
                }
                let copy = self.mapping_copy(attached, writes.ok_or_else(|| unsupported("preview setter API"))?, api, &graph)?;
                if self.unique_modifiers(attached, api, &graph.images, &copy, &definition, identity.rolls.len())? != original {
                    return Err(unsupported("detached unique context differs from selected item"));
                }
                let rolls = self.child(attached, &copy, c"uniqueRolls")?.ok_or_else(|| unsupported("detached unique rolls absent"))?;
                let count = self.array_count(&rolls, self.system_class(c"Byte")?, 8)? as usize;
                if count != identity.rolls.len() || preview.changes().iter().any(|change| usize::from(change.index) >= count) {
                    return Err(unsupported("detached unique roll count changed"));
                }
                let live_rolls = self.child(attached, &graph.item, c"uniqueRolls")?.ok_or_else(|| unsupported("live unique rolls absent"))?;
                if target(&rolls)? == target(&live_rolls)? || target(&copy)? == target(&graph.item)? {
                    return Err(unsupported("detached unique preview aliases live storage"));
                }
                for change in preview.changes() {
                    crate::game::write_bytes(target(&rolls)?.as_ptr().cast::<u8>().add(32 + usize::from(change.index)), &[change.value]);
                }
                self.unique_modifiers(attached, api, &graph.images, &copy, &definition, identity.rolls.len())?
            } else {
                original
            };
            let current = self.forge_graph(attached)?.ok_or(GameError::NotReady("forge removed during unique description"))?;
            if !self.forge_unchanged(attached, &current, request.manager(), request.expected())? || self.read_game_locale(attached, api)? != locale {
                return Err(unsupported("forge or locale changed during unique description"));
            }
            Ok(result)
        }
    }
}

/// The game's unique tooltip for an item, from [`RuntimeAccess::unique_lines`].
pub(super) struct UniqueLines<'a, 'r> {
    pub entries: Vec<Root<'a, 'r>>,
    pub mods: Vec<Root<'a, 'r>>,
    pub infos: Vec<Option<Root<'a, 'r>>>,
}
