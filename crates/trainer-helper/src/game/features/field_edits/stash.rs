//! Keys and woven echoes share traversal and quantity semantics. Zero removes the current entry through TryRemoveItem;
//! positive quantities use its setter. Entry/data/type identities and all counts are compared before any change.
use super::*;
use eg_trainer_wire::field_edits::{Field, MAX_FIELDS};
use il2cpp_runtime::Argument;

impl RuntimeAccess {
    pub(super) unsafe fn stash_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: Root<'a, 'r>, set: FieldSet) -> Result<FieldGraph<'a, 'r>, WriteError> {
        // SAFETY: current rooted manager, inherited holder getter and exact current tab; all returned objects stay rooted.
        unsafe {
            let context = InvocationContext::new(attached);
            let holder = self.field_child(attached, &manager, c"stash")?;
            let active = self.field_call(&context, &holder, c"get_ActiveContainer", 0)?;
            let stash = reference(returned(active.invoke(Some(&holder), &mut []))?)?;
            let getter = if set == FieldSet::Keys { c"get_Keys" } else { c"get_WovenEchoes" };
            let tab = reference(returned(self.field_call(&context, &stash, getter, 0)?.invoke(Some(&stash), &mut []))?)?;
            let images = self.images()?;
            let entry_class = self.class(&images, c"", c"ItemContainerEntry")?;
            let method = self.field_method((self.object_class)(target(&tab)?.as_ptr()), c"GetContent", 0)?;
            let list_class = (self.type_class)((self.method_return)(method.as_ptr()));
            self.list_class(list_class)?;
            self.array_class(self.field_class(self.field(list_class, c"_items")?, "stash entries")?, entry_class)?;
            let mut closed = vec![NonNull::new(list_class).ok_or_else(|| WriteError::Refused("Missing entry List".into()))?];
            let owner = (self.method_class)(method.as_ptr());
            if (self.class_inflated)(owner) {
                closed.push(NonNull::new(owner).unwrap());
            }
            let content = context.bind_with_closed_references(method, &closed).map_err(|e| WriteError::Refused(e.to_string()))?;
            let list = reference(returned(content.invoke(Some(&tab), &mut []))?)?;
            let entries = self.list(attached, &list, entry_class, MAX_FIELDS as u32)?;
            let item_list = self.class(&images, c"", c"ItemList")?;
            let get_items = context.bind(self.field_method(item_list, c"get", 0)?).map_err(|e| WriteError::Refused(e.to_string()))?;
            let items = reference(returned(get_items.invoke(None, &mut []))?)?;
            let display = self.field_call(&context, &items, c"GetItemDisplayName", 2)?;
            let string = self.system_class(c"String")?;
            let mut rows = Vec::new();
            for entry in entries {
                let Some(data) = self.child(attached, &entry, c"data")? else {
                    continue;
                };
                let item_type = u16::from(self.primitive::<u8>(&data, c"itemType")?);
                let sub_type = self.primitive::<u16>(&data, c"subType")?;
                let expected_type = if set == FieldSet::Keys { 104 } else { 107 };
                if item_type != expected_type {
                    return Err(WriteError::Refused("Unexpected item type in the stash tab".into()));
                }
                let name = match returned(display.invoke(
                    Some(&items),
                    &mut [Argument::Scalar(Scalar::I32(item_type.into())), Argument::Scalar(Scalar::I32(sub_type.into()))],
                ))? {
                    ReturnValue::Reference(name) => name,
                    _ => return Err(WriteError::Refused("Unexpected item name result".into())),
                };
                let label = self.owned_text(name.as_ref(), string, 512)?;
                let field = Field {
                    key: target(&entry)?.as_ptr() as u64,
                    identity: target(&data)?.as_ptr() as u64,
                    item_type,
                    sub_type,
                    label,
                    value: self.primitive::<i32>(&entry, c"quantity")?,
                };
                rows.push((field, entry));
            }
            rows.sort_by_key(|(field, _)| field.sub_type);
            let names: Vec<_> = rows.iter().map(|(field, _)| field.label.clone()).collect();
            for (field, _) in &mut rows {
                if field.label.is_empty() || names.iter().filter(|name| **name == field.label).count() > 1 {
                    field.label = if field.label.is_empty() {
                        format!("#{}", field.sub_type)
                    } else {
                        format!("{} #{}", field.label, field.sub_type)
                    };
                }
            }
            let (fields, entries) = rows.into_iter().unzip();
            let snapshot = Snapshot {
                set,
                manager: target(&manager)?.as_ptr() as u64,
                container: target(&tab)?.as_ptr() as u64,
                fields,
            };
            snapshot.encode().map_err(|e| WriteError::Refused(e.to_string()))?;
            Ok(FieldGraph {
                _manager: manager,
                container: tab,
                entries,
                snapshot,
            })
        }
    }
    pub(super) unsafe fn apply_stash(&self, attached: &AttachedThread<'_>, graph: &FieldGraph<'_, '_>, values: &[Option<i32>]) -> Result<Snapshot, WriteError> {
        // SAFETY: retained entries and tab; every setter/removal signature and permission is checked before stores.
        unsafe {
            let context = InvocationContext::new(attached);
            let setters = graph
                .entries
                .iter()
                .map(|entry| self.field_call(&context, entry, c"set_Quantity", 1))
                .collect::<Result<Vec<_>, _>>()?;
            let removing = values.contains(&Some(0));
            let remove = if removing {
                if !boolean(returned(
                    self.field_call(&context, &graph.container, c"get_PlayerCanRemove", 0)?
                        .invoke(Some(&graph.container), &mut []),
                )?)? {
                    return Err(WriteError::Refused("The game would not take anything out of this stash tab".into()));
                }
                let method = self.field_method((self.object_class)(target(&graph.container)?.as_ptr()), c"TryRemoveItem", 3)?;
                let class = NonNull::new((self.type_class)((self.method_param)(method.as_ptr(), 2))).ok_or_else(|| WriteError::Refused("Missing removal context".into()))?;
                let method = context
                    .bind_with_value_types(method, &[ValueTypeContract::I32Enum { class }])
                    .map_err(|e| WriteError::Refused(e.to_string()))?;
                Some((method, class))
            } else {
                None
            };
            self.field_gate(attached, graph.snapshot.manager)?;
            stored(
                || {
                    for (index, (entry, value)) in graph.entries.iter().zip(values).enumerate() {
                        let Some(value) = value else {
                            continue;
                        };
                        let before = graph.snapshot.fields[index].value;
                        if *value == before {
                            continue;
                        }
                        if *value == 0 {
                            let (remove, class) = remove.as_ref().unwrap();
                            if !boolean(returned(remove.invoke(
                                Some(&graph.container),
                                &mut [
                                    Argument::Reference(Some(entry)),
                                    Argument::Scalar(Scalar::I32(before)),
                                    Argument::EnumI32 { class: *class, value: 0 },
                                ],
                            ))?)? {
                                return Err(WriteError::Refused("The game did not remove the stash entry".into()));
                            }
                        } else if !matches!(
                            returned(setters[index].invoke(Some(entry), &mut [Argument::Scalar(Scalar::I32(*value))]))?,
                            ReturnValue::Void
                        ) {
                            return Err(WriteError::Refused("Unexpected quantity setter result".into()));
                        }
                        self.field_gate(attached, graph.snapshot.manager)?;
                    }
                    Ok(())
                },
                || self.read_field_set(attached, graph.snapshot.manager, graph.snapshot.set),
            )
        }
    }
}
