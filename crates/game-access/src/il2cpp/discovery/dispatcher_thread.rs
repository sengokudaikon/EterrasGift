//! Callback-local check against Unity's native main-thread identity.
use super::metadata_text;
use super::{DiscoveryError, RuntimeAccess, unsupported};
use crate::il2cpp::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use std::ptr::NonNull;

impl RuntimeAccess {
    /// Ask Unity whether this callback is running on its native main thread.
    /// SynchronizationContext.Current can be replaced by the game or its mods and
    /// is not a thread identity. No OS thread is nominated before bootstrap.
    ///
    /// # Safety
    /// Invoke only after the original ExecuteTasks returns, on an attached thread
    /// of the supported initialized Unity6000.4 runtime. Code/metadata stay resident.
    /// The supported Object.CurrentThreadIsMainThread internal call compares the
    /// caller's OS identity with Unity's initialized native main-thread identity.
    /// Its cold icall resolution and Object class initialization are permitted.
    pub unsafe fn is_dispatcher_thread(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi) -> Result<bool, DiscoveryError> {
        // SAFETY: The original callback returned on an attached runtime thread;
        // the checked, thread-safe engine predicate observes only caller identity.
        unsafe {
            let images = self.images()?;
            let owner = self.class(&images, c"UnityEngine", c"Object")?;
            let boolean = self.system_class(c"Boolean")?;
            let mut found = None;
            for method in self.methods(owner)? {
                let name = (self.method_name)(method);
                if name.is_null() {
                    return Err(unsupported("dispatcher predicate name"));
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
                    return Err(unsupported("dispatcher predicate signature"));
                }
                found = NonNull::new(method.cast_mut());
            }
            let calls = InvocationContext::new(attached, api);
            let method = found.ok_or_else(|| unsupported("dispatcher predicate missing"))?;
            let predicate = calls.bind(method).map_err(|e| unsupported(e.to_string()))?;
            match predicate.invoke(None, &mut []).map_err(|e| unsupported(e.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Scalar { value: Scalar::Boolean(main), .. }) => Ok(main),
                _ => Err(unsupported("dispatcher predicate result")),
            }
        }
    }
}
