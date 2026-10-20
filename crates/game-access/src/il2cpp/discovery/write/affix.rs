//! Game-specific affix list edits: construct, validate, then rewrite through List<T>'s barriers.

use super::*;
use crate::{
    forge::AffixSnapshot,
    il2cpp::ValueTypeContract,
    write::{affix_order_key, layout_problems},
};

pub(super) struct AffixRewrite<'a, 'r> {
    list: Root<'a, 'r>,
    // All old objects remain rooted across Clear, Add, refresh, GC and exceptions.
    _original: Vec<Root<'a, 'r>>,
    next: Vec<Root<'a, 'r>>,
    list_class: NonNull<raw::Class>,
    clear: super::super::Method,
    add: super::super::Method,
    flags: Vec<(NonNull<raw::FieldInfo>, u8)>,
}

impl RuntimeAccess {
    /// The parent writer supplies its runtime/graph/exclusive-write contract.
    pub(super) unsafe fn prepare_affix<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        graph: &ForgeGraph<'_, '_>,
        api: raw::InvocationApi,
        expected: &ForgeSnapshot,
        mutation: ForgeMutation,
    ) -> Result<AffixRewrite<'a, 'r>, ForgeWriteError> {
        // SAFETY: inherits edit_forge's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let (slot, construction) = match mutation {
                ForgeMutation::Affix { slot, id, tier, roll, seal, .. } => {
                    if usize::from(slot) >= expected.affixes.len() {
                        return Err(ForgeWriteError::Refused("Invalid affix slot".into()));
                    }
                    (slot, Some((id, tier, roll, seal)))
                }
                ForgeMutation::AddAffix { id, tier, roll, seal, .. } => {
                    if expected.affixes.len() >= 32 {
                        return Err(ForgeWriteError::Refused("Affix list is full".into()));
                    }
                    (expected.affixes.len() as u8, Some((id, tier, roll, seal)))
                }
                ForgeMutation::RemoveAffix { slot, id } => {
                    if expected.affixes.get(usize::from(slot)).is_none_or(|a| a.id != id) {
                        return Err(ForgeWriteError::Refused("That affix is no longer in that slot".into()));
                    }
                    (slot, None)
                }
                _ => unreachable!(),
            };
            let class = self.class(&graph.images, c"", c"ItemAffix")?;
            let list = self
                .catalog_reference(attached, &graph.item, c"affixes")?
                .ok_or_else(|| ForgeWriteError::Refused("Missing affix list".into()))?;
            // Validates exact closed corlib List<ItemAffix> storage and roots every element.
            let original = self.catalog_list(attached, &list, class, 32)?;
            if original.len() != expected.affixes.len() {
                return Err(ForgeWriteError::Stale("Affix count changed".into()));
            }
            let list_class = (self.object_class)(target(&list)?.as_ptr());
            let clear = self.affix_list_method(list_class, class, c"Clear", false, api)?;
            let add = self.affix_list_method(list_class, class, c"Add", true, api)?;
            let list_class = NonNull::new(list_class).ok_or_else(|| ForgeWriteError::Refused("Missing affix-list class".into()))?;
            let context = InvocationContext::new(attached, api);
            // Finish checked binding before the live mutation boundary. Metadata
            // stays immutable through apply, which repeats binding before Clear.
            for method in [clear, add] {
                context
                    .bind_with_closed_references(NonNull::new(method.cast_mut()).expect("resolved list method"), &[list_class])
                    .map_err(|e| ForgeWriteError::Refused(e.to_string()))?;
            }
            let owner = self.class(&graph.images, c"", c"ItemData")?;
            let flag_fields = [c"hasSealedRegularAffix", c"hasSealedPrimordialAffix", c"hasSealedAffixFromCorruption"]
                .map(|name| self.mutable_field(owner, name, c"Boolean", 0x02))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?;
            let replacement = if let Some((id, tier, roll, seal)) = construction {
                self.validate_affix_definition(attached, graph, id, u16::from(tier))?;
                if seal > 3 {
                    return Err(ForgeWriteError::Refused("Invalid affix seal".into()));
                }
                let constructor = self.method(class, c".ctor", &["System.UInt16", "System.Byte", "System.Byte", "System.Byte", "SealedAffixType"], true)?;
                let enum_class = NonNull::new(self.field_class(self.field(class, c"sealedAffixType")?)?).ok_or_else(|| ForgeWriteError::Refused("Missing seal enum".into()))?;
                let context = InvocationContext::new(attached, api);
                let constructor = context
                    .bind_with_value_types(
                        NonNull::new(constructor.cast_mut()).expect("resolved constructor"),
                        &[ValueTypeContract::I32Enum { class: enum_class }],
                    )
                    .map_err(|e| ForgeWriteError::Refused(e.to_string()))?;
                // Root before any further runtime call, including constructor invocation.
                let object = NonNull::new((self.object_new)(class)).ok_or_else(|| ForgeWriteError::Refused("Cannot allocate ItemAffix".into()))?;
                let replacement = attached.root(object).map_err(DiscoveryError::from)?;
                let result = constructor
                    .invoke(
                        Some(&replacement),
                        &mut [
                            Argument::Scalar(Scalar::U16(id)),
                            Argument::Scalar(Scalar::U8(tier - 1)),
                            Argument::Scalar(Scalar::U8(roll)),
                            Argument::Scalar(Scalar::U8(expected.item_type)),
                            Argument::EnumI32 {
                                class: enum_class,
                                value: i32::from(seal),
                            },
                        ],
                    )
                    .map_err(|e| ForgeWriteError::Refused(e.to_string()))?;
                if !matches!(result, InvocationOutcome::Returned(ReturnValue::Void)) {
                    return Err(ForgeWriteError::Refused("Affix constructor raised an exception".into()));
                }
                self.exact(&replacement, class)?;
                Some(replacement)
            } else {
                None
            };
            let mut values = expected.affixes.clone();
            if let Some(replacement) = &replacement {
                let value = AffixSnapshot {
                    slot,
                    id: self.primitive(replacement, c"affixId")?,
                    tier: u16::from(self.primitive::<u8>(replacement, c"affixTier")?) + 1,
                    roll: self.primitive(replacement, c"affixRoll")?,
                    kind: self.enumeration(replacement, c"affixType", c"AffixType")?,
                    seal: self.enumeration(replacement, c"sealedAffixType", c"SealedAffixType")?,
                    special: self.enumeration(replacement, c"specialAffixType", c"SpecialAffixType")?,
                    name: self.text(attached, replacement, c"affixName")?,
                };
                if value.id > 4095 || !(1..=8).contains(&value.tier) || !(0..=3).contains(&value.seal) {
                    return Err(ForgeWriteError::Refused("Constructor returned an unserializable affix".into()));
                }
                if usize::from(slot) == values.len() {
                    values.push(value);
                } else {
                    values[usize::from(slot)] = value;
                }
            } else {
                values.remove(usize::from(slot));
            }
            let old = layout_problems(expected, &expected.affixes);
            if layout_problems(expected, &values).iter().any(|p| !old.contains(p)) {
                return Err(ForgeWriteError::Refused("Affix edit introduces an invalid seal layout".into()));
            }
            values.sort_by_key(|a| affix_order_key(expected.rarity, a.seal));
            let flags = flag_fields
                .into_iter()
                .zip([1, 2, 3])
                .map(|(field, seal)| (field, u8::from(values.iter().any(|a| a.seal == seal))))
                .collect();
            let mut next = Vec::with_capacity(original.len());
            for value in &values {
                let root = match &replacement {
                    Some(replacement) if value.slot == slot => replacement,
                    _ => &original[usize::from(value.slot)],
                };
                next.push(attached.root(target(root)?).map_err(DiscoveryError::from)?);
            }
            Ok(AffixRewrite {
                list,
                _original: original,
                next,
                list_class,
                clear,
                add,
                flags,
            })
        }
    }

    // Game-specific discovery for these two closed List<ItemAffix> methods.
    // CheckedMethod validates the receiver/arguments at each invocation too.
    // Interface implementations may be final virtual; the receiver is the exact closed List.
    pub(in super::super) unsafe fn affix_list_method(
        &self,
        class: Class,
        element: Class,
        name: &CStr,
        takes_element: bool,
        api: raw::InvocationApi,
    ) -> Result<super::super::Method, ForgeWriteError> {
        // SAFETY: inherits edit_forge's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let mut iterator = std::ptr::null_mut();
            let mut found = None;
            for _ in 0..100_000 {
                let method = (self.class_methods)(class, &mut iterator);
                if method.is_null() {
                    return found.ok_or_else(|| ForgeWriteError::Refused("Missing closed affix-list method".into()));
                }
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(ForgeWriteError::Refused("Missing list method name".into()));
                }
                if CStr::from_ptr(actual) != name || (self.method_params)(method) != u32::from(takes_element) {
                    continue;
                }
                let returns = (self.method_return)(method);
                let mut implementation_flags = 0;
                if found.is_some()
                    || !(self.method_instance)(method)
                    || (self.method_generic)(method)
                    || (api.method_get_class)(method) != class
                    || returns.is_null()
                    || (self.type_byref)(returns)
                    || (self.type_code)(returns) != 1
                    || (api.method_get_flags)(method, &mut implementation_flags) & 0x2400 != 0
                {
                    return Err(ForgeWriteError::Refused("Affix-list method signature differs".into()));
                }
                if takes_element {
                    let parameter = (self.method_param)(method, 0);
                    if parameter.is_null() || (self.type_byref)(parameter) || (self.type_code)(parameter) != 0x12 || (self.type_class)(parameter) != element {
                        return Err(ForgeWriteError::Refused("Affix-list Add parameter differs".into()));
                    }
                }
                found = Some(method);
            }
            Err(ForgeWriteError::Refused("Affix-list method enumeration limit".into()))
        }
    }
}

impl AffixRewrite<'_, '_> {
    /// Caller crossed the mutation boundary: any error is uncertain, even from Clear.
    pub(super) unsafe fn apply(
        &self,
        attached: &AttachedThread<'_>,
        writes: raw::FieldWriteApi,
        api: raw::InvocationApi,
        item: NonNull<raw::Object>,
    ) -> Result<(), ForgeWriteError> {
        // SAFETY: inherits edit_forge's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let context = InvocationContext::new(attached, api);
            let failed = |e: crate::il2cpp::InvokeError| ForgeWriteError::Failed(e.to_string());
            let clear = context
                .bind_with_closed_references(NonNull::new(self.clear.cast_mut()).expect("prepared Clear"), &[self.list_class])
                .map_err(failed)?;
            let add = context
                .bind_with_closed_references(NonNull::new(self.add.cast_mut()).expect("prepared Add"), &[self.list_class])
                .map_err(failed)?;
            if !matches!(clear.invoke(Some(&self.list), &mut []).map_err(failed)?, InvocationOutcome::Returned(ReturnValue::Void)) {
                return Err(ForgeWriteError::Failed("Affix-list Clear raised an exception".into()));
            }
            for root in &self.next {
                if !matches!(
                    add.invoke(Some(&self.list), &mut [Argument::Reference(Some(root))]).map_err(failed)?,
                    InvocationOutcome::Returned(ReturnValue::Void)
                ) {
                    return Err(ForgeWriteError::Failed("Affix-list Add raised an exception".into()));
                }
            }
            // Managed Clear/Add perform all reference stores with runtime barriers.
            for &(field, value) in &self.flags {
                writes.set_u8(item, field, value);
            }
            Ok(())
        }
    }
}
