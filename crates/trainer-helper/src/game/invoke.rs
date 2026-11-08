//! Calling the game's own methods: the body a receiver runs for one.
use crate::game::{Method, Ptr, RuntimeAccess};
use il2cpp_runtime::raw;
use std::ptr::NonNull;

impl RuntimeAccess {
    /// The body `receiver` runs for `method` (`InvocationApi::dispatched`), when it is the game's own: an override from
    /// another image than `game`, a mod's, is none.
    ///
    /// # Safety
    /// An attached thread of this runtime, `receiver` live, and `method` a method of one of its classes.
    pub(crate) unsafe fn game_body(&self, api: raw::InvocationApi, game: Ptr, receiver: NonNull<raw::Object>, method: Method) -> Option<Method> {
        // SAFETY: as this function's; the override's descriptor is retained metadata.
        unsafe {
            let body = api.dispatched(receiver, method)?;
            (body == method || (self.class_image)((api.method_get_class)(body)) == game).then_some(body)
        }
    }
}
