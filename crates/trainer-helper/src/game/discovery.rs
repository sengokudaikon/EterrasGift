//! Resolving the game's runtime: its own image, its classes (including the two it nests in others) and the dispatcher
//! the helper runs on. Nothing here reads a game object or calls a game method.

use super::{Class, GameError, Image, RuntimeAccess, read_value, unsupported};
use eg_trainer_wire::protocol::{Discovery, DispatcherSpec};
use il2cpp_runtime::{AttachedThread, metadata::record_class};
use std::{ffi::CStr, ptr::NonNull, sync::Mutex};

/// Discovery data plus a typed address retained only while the caller's runtime contract holds.
pub struct DiscoveredRuntime {
    pub discovery: Discovery,
    pub dispatcher: NonNull<u8>,
}

#[derive(Clone, Copy)]
pub(crate) struct Images {
    pub(crate) all: &'static [Image],
    /// The game's own assembly: the image that defines `PlayerFinder`.
    pub(crate) game: Image,
}

/// The images of the assembly table they were read from. IL2CPP's table is fixed once its domain exists; a table at
/// another address or of another length is read again rather than trusted.
pub(crate) struct ImageCache(Mutex<Option<CachedImages>>);

impl Default for ImageCache {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

struct CachedImages {
    table: usize,
    count: usize,
    images: Images,
}

// SAFETY: image descriptors are the runtime's immutable metadata. The cache only hands them back to that runtime's
// exports, on whichever thread is attached; it holds no object and no mutable state.
unsafe impl Send for CachedImages {}

impl RuntimeAccess {
    /// Every assembly's image and the game's own, read once per assembly table. The image list is kept for the process,
    /// as the helper keeps everything the game's threads may still reach.
    // SAFETY for internal helpers: called only beneath discover/status's live attached-runtime contract.
    pub(crate) unsafe fn images(&self) -> Result<Images, GameError> {
        // SAFETY: Image and class descriptors are retained immutable metadata; lookup never invokes a game method.
        unsafe {
            let Some(caches) = self.caches else {
                return self.read_images();
            };
            let domain = (self.domain_get)();
            if domain.is_null() {
                return Err(GameError::NotReady("domain unavailable"));
            }
            let mut count = 0;
            let table = (self.assemblies)(domain, &mut count) as usize;
            let cache = || caches.images.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(held) = cache().as_ref()
                && held.table == table
                && held.count == count
            {
                return Ok(held.images);
            }
            // The runtime is asked with no lock held.
            let images = self.read_images()?;
            *cache() = Some(CachedImages { table, count, images });
            Ok(images)
        }
    }

    /// # Safety
    /// As [`RuntimeAccess::images`].
    unsafe fn read_images(&self) -> Result<Images, GameError> {
        // SAFETY: as `images`.
        unsafe {
            let all: &'static [Image] = Vec::leak(self.api.images()?);
            let mut game: Image = std::ptr::null();
            let mut player_finder: Class = std::ptr::null_mut();
            for &image in all {
                let candidate = (self.class_from_name)(image, c"".as_ptr(), c"PlayerFinder".as_ptr());
                if !candidate.is_null() {
                    record_class(&mut player_finder, candidate, c"PlayerFinder")?;
                    if game.is_null() {
                        game = image;
                    }
                }
            }
            if game.is_null() {
                return Err(GameError::NotReady("PlayerFinder assembly missing"));
            }
            Ok(Images { all, game })
        }
    }

    // SAFETY: Images and class descriptors are retained immutable metadata; names and nested-class traversal are bounded and checked.
    pub(crate) unsafe fn class(&self, images: &Images, namespace: &CStr, name: &CStr) -> Result<Class, GameError> {
        // SAFETY: Image and class descriptors belong to retained immutable metadata; bounded lookup never invokes a game method.
        unsafe {
            let first = (self.class_from_name)(images.game, namespace.as_ptr(), name.as_ptr());
            if !first.is_null() {
                return Ok(first);
            }
            // Two classes the game nests in another rather than declaring at the top level.
            let parent = if name == c"CraftingContainers" {
                Some(c"ItemContainersManager")
            } else if name == c"Affix" {
                Some(c"AffixList")
            } else {
                None
            };
            if let Some(parent) = parent {
                let parent = (self.class_from_name)(images.game, c"".as_ptr(), parent.as_ptr());
                if !parent.is_null()
                    && let Some(nested) = self.nested_class(parent, name)
                {
                    return Ok(nested);
                }
            }
            Ok(self.class_in(images.all, namespace, name)?)
        }
    }

    /// Find the dispatcher the helper runs on, checking the runtime's layouts, without invoking game methods.
    ///
    /// # Safety
    /// `attached` must belong to these exports' live runtime on this OS thread. Runtime metadata
    /// must be immutable and assembly loading/shutdown excluded for the operation. The runtime
    /// must use the Unity 6000.4.8 x64 nonmoving-GC layouts: MethodInfo.methodPointer at offset 0,
    /// object header 16, array header 32, string length/chars at 16/20. The module range must be
    /// the readable mapped GameAssembly image containing generated method bodies. Class metadata
    /// and the allocation APIs must be valid; public header queries and allocated adapter samples
    /// are cross-checks, not permission to dereference arbitrary or untrusted memory.
    pub unsafe fn discover(&self, attached: &AttachedThread<'_>, build_id: String, module_base: NonNull<u8>, module_size: usize) -> Result<DiscoveredRuntime, GameError> {
        // SAFETY: The caller retains the mapped module and immutable metadata; the dispatcher RVA is bounded before its instruction prefix is copied.
        unsafe {
            if !eg_trainer_wire::protocol::is_build_id(&build_id) {
                return Err(unsupported("helper build ID"));
            }
            let images = self.images()?;
            self.check_allocated_adapters(attached)?;
            let dispatcher = self.method(self.class(&images, c"UnityEngine", c"UnitySynchronizationContext")?, c"ExecuteTasks", &[], false)?;
            let address = read_value(dispatcher.cast::<*mut u8>());
            let base = module_base.as_ptr() as usize;
            let rva = (address as usize)
                .checked_sub(base)
                .filter(|offset| offset.checked_add(16).is_some_and(|end| end <= module_size))
                .ok_or_else(|| unsupported("dispatcher module range"))?;
            let mut bytes = [0; 16];
            bytes.copy_from_slice(&read_value::<[u8; 16]>(address.cast()));
            Ok(DiscoveredRuntime {
                discovery: Discovery {
                    build_id,
                    dispatcher: DispatcherSpec { rva: rva as u64, bytes },
                },
                dispatcher: NonNull::new(address).ok_or_else(|| unsupported("dispatcher address"))?,
            })
        }
    }
}
