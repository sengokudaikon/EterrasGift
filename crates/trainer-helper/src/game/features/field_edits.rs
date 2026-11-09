//! Main-thread numeric editors over rooted current player graphs. The read/apply transport and store boundary are
//! shared; each feature owns its lookup, validation and game calls. A store is never retried.
mod currency;
mod faction_standing;
mod monolith_run;
mod points_earned;
mod stash;
#[cfg(test)]
mod tests;
use crate::game::{
    apply::{WriteError, stored},
    *,
};
use eg_trainer_wire::field_edits::{Edit, Field, FieldSet, Snapshot};
use il2cpp_runtime::metadata::{MethodError, MethodQuery, MethodSpec};
use il2cpp_runtime::{AttachedThread, CheckedMethod, InvocationContext, InvocationOutcome, ReturnValue, Scalar, ValueTypeContract};

pub(crate) struct FieldGraph<'a, 'r> {
    _manager: Root<'a, 'r>,
    container: Root<'a, 'r>,
    entries: Vec<Root<'a, 'r>>,
    snapshot: Snapshot,
}

impl RuntimeAccess {
    /// # Safety
    /// Admitted attached main thread, initialized runtime and exclusive synchronous player graph, as read_forge.
    pub(crate) unsafe fn read_field_set(&self, attached: &AttachedThread<'_>, manager: u64, set: FieldSet) -> Result<Snapshot, WriteError> {
        // SAFETY: caller retains runtime and excludes graph mutation throughout the rooted read and final gate.
        unsafe {
            let graph = self.field_graph(attached, manager, set)?;
            self.field_gate(attached, manager)?;
            Ok(graph.snapshot)
        }
    }
    /// # Safety
    /// read_field_set contract plus permission for the feature's declared setters, field stores, saves and events.
    pub(crate) unsafe fn apply_field_set(&self, attached: &AttachedThread<'_>, writes: raw::FieldWriteApi, edit: &Edit) -> Result<Snapshot, WriteError> {
        // SAFETY: all destinations are prepared before the store boundary; rooted graph retained through each call.
        unsafe {
            edit.validate().map_err(|e| WriteError::Refused(e.to_string()))?;
            let graph = self.field_graph(attached, edit.expected.manager, edit.expected.set)?;
            if !graph.snapshot.same_values(&edit.expected) {
                return Err(WriteError::Stale("The game holds something else now; read again".into()));
            }
            if edit.values.iter().zip(&edit.expected.fields).all(|(v, field)| v.is_none_or(|v| v == field.value)) {
                return Ok(graph.snapshot);
            }
            match edit.expected.set {
                FieldSet::Currency => self.apply_currency(attached, &graph, &edit.values),
                FieldSet::Keys | FieldSet::WovenEchoes => self.apply_stash(attached, &graph, &edit.values),
                FieldSet::PointsEarned => self.apply_points(attached, &graph, &edit.values),
                FieldSet::MonolithRun => self.apply_run(attached, writes, &graph, &edit.values),
                FieldSet::FactionStanding => self.apply_standing(attached, &graph, &edit.values),
            }
        }
    }
    unsafe fn field_gate(&self, attached: &AttachedThread<'_>, expected: u64) -> Result<(), WriteError> {
        // SAFETY: same main-thread rooted graph span as the calling editor.
        unsafe {
            let status = self.status(attached)?;
            if status.online || status.manager == 0 {
                return Err(WriteError::Offline);
            }
            if status.manager != expected {
                return Err(WriteError::Stale("Character changed; read again".into()));
            }
            Ok(())
        }
    }
    unsafe fn field_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, manager: u64, set: FieldSet) -> Result<FieldGraph<'a, 'r>, WriteError> {
        // SAFETY: checked offline/live manager, then root its current static reference, never a caller-supplied address.
        unsafe {
            self.field_gate(attached, manager)?;
            let images = self.images()?;
            let owner = self.class(&images, c"", c"PlayerFinder")?;
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let root = self
                .static_object(attached, owner, c"localItemContainersManager", manager_class)?
                .ok_or_else(|| WriteError::NotReady("No character loaded".into()))?;
            if target(&root)?.as_ptr() as u64 != manager {
                return Err(WriteError::Stale("Character changed; read again".into()));
            }
            match set {
                FieldSet::Currency => self.currency_graph(attached, root),
                FieldSet::Keys | FieldSet::WovenEchoes => self.stash_graph(attached, root, set),
                FieldSet::PointsEarned => self.points_graph(attached, root),
                FieldSet::MonolithRun => self.run_graph(attached, root),
                FieldSet::FactionStanding => self.standing_graph(attached, root),
            }
        }
    }
    unsafe fn field_child<'a, 'r>(&self, attached: &'a AttachedThread<'r>, parent: &Root<'_, '_>, name: &CStr) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: rooted current parent, validated reference field, same exclusive graph span.
        unsafe {
            self.child(attached, parent, name)?
                .ok_or_else(|| WriteError::NotReady(format!("{} is still loading", name.to_string_lossy())))
        }
    }
    /// The method `name` with `arity` parameters on `owner` or the nearest parent that declares one. Inherited virtual
    /// methods are not dispatched: the current dynamic receiver's matching override wins, as in the original trainer.
    unsafe fn field_method(&self, owner: Class, name: &CStr, arity: usize) -> Result<NonNull<raw::MethodInfo>, WriteError> {
        let worded = |error| match error {
            MethodError::Metadata(error) => WriteError::from(error),
            MethodError::Twice => WriteError::Refused(format!("Ambiguous {}", name.to_string_lossy())),
            MethodError::Unnamed => unsupported("null method name").into(),
            MethodError::Missing | MethodError::Untyped | MethodError::Signature | MethodError::Parameters => WriteError::Refused(format!("Missing {}", name.to_string_lossy())),
        };
        // SAFETY: immutable runtime metadata.
        unsafe {
            let owner = self.nearest_declaring(owner, name, arity).map_err(worded)?;
            let method = self.find_method(owner, &MethodQuery::named(name, arity)).map_err(worded)?;
            NonNull::new(method.cast_mut()).ok_or_else(|| worded(MethodError::Missing))
        }
    }
    unsafe fn field_call<'c, 'a, 'r>(
        &self,
        context: &'c InvocationContext<'a, 'r>,
        object: &Root<'_, '_>,
        name: &CStr,
        arity: usize,
    ) -> Result<CheckedMethod<'c, 'a, 'r>, WriteError> {
        // SAFETY: only the feature-listed getter/setter bodies are authorized by callers, on rooted current objects.
        unsafe {
            let owner = (self.object_class)(target(object)?.as_ptr());
            let method = self.field_method(owner, name, arity)?;
            let declaring = (self.method_class)(method.as_ptr());
            let closed = if (self.class_inflated)(declaring) {
                vec![NonNull::new(declaring).ok_or_else(|| WriteError::Refused("Missing declaring class".into()))?]
            } else {
                vec![]
            };
            context.bind_with_closed_references(method, &closed).map_err(|e| WriteError::Refused(e.to_string()))
        }
    }
    /// Bind an operation's complete expected signature before its first mutation.
    unsafe fn field_exact<'c, 'a, 'r>(&self, context: &'c InvocationContext<'a, 'r>, object: &Root<'_, '_>, spec: &MethodSpec) -> Result<CheckedMethod<'c, 'a, 'r>, WriteError> {
        // SAFETY: rooted current receiver and immutable metadata; select its nearest implementation, then check every type.
        unsafe {
            let method = self.field_method((self.object_class)(target(object)?.as_ptr()), spec.method, spec.params.len())?;
            self.hook_method_contract((self.method_class)(method.as_ptr()), spec)?;
            context.bind(method).map_err(|e| WriteError::Refused(e.to_string()))
        }
    }

    /// A Bool-returning lookup with one final out reference into the rooted source's existing graph. Inputs have
    /// exactly the spec's by-value primitive/enum storage; they are supplied only by the three feature-owned queries.
    /// This is not general ref/out invocation: no newly allocated or otherwise unowned reference may be returned.
    unsafe fn field_lookup<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        source: &Root<'_, '_>,
        spec: &MethodSpec,
        inputs: &mut [*mut c_void],
    ) -> Result<Option<Root<'a, 'r>>, WriteError> {
        // SAFETY: complete signature is checked, source retains the queried objects for this nonmoving-GC span,
        // and the out pointer is a native stack slot. Root it before subsequent metadata/managed operations.
        unsafe {
            if !spec.instance || spec.returns != "System.Boolean" || spec.params.len() != inputs.len() + 1 {
                return Err(WriteError::Refused("Invalid reference lookup contract".into()));
            }
            let method = self.field_method((self.object_class)(target(source)?.as_ptr()), spec.method, spec.params.len())?;
            self.hook_method_contract((self.method_class)(method.as_ptr()), spec)?;
            let output = (self.method_param)(method.as_ptr(), inputs.len() as u32);
            let class = (self.type_class)(output);
            if !(self.type_byref)(output) || class.is_null() || (self.class_value)(class) {
                return Err(WriteError::Refused("Reference lookup output type".into()));
            }
            let mut result = std::ptr::null_mut::<raw::Object>();
            let mut args = inputs.to_vec();
            args.push((&raw mut result).cast());
            let mut exception = std::ptr::null_mut();
            let returned = (self.runtime_invoke)(method.as_ptr(), target(source)?.as_ptr().cast(), args.as_mut_ptr(), &raw mut exception);
            if let Some(exception) = NonNull::new(exception) {
                let _exception = attached.root(exception.cast()).map_err(GameError::from)?;
                return Err(WriteError::Refused("The game's lookup raised an exception".into()));
            }
            // The source already owns the output. Root the newly boxed Boolean before making any further runtime call.
            let answer = attached
                .root(NonNull::new(returned).ok_or_else(|| WriteError::Refused("Missing lookup result".into()))?)
                .map_err(GameError::from)?;
            let result = NonNull::new(result).map(|p| attached.root(p)).transpose().map_err(GameError::from)?;
            self.exact(&answer, self.system_class(c"Boolean")?)?;
            let payload = NonNull::new((self.object_unbox)(target(&answer)?.as_ptr())).ok_or_else(|| WriteError::Refused("Missing lookup Boolean".into()))?;
            if payload.cast::<u8>().as_ptr().read_unaligned() == 0 {
                return Ok(None);
            }
            if let Some(result) = &result {
                self.check_object(class, target(result)?)?;
            }
            Ok(result)
        }
    }
}
fn fixed_snapshot(set: FieldSet, manager: &Root<'_, '_>, container: &Root<'_, '_>, values: impl IntoIterator<Item = (u64, i32)>) -> Result<Snapshot, WriteError> {
    let snapshot = Snapshot {
        set,
        manager: target(manager)?.as_ptr() as u64,
        container: target(container)?.as_ptr() as u64,
        fields: values
            .into_iter()
            .enumerate()
            .map(|(index, (identity, value))| Field {
                key: index as u64,
                identity,
                item_type: 0,
                sub_type: 0,
                label: String::new(),
                value,
            })
            .collect(),
    };
    snapshot.encode().map_err(|e| WriteError::Refused(e.to_string()))?;
    Ok(snapshot)
}
fn integer(value: ReturnValue<'_, '_>) -> Result<i32, WriteError> {
    match value {
        ReturnValue::Scalar { value: Scalar::I32(value), .. } => Ok(value),
        _ => Err(WriteError::Refused("Unexpected integer result".into())),
    }
}
fn void(value: ReturnValue<'_, '_>) -> Result<(), WriteError> {
    if matches!(value, ReturnValue::Void) {
        Ok(())
    } else {
        Err(WriteError::Refused("Unexpected setter result".into()))
    }
}
fn returned<'a, 'r>(outcome: Result<InvocationOutcome<'a, 'r>, il2cpp_runtime::InvokeError>) -> Result<ReturnValue<'a, 'r>, WriteError> {
    match outcome.map_err(|e| WriteError::Refused(e.to_string()))? {
        InvocationOutcome::Returned(value) => Ok(value),
        InvocationOutcome::Exception(_) => Err(WriteError::Refused("The game's method raised an exception".into())),
    }
}
fn reference<'a, 'r>(value: ReturnValue<'a, 'r>) -> Result<Root<'a, 'r>, WriteError> {
    match value {
        ReturnValue::Reference(Some(root)) => Ok(root),
        ReturnValue::Reference(None) => Err(WriteError::NotReady("The container is still loading".into())),
        _ => Err(WriteError::Refused("Unexpected container result".into())),
    }
}
fn boolean(value: ReturnValue<'_, '_>) -> Result<bool, WriteError> {
    match value {
        ReturnValue::Scalar {
            value: Scalar::Boolean(value), ..
        } => Ok(value),
        _ => Err(WriteError::Refused("Unexpected Boolean result".into())),
    }
}
