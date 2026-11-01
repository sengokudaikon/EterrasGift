//! What a Unity player adds to the runtime, through its managed classes: whether a wrapper's native object still exists,
//! and whether this is Unity's main thread.

use crate::metadata::{Class, MetadataApi, MetadataError, metadata_text, target, unsupported};
use crate::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, StrongRoot, raw};
use std::ptr::NonNull;

impl MetadataApi {
    /// Whether the native object behind a rooted `UnityEngine.Object` wrapper, an `owner`, still exists. `label` begins any
    /// contract failure's text.
    ///
    /// # Safety
    /// The lookup contract ([`crate::metadata`]), with `object` rooted in this runtime.
    pub unsafe fn unity_object_alive(&self, object: &StrongRoot<'_, '_>, owner: Class, label: &str) -> Result<bool, MetadataError> {
        // SAFETY: The rooted Unity wrapper has exact IntPtr storage; the copied native address is never dereferenced.
        unsafe {
            self.check_object(owner, target(object)?)?;
            let field = self.instance_field_in(owner, c"m_CachedPtr", self.system_class(c"IntPtr")?, label)?;
            let mut native = 0_usize;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut native as *mut usize).cast());
            Ok(native != 0)
        }
    }

    /// Asks Unity whether the calling thread is its native main thread, through `UnityEngine.Object`, here `owner`.
    /// `SynchronizationContext.Current` can be replaced by the game or its mods and is not a thread identity. `label` begins
    /// any contract failure's text.
    ///
    /// # Safety
    /// An attached thread of an initialized Unity runtime, with its code and metadata resident, and `api` of that runtime.
    /// `Object.CurrentThreadIsMainThread` is an internal call that compares the caller's OS identity with Unity's native
    /// main-thread identity; the caller permits its cold icall resolution and Object's class initialization.
    pub unsafe fn current_thread_is_main(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, owner: Class, label: &str) -> Result<bool, MetadataError> {
        // SAFETY: The checked, thread-safe engine predicate observes only caller identity.
        unsafe {
            let boolean = self.system_class(c"Boolean")?;
            let mut found = None;
            for method in self.methods(owner) {
                let method = method?;
                let name = (self.method_name)(method);
                if name.is_null() {
                    return Err(unsupported(format!("{label} predicate name")));
                }
                if metadata_text(name) != c"CurrentThreadIsMainThread" {
                    continue;
                }
                let ty = (api.method_get_return_type)(method);
                if found.is_some()
                    || (api.method_get_class)(method) != owner
                    || (api.method_is_instance)(method)
                    || (api.method_get_param_count)(method) != 0
                    || ty.is_null()
                    || (api.type_is_byref)(ty)
                    || (api.class_from_type)(ty) != boolean
                {
                    return Err(unsupported(format!("{label} predicate signature")));
                }
                found = NonNull::new(method.cast_mut());
            }
            let calls = InvocationContext::new(attached, api);
            let method = found.ok_or_else(|| unsupported(format!("{label} predicate missing")))?;
            let predicate = calls.bind(method).map_err(|e| unsupported(e.to_string()))?;
            match predicate.invoke(None, &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Scalar { value: Scalar::Boolean(main), .. }) => Ok(main),
                _ => Err(unsupported(format!("{label} predicate result"))),
            }
        }
    }
}
