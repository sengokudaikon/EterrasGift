//! Main-thread count editors over one rooted player/container graph. The read/apply transport is shared; currency
//! deltas and held-stack removal/set semantics stay with their respective features. A store is never retried.
mod currency;
mod stash;
use crate::game::{
    apply::{WriteError, stored},
    *,
};
use eg_trainer_wire::field_edits::{Edit, FieldSet, Snapshot};
use il2cpp_runtime::{AttachedThread, CheckedMethod, InvocationContext, InvocationOutcome, ReturnValue, Scalar, ValueTypeContract};

pub(crate) struct Counts<'a, 'r> {
    _manager: Root<'a, 'r>,
    container: Root<'a, 'r>,
    entries: Vec<Root<'a, 'r>>,
    snapshot: Snapshot,
}

impl RuntimeAccess {
    /// # Safety
    /// Admitted attached main thread, initialized runtime and exclusive synchronous player graph, as read_forge.
    pub(crate) unsafe fn read_counts(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, manager: u64, set: FieldSet) -> Result<Snapshot, WriteError> {
        // SAFETY: caller retains runtime and excludes graph mutation throughout the rooted read and final gate.
        unsafe {
            let graph = self.count_graph(attached, api, manager, set)?;
            self.count_gate(attached, manager)?;
            Ok(graph.snapshot)
        }
    }
    /// # Safety
    /// read_counts contract plus permission for the game's currency/quantity/removal methods and their saves/events.
    pub(crate) unsafe fn apply_counts(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, edit: &Edit) -> Result<Snapshot, WriteError> {
        // SAFETY: all destinations are prepared before the store boundary; rooted graph retained through each call.
        unsafe {
            edit.validate().map_err(|e| WriteError::Refused(e.to_string()))?;
            let graph = self.count_graph(attached, api, edit.expected.manager, edit.expected.set)?;
            if !graph.snapshot.same_values(&edit.expected) {
                return Err(WriteError::Stale("The game holds something else now; read again".into()));
            }
            if edit.values.iter().zip(&edit.expected.fields).all(|(v, field)| v.is_none_or(|v| v == field.value)) {
                return Ok(graph.snapshot);
            }
            match edit.expected.set {
                FieldSet::Currency => self.apply_currency(attached, api, &graph, &edit.values),
                _ => self.apply_stash(attached, api, &graph, &edit.values),
            }
        }
    }
    unsafe fn count_gate(&self, attached: &AttachedThread<'_>, expected: u64) -> Result<(), WriteError> {
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
    unsafe fn count_graph<'a, 'r>(&self, attached: &'a AttachedThread<'r>, api: raw::InvocationApi, manager: u64, set: FieldSet) -> Result<Counts<'a, 'r>, WriteError> {
        // SAFETY: checked offline/live manager, then root its current static reference, never a caller-supplied address.
        unsafe {
            self.count_gate(attached, manager)?;
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
                FieldSet::Currency => self.currency_graph(attached, api, root),
                _ => self.stash_graph(attached, api, root, set),
            }
        }
    }
    unsafe fn count_child<'a, 'r>(&self, attached: &'a AttachedThread<'r>, parent: &Root<'_, '_>, name: &CStr) -> Result<Root<'a, 'r>, WriteError> {
        // SAFETY: rooted current parent, validated reference field, same exclusive graph span.
        unsafe {
            self.child(attached, parent, name)?
                .ok_or_else(|| WriteError::NotReady(format!("{} is still loading", name.to_string_lossy())))
        }
    }
    unsafe fn count_method(&self, mut owner: Class, name: &CStr, arity: usize) -> Result<NonNull<raw::MethodInfo>, WriteError> {
        // SAFETY: immutable runtime metadata. Select the nearest implementation; inherited virtual methods are not
        // implicitly dispatched. The current dynamic receiver's matching override wins, as in the original trainer.
        unsafe {
            for _ in 0..128 {
                if owner.is_null() {
                    break;
                }
                let mut found = None;
                for method in self.methods(owner) {
                    let method = method?;
                    let actual = (self.method_name)(method);
                    if !actual.is_null() && metadata_text(actual) == name && (self.method_params)(method) as usize == arity {
                        if found.is_some() {
                            return Err(WriteError::Refused(format!("Ambiguous {}", name.to_string_lossy())));
                        }
                        found = NonNull::new(method.cast_mut());
                    }
                }
                if let Some(found) = found {
                    return Ok(found);
                }
                owner = (self.class_parent)(owner);
            }
            Err(WriteError::Refused(format!("Missing {}", name.to_string_lossy())))
        }
    }
    unsafe fn count_call<'c, 'a, 'r>(
        &self,
        context: &'c InvocationContext<'a, 'r>,
        api: raw::InvocationApi,
        object: &Root<'_, '_>,
        name: &CStr,
        arity: usize,
    ) -> Result<CheckedMethod<'c, 'a, 'r>, WriteError> {
        // SAFETY: only the feature-listed getter/setter bodies are authorized by callers, on rooted current objects.
        unsafe {
            let owner = (self.object_class)(target(object)?.as_ptr());
            let method = self.count_method(owner, name, arity)?;
            let declaring = (api.method_get_class)(method.as_ptr());
            let closed = if (api.class_is_inflated)(declaring) {
                vec![NonNull::new(declaring).ok_or_else(|| WriteError::Refused("Missing declaring class".into()))?]
            } else {
                vec![]
            };
            context.bind_with_closed_references(method, &closed).map_err(|e| WriteError::Refused(e.to_string()))
        }
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
