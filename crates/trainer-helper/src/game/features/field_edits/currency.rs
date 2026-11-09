//! Currency changes use the game's delta methods so its limits, UI updates and saves still run.
use super::*;
use il2cpp_runtime::Argument;

impl RuntimeAccess {
    pub(super) unsafe fn currency_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: Root<'a, 'r>) -> Result<FieldGraph<'a, 'r>, WriteError> {
        // SAFETY: the current manager and all returned components stay rooted throughout their metadata reads/getter.
        unsafe {
            let actor = self.field_child(attached, &manager, c"actor")?;
            let gold = self.field_child(attached, &actor, c"goldTracker")?;
            let dungeon = self.field_child(attached, &actor, c"dungeonRunManager")?;
            let bones = self.field_child(attached, &actor, c"ancientBonesTracker")?;
            let context = InvocationContext::new(attached);
            let get = self.field_call(&context, &dungeon, c"GetSoulEmbers", 0)?;
            let embers = match returned(get.invoke(Some(&dungeon), &mut []))? {
                ReturnValue::Scalar { value: Scalar::I32(value), .. } => value,
                _ => return Err(WriteError::Refused("Unexpected soul embers result".into())),
            };
            let entries = vec![gold, dungeon, bones];
            let values = [self.primitive::<i32>(&entries[0], c"value")?, embers, self.primitive::<i32>(&entries[2], c"value")?];
            let values = entries
                .iter()
                .zip(values)
                .map(|(root, value)| Ok((target(root)?.as_ptr() as u64, value)))
                .collect::<Result<Vec<_>, WriteError>>()?;
            let snapshot = fixed_snapshot(FieldSet::Currency, &manager, &actor, values)?;
            Ok(FieldGraph {
                _manager: manager,
                container: actor,
                entries,
                snapshot,
            })
        }
    }
    pub(super) unsafe fn apply_currency(&self, attached: &AttachedThread<'_>, graph: &FieldGraph<'_, '_>, values: &[Option<i32>]) -> Result<Snapshot, WriteError> {
        // SAFETY: all exact method signatures are bound before the first mutation; each target stays rooted.
        unsafe {
            let context = InvocationContext::new(attached);
            let names = [c"modifyGold", c"modifySoulEmberCountOnSingleplayerOrServer", c"modifyAncientBones"];
            let methods = graph
                .entries
                .iter()
                .zip(names)
                .enumerate()
                .map(|(index, (root, name))| self.field_call(&context, root, name, if index == 1 { 2 } else { 1 }))
                .collect::<Result<Vec<_>, _>>()?;
            self.field_gate(attached, graph.snapshot.manager)?;
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
                        self.field_gate(attached, graph.snapshot.manager)?;
                    }
                    Ok(())
                },
                || self.read_field_set(attached, graph.snapshot.manager, graph.snapshot.set),
            )
        }
    }
}
