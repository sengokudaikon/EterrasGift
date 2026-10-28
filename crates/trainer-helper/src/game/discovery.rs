//! Resolving the game's runtime: its own image, its classes (including the two it nests in others) and the dispatcher
//! the helper runs on. Nothing here reads a game object or calls a game method.

use super::{Class, GameError, Ptr, RuntimeAccess, read_value, unsupported};
use eg_il2cpp_runtime::{AttachedThread, metadata::record_class};
use eg_trainer_wire::protocol::{Discovery, DispatcherSpec};
use std::{ffi::CStr, ptr::NonNull};

/// Discovery data plus a typed address retained only while the caller's runtime contract holds.
pub struct DiscoveredRuntime {
    pub discovery: Discovery,
    pub dispatcher: NonNull<u8>,
}

pub(crate) struct Images {
    pub(crate) all: Vec<Ptr>,
    /// The game's own assembly: the image that defines `PlayerFinder`.
    pub(crate) game: Ptr,
}

impl RuntimeAccess {
    // SAFETY for internal helpers: called only beneath discover/status's live attached-runtime contract.
    pub(crate) unsafe fn images(&self) -> Result<Images, GameError> {
        // SAFETY: Image and class descriptors are retained immutable metadata; lookup never invokes a game method.
        unsafe {
            let all = self.api.images()?;
            let mut game: Ptr = std::ptr::null_mut();
            let mut player_finder: Class = std::ptr::null_mut();
            for &image in &all {
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
            Ok(self.class_in(&images.all, namespace, name)?)
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
            if build_id.len() != 64 || !build_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
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
