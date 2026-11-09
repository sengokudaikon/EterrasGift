//! Earned passive and Weaver points through the trees' own UInt16 getters/setters. Validate both requested totals
//! against freshly read allocated-point floors before either setter runs. Progression/load may recalculate totals;
//! these are one-time edits, independent of the unlimited-points hooks.
use super::*;
use il2cpp_runtime::Argument;

fn setter(name: &'static CStr) -> MethodSpec {
    MethodSpec {
        class: c"tree",
        method: name,
        instance: true,
        params: &["System.UInt16"],
        returns: "System.Void",
    }
}
fn points(value: i32, allocated: i32) -> Result<u16, WriteError> {
    if value < allocated {
        return Err(WriteError::Refused(format!("Points earned cannot be fewer than the {allocated} allocated")));
    }
    u16::try_from(value).map_err(|_| WriteError::Refused("Points earned are from 0 to 65535".into()))
}
impl RuntimeAccess {
    pub(super) unsafe fn points_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: Root<'a, 'r>) -> Result<FieldGraph<'a, 'r>, WriteError> {
        // SAFETY: rooted current actor/trees and checked getter invocation; returned trees are rooted immediately.
        unsafe {
            let actor = self.field_child(attached, &manager, c"actor")?;
            let trees = self.field_child(attached, &actor, c"localTreeData")?;
            let context = InvocationContext::new(attached);
            let mut entries = Vec::new();
            let mut values = Vec::new();
            for (get_tree, get_points) in [(c"getPassiveTreeData", c"getPointsEarnt"), (c"getWeaverTreeData", c"get_EarnedWeaverPoints")] {
                let tree = reference(returned(self.field_call(&context, &trees, get_tree, 0)?.invoke(Some(&trees), &mut []))?)?;
                let value = match returned(self.field_call(&context, &tree, get_points, 0)?.invoke(Some(&tree), &mut []))? {
                    ReturnValue::Scalar { value: Scalar::U16(value), .. } => i32::from(value),
                    _ => return Err(WriteError::Refused("Unexpected earned points result".into())),
                };
                values.push((target(&tree)?.as_ptr() as u64, value));
                entries.push(tree);
            }
            let snapshot = fixed_snapshot(FieldSet::PointsEarned, &manager, &trees, values)?;
            Ok(FieldGraph {
                _manager: manager,
                container: trees,
                entries,
                snapshot,
            })
        }
    }
    pub(super) unsafe fn apply_points(&self, attached: &AttachedThread<'_>, graph: &FieldGraph<'_, '_>, values: &[Option<i32>]) -> Result<Snapshot, WriteError> {
        // SAFETY: exact setters and all allocation floors are prepared before the first store; tree roots span calls.
        unsafe {
            let context = InvocationContext::new(attached);
            let mut changes = Vec::new();
            for (index, name) in [c"setPointsEarnt", c"set_EarnedWeaverPoints"].into_iter().enumerate() {
                let Some(value) = values[index].filter(|value| *value != graph.snapshot.fields[index].value) else {
                    continue;
                };
                let tree = &graph.entries[index];
                let allocated = integer(returned(self.field_call(&context, tree, c"numberOfPointsAllocated", 0)?.invoke(Some(tree), &mut []))?)?;
                let value = points(value, allocated)?;
                changes.push((tree, self.field_exact(&context, tree, &setter(name))?, value));
            }
            self.field_gate(attached, graph.snapshot.manager)?;
            stored(
                || {
                    for (tree, setter, value) in changes {
                        void(returned(setter.invoke(Some(tree), &mut [Argument::Scalar(Scalar::U16(value))]))?)?;
                        self.field_gate(attached, graph.snapshot.manager)?;
                    }
                    Ok(())
                },
                || self.read_field_set(attached, graph.snapshot.manager, graph.snapshot.set),
            )
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocated_points_are_a_floor_and_uint16_is_the_storage_limit() {
        assert_eq!(points(20, 20), Ok(20));
        assert_eq!(points(65535, 20), Ok(65535));
        assert_eq!(points(0, 0), Ok(0));
        for (value, allocated) in [(19, 20), (65535, 65536), (65536, 20), (-1, 0)] {
            assert!(points(value, allocated).is_err());
        }
    }
}
