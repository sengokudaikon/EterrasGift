//! A pin for a retained gateway's mutable managed scalar. It owns no feature or Unity operation. The lifecycle
//! closes, drains and conditionally restores the gateway before releasing this pin on an attached runtime thread.
use crate::game::{GameError, unsupported};
use il2cpp_runtime::raw;
use std::{ffi::c_void, ptr::NonNull};

pub(crate) struct ScalarBinding {
    pub receiver: usize,
    pub offset: u32,
    pub baseline: u32,
    pub online: usize,
    handle: NonNull<c_void>,
    free: raw::GcHandleFree,
}
// SAFETY: pinned nonmoving storage; its serialized owner releases it only after close/drain/restore.
unsafe impl Send for ScalarBinding {}
// SAFETY: sharing reads immutable binding data; field writes are owned by the gateway lifecycle.
unsafe impl Sync for ScalarBinding {}
impl ScalarBinding {
    /// # Safety
    /// Attached thread, live rooted receiver with a validated aligned mutable scalar; retain this binding until
    /// every gateway using it is closed, drained and restored.
    pub(crate) unsafe fn pin(api: raw::Api, receiver: usize, offset: u32, baseline: u32, online: usize) -> Result<Self, GameError> {
        // SAFETY: caller holds a root through acquisition; the GC pin retains this exact managed object.
        let handle = NonNull::new(unsafe { (api.gchandle_new)(receiver as *mut raw::Object, true) }).ok_or_else(|| unsupported("pin scalar receiver"))?;
        Ok(Self {
            receiver,
            offset,
            baseline,
            online,
            handle,
            free: api.gchandle_free,
        })
    }
}
impl Drop for ScalarBinding {
    fn drop(&mut self) {
        // SAFETY: the owner closed/drained/restored every gateway before releasing its last binding.
        unsafe { (self.free)(self.handle.as_ptr()) };
    }
}
