//! The two liveness reads that need the game's own types: the wire's identity and the game's image for the class. Every
//! other read of an object is the runtime crate's.

use super::{Class, Field, GameError, Images, Root, RuntimeAccess, unsupported};
use eg_trainer_wire::fields::ObservedObjectIdentity;
use il2cpp_runtime::AttachedThread;

impl RuntimeAccess {
    /// A Unity object's identity, its managed wrapper and native object, or `None` once the native object is gone.
    /// The native address is compared, never dereferenced.
    ///
    /// # Safety
    /// `object` is rooted; `object_class` is `UnityEngine.Object` and `cached` its validated `m_CachedPtr` field.
    pub(crate) unsafe fn alive(&self, object: &Root<'_, '_>, object_class: Class, cached: Field) -> Result<Option<ObservedObjectIdentity>, GameError> {
        // SAFETY: the caller's contract is the runtime crate's.
        let Some((object, native)) = (unsafe { self.identity_words(object, object_class, cached) })? else {
            return Ok(None);
        };
        ObservedObjectIdentity::new(object as u64, native as u64)
            .map(Some)
            .map_err(|_| unsupported("nonzero object identity"))
    }

    pub(crate) unsafe fn is_live(&self, attached: &AttachedThread<'_>, images: &Images, object: &Root<'_, '_>) -> Result<bool, GameError> {
        // SAFETY: the class comes from retained metadata; the caller's contract is the runtime crate's.
        unsafe {
            let unity_object = self.class(images, c"UnityEngine", c"Object")?;
            Ok(self.api.is_live(attached, unity_object, object)?)
        }
    }
}
