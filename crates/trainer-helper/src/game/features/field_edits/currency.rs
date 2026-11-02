//! Currency changes use the game's delta methods so its limits, UI updates and saves still run.
use super::*;
use eg_trainer_wire::field_edits::Field;
use il2cpp_runtime::Argument;

impl RuntimeAccess {
    pub(super) unsafe fn currency_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, api: raw::InvocationApi, manager: Root<'a, 'r>) -> Result<Counts<'a, 'r>, WriteError> {
        // SAFETY: the current manager and all returned components stay rooted throughout their metadata reads/getter.
        unsafe {
            let actor = self.count_child(attached, &manager, c"actor")?;
            let gold = self.count_child(attached, &actor, c"goldTracker")?;
            let dungeon = self.count_child(attached, &actor, c"dungeonRunManager")?;
            let bones = self.count_child(attached, &actor, c"ancientBonesTracker")?;
            let context = InvocationContext::new(attached, api);
            let get = self.count_call(&context, api, &dungeon, c"GetSoulEmbers", 0)?;
            let embers = match returned(get.invoke(Some(&dungeon), &mut []))? {
                ReturnValue::Scalar { value: Scalar::I32(value), .. } => value,
                _ => return Err(WriteError::Refused("Unexpected soul embers result".into())),
            };
            let entries = vec![gold, dungeon, bones];
            let values = [self.primitive::<i32>(&entries[0], c"value")?, embers, self.primitive::<i32>(&entries[2], c"value")?];
            let fields = entries
                .iter()
                .zip(values)
                .enumerate()
                .map(|(index, (root, value))| {
                    Ok(Field {
                        key: index as u64,
                        identity: target(root)?.as_ptr() as u64,
                        item_type: 0,
                        sub_type: 0,
                        label: String::new(),
                        value,
                    })
                })
                .collect::<Result<Vec<_>, WriteError>>()?;
            let snapshot = Snapshot {
                set: FieldSet::Currency,
                manager: target(&manager)?.as_ptr() as u64,
                container: target(&actor)?.as_ptr() as u64,
                fields,
            };
            snapshot.encode().map_err(|e| WriteError::Refused(e.to_string()))?;
            Ok(Counts {
                _manager: manager,
                container: actor,
                entries,
                snapshot,
            })
        }
    }
    pub(super) unsafe fn apply_currency(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        graph: &Counts<'_, '_>,
        values: &[Option<i32>],
    ) -> Result<Snapshot, WriteError> {
        // SAFETY: all exact method signatures are bound before the first mutation; each target stays rooted.
        unsafe {
            let context = InvocationContext::new(attached, api);
            let names = [c"modifyGold", c"modifySoulEmberCountOnSingleplayerOrServer", c"modifyAncientBones"];
            let methods = graph
                .entries
                .iter()
                .zip(names)
                .enumerate()
                .map(|(index, (root, name))| self.count_call(&context, api, root, name, if index == 1 { 2 } else { 1 }))
                .collect::<Result<Vec<_>, _>>()?;
            self.count_gate(attached, graph.snapshot.manager)?;
            stored(
                || {
                    for (index, ((method, root), value)) in methods.iter().zip(&graph.entries).zip(values).enumerate() {
                        let Some(value) = value else {
                            continue;
                        };
                        let delta = *value - graph.snapshot.fields[index].value;
                        if delta == 0 {
                            continue;
                        }
                        let mut arguments = vec![Argument::Scalar(Scalar::I32(delta))];
                        if index == 1 {
                            arguments.push(Argument::Scalar(Scalar::Boolean(true)));
                        }
                        let result = returned(method.invoke(Some(root), &mut arguments))?;
                        if index == 1 {
                            if !matches!(result, ReturnValue::Void) {
                                return Err(WriteError::Refused("Unexpected soul ember change result".into()));
                            }
                        } else if !boolean(result)? {
                            return Err(WriteError::Refused("The game refused that currency change".into()));
                        }
                        self.count_gate(attached, graph.snapshot.manager)?;
                    }
                    Ok(())
                },
                || self.read_counts(attached, api, graph.snapshot.manager, graph.snapshot.set),
            )
        }
    }
}
