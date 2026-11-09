//! Callback-local check against Unity's native main-thread identity.
use super::{GameError, RuntimeAccess};
use il2cpp_runtime::AttachedThread;

impl RuntimeAccess {
    /// Ask Unity whether this callback is running on its native main thread.
    ///
    /// # Safety
    /// Invoke only after the original ExecuteTasks returns, on an attached thread
    /// of the supported initialized Unity6000.4 runtime. Code/metadata stay resident.
    /// As [`il2cpp_runtime::Exports::current_thread_is_main`].
    pub unsafe fn is_dispatcher_thread(&self, attached: &AttachedThread<'_>) -> Result<bool, GameError> {
        // SAFETY: The original callback returned on an attached runtime thread, and its caller permits the predicate's cold paths.
        unsafe {
            let images = self.images()?;
            let owner = self.class(&images, c"UnityEngine", c"Object")?;
            Ok(self.current_thread_is_main(attached, owner, "dispatcher")?)
        }
    }
}
