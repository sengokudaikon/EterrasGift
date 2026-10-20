//! Callback-local check using the same synchronization context as ExecuteTasks.
use super::{Class, DiscoveryError, RuntimeAccess, unsupported};
use crate::il2cpp::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, raw};
use std::{ffi::CStr, ptr::NonNull};

impl RuntimeAccess {
    /// Test the current callback against UnitySynchronizationContext's managed main ID.
    /// No OS thread is nominated before bootstrap. A missing/wrong context is not main.
    ///
    /// # Safety
    /// Invoke only after the original ExecuteTasks returns, on an attached thread
    /// of the supported initialized Unity6000.4 runtime. Code/metadata stay resident.
    /// The supported CoreModule/BCL getters synchronously expose the current thread
    /// and context; the context's main ID is immutable after Unity initializes it.
    pub unsafe fn is_dispatcher_thread(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi) -> Result<bool, DiscoveryError> {
        // SAFETY: inherits is_dispatcher_thread's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let images = self.images()?;
            let context = self.class(&images, c"System.Threading", c"SynchronizationContext")?;
            let unity = self.class(&images, c"UnityEngine", c"UnitySynchronizationContext")?;
            let thread = self.class(&images, c"System.Threading", c"Thread")?;
            let int32 = self.system_class(c"Int32")?;
            let getter = |owner: Class, name: &CStr, returns: Class, instance: bool| -> Result<NonNull<raw::MethodInfo>, DiscoveryError> {
                let mut iterator = std::ptr::null_mut();
                let mut found = None;
                for _ in 0..4096 {
                    let method = (self.class_methods)(owner, &mut iterator);
                    if method.is_null() {
                        return found.ok_or_else(|| unsupported("dispatcher thread getter missing"));
                    }
                    let actual = (self.method_name)(method);
                    if actual.is_null() {
                        return Err(unsupported("dispatcher getter name"));
                    }
                    if CStr::from_ptr(actual) != name || (api.method_get_param_count)(method) != 0 {
                        continue;
                    }
                    let ty = (api.method_get_return_type)(method);
                    if found.is_some()
                        || (api.method_get_class)(method) != owner
                        || (api.method_is_instance)(method) != instance
                        || ty.is_null()
                        || (api.type_is_byref)(ty)
                        || (api.class_from_type)(ty) != returns
                    {
                        return Err(unsupported("dispatcher thread getter signature"));
                    }
                    found = NonNull::new(method.cast_mut());
                }
                Err(unsupported("dispatcher getter enumeration limit"))
            };
            let calls = InvocationContext::new(attached, api);
            let current = calls.bind(getter(context, c"get_Current", context, false)?).map_err(|e| unsupported(e.to_string()))?;
            let current = current.invoke(None, &mut []).map_err(|e| unsupported(e.to_string()))?;
            let InvocationOutcome::Returned(ReturnValue::Reference(Some(context))) = current else {
                return Ok(false);
            };
            if (self.object_class)(super::forge::target(&context)?.as_ptr()) != unity {
                return Ok(false);
            }
            let expected = self.primitive::<i32>(&context, c"m_MainThreadID")?;
            if expected <= 0 {
                return Ok(false);
            }
            let current_thread = calls.bind(getter(thread, c"get_CurrentThread", thread, false)?).map_err(|e| unsupported(e.to_string()))?;
            let current_thread = current_thread.invoke(None, &mut []).map_err(|e| unsupported(e.to_string()))?;
            let InvocationOutcome::Returned(ReturnValue::Reference(Some(thread_object))) = current_thread else {
                return Ok(false);
            };
            self.check_object(thread, super::forge::target(&thread_object)?)?;
            let id = calls.bind(getter(thread, c"get_ManagedThreadId", int32, true)?).map_err(|e| unsupported(e.to_string()))?;
            let id = id.invoke(Some(&thread_object), &mut []).map_err(|e| unsupported(e.to_string()))?;
            // CheckedMethod verifies the scalar storage before copying its value.
            Ok(matches!(id, InvocationOutcome::Returned(ReturnValue::Scalar { value: Scalar::I32(actual), .. }) if actual == expected))
        }
    }
}
