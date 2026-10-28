//! Last Epoch's runtime, read through [`eg_il2cpp_runtime`]: the game's image and classes, the dispatcher discovery and
//! the narrow main-thread status query, then each operation family in its own module.
//!
//! The public API resolves fields instead of carrying game-version offsets. [`RuntimeAccess`] is the runtime's
//! [`MetadataApi`] with the game's own cached bindings beside it; every generic lookup is the runtime crate's.

// Entry-point contracts cover the shared runtime assumptions. Keep local notes
// for bounds, ownership and ABI details rather than repeating those contracts.

use eg_il2cpp_runtime::metadata::{Class, MetadataError, Method, Ptr, copy_values, metadata_text, read_value, record_class, write_bytes};
#[cfg(test)]
use eg_il2cpp_runtime::metadata::{TestExportStub, Type};
use eg_il2cpp_runtime::{AttachedThread, MetadataApi, RuntimeError, raw};
use eg_trainer_wire::protocol::{Discovery, DispatcherSpec, RuntimeStatus};
use std::{
    ffi::{CStr, c_void},
    ops::Deref,
    ptr::NonNull,
    sync::OnceLock,
};

mod attack_leech;
mod gameplay_hooks;
pub use gameplay_hooks::GameplayHookTarget;
mod descriptions;
mod dispatcher_thread;
mod unique;
pub use attack_leech::AttackLeechWriteError;
mod catalog;
mod forge;
mod limits;
mod locale;
mod mapping;
pub mod presence;
mod write;
pub use write::ForgeWriteError;

/// A missing contract fails discovery as a whole; no partial layout is published.
#[derive(Debug)]
pub enum DiscoveryError {
    MissingExport(raw::MissingExport),
    Runtime(RuntimeError),
    NotReady(&'static str),
    OfflineRequired,
    Contract(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingExport(error) => error.fmt(f),
            Self::Runtime(error) => error.fmt(f),
            Self::NotReady(reason) => write!(f, "IL2CPP runtime is not ready: {reason}"),
            Self::OfflineRequired => f.write_str("An offline character with a live item manager is required"),
            Self::Contract(contract) => write!(f, "Unsupported IL2CPP runtime contract: {contract}"),
        }
    }
}
impl std::error::Error for DiscoveryError {}
impl From<RuntimeError> for DiscoveryError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}
impl From<MetadataError> for DiscoveryError {
    fn from(error: MetadataError) -> Self {
        match error {
            MetadataError::MissingExport(missing) => Self::MissingExport(missing),
            MetadataError::Runtime(error) => Self::Runtime(error),
            MetadataError::NotReady(reason) => Self::NotReady(reason),
            MetadataError::Contract(contract) => Self::Contract(contract),
        }
    }
}

fn unsupported(contract: impl Into<String>) -> DiscoveryError {
    DiscoveryError::Contract(contract.into())
}

/// The runtime's metadata exports, with the bindings this game's queries cache beside them.
#[derive(Clone, Copy)]
pub struct RuntimeAccess {
    api: MetadataApi,
    presence: Option<&'static OnceLock<presence::PresenceBindings>>,
}

impl Deref for RuntimeAccess {
    type Target = MetadataApi;
    fn deref(&self) -> &MetadataApi {
        &self.api
    }
}

// Tests replace single exports with fakes.
#[cfg(test)]
impl std::ops::DerefMut for RuntimeAccess {
    fn deref_mut(&mut self) -> &mut MetadataApi {
        &mut self.api
    }
}

impl RuntimeAccess {
    /// Resolve the complete metadata/status export table.
    ///
    /// # Safety
    /// As [`MetadataApi::resolve`].
    pub unsafe fn resolve(resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, DiscoveryError> {
        Ok(Self {
            // SAFETY: the caller establishes export identity, ABI and lifetime.
            api: unsafe { MetadataApi::resolve(resolver) }?,
            presence: Some(Box::leak(Box::new(OnceLock::new()))),
        })
    }

    /// Every export a stub that panics when reached, and nothing cached.
    #[cfg(test)]
    fn test_stubs() -> Self {
        Self {
            api: MetadataApi::test_stubs(),
            presence: None,
        }
    }
}

/// Discovery data plus a typed address retained only while the caller's runtime contract holds.
pub struct DiscoveredRuntime {
    pub discovery: Discovery,
    pub dispatcher: NonNull<u8>,
}

struct Images {
    all: Vec<Ptr>,
    /// The game's own assembly: the image that defines `PlayerFinder`.
    game: Ptr,
}

impl RuntimeAccess {
    // SAFETY for internal helpers: called only beneath discover/status's live attached-runtime contract.
    unsafe fn images(&self) -> Result<Images, DiscoveryError> {
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
                return Err(DiscoveryError::NotReady("PlayerFinder assembly missing"));
            }
            Ok(Images { all, game })
        }
    }

    // SAFETY: Images and class descriptors are retained immutable metadata; names and nested-class traversal are bounded and checked.
    unsafe fn class(&self, images: &Images, namespace: &CStr, name: &CStr) -> Result<Class, DiscoveryError> {
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

    /// Validate the legacy control discovery contract without invoking game methods.
    ///
    /// # Safety
    /// `attached` must belong to these exports' live runtime on this OS thread. Runtime metadata
    /// must be immutable and assembly loading/shutdown excluded for the operation. The runtime
    /// must use the Unity 6000.4.8 x64 nonmoving-GC layouts: MethodInfo.methodPointer at offset 0,
    /// object header 16, array header 32, string length/chars at 16/20. The module range must be
    /// the readable mapped GameAssembly image containing generated method bodies. Class metadata
    /// and the allocation APIs must be valid; public header queries and allocated adapter samples
    /// are cross-checks, not permission to dereference arbitrary or untrusted memory.
    pub unsafe fn discover(&self, attached: &AttachedThread<'_>, build_id: String, module_base: NonNull<u8>, module_size: usize) -> Result<DiscoveredRuntime, DiscoveryError> {
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

    /// Read the online flag and the current managed manager wrapper, without running game methods.
    /// The manager address is an ephemeral identity token; it grants no dereference permission.
    ///
    /// # Safety
    /// All `discover` runtime/attachment/layout conditions apply. Call only on Unity's main thread,
    /// after its original ExecuteTasks call has returned. The game's static fields/native-object
    /// lifetime must not race this query; no other thread may destroy the wrapper during rooting.
    /// No process-global initialized state is changed by this query. Static fields must already be
    /// initialized, as required by the admitted game session. This reader reports zero features;
    /// the helper fills in its independently tracked active hook count.
    pub unsafe fn status(&self, attached: &AttachedThread<'_>) -> Result<RuntimeStatus, DiscoveryError> {
        // SAFETY: The caller supplies the immutable main-thread graph span; static references are rooted and native pointer fields are validated before copying.
        unsafe {
            let images = self.images()?;
            let online_field = self.static_field(self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?, c"_isOnlinePlay", "System.Boolean")?;
            let mut online = 1_u8;
            (self.static_get)(online_field, (&mut online as *mut u8).cast());
            if online > 1 {
                return Err(unsupported("online Boolean representation"));
            }
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let field = self.static_field(self.class(&images, c"", c"PlayerFinder")?, c"localItemContainersManager", "ItemContainersManager")?;
            let mut manager = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(field, (&mut manager as *mut *mut raw::Object).cast());
            let manager_root = NonNull::new(manager).map(|pointer| attached.root(pointer)).transpose()?;
            let mut manager_address = 0;
            if let Some(root) = &manager_root {
                let manager = root.target().ok_or_else(|| unsupported("manager root target"))?;
                self.check_object(manager_class, manager)?;
                let cached = self.field(manager_class, c"m_CachedPtr")?;
                if (self.field_flags)(cached) & raw::FIELD_ATTRIBUTE_STATIC != 0
                    || (self.field_offset)(cached) != 16
                    || self.type_name((self.field_type)(cached))? != "System.IntPtr"
                {
                    return Err(unsupported("manager native pointer"));
                }
                // The managed wrapper is strongly rooted; this reads only its pointer-sized field,
                // and never dereferences the native object or assumes a root preserves native life.
                if !read_value(manager.as_ptr().cast::<u8>().add(16).cast::<Ptr>()).is_null() {
                    manager_address = manager.as_ptr() as u64;
                }
            }
            let master_class = self.class(&images, c"", c"AffixList")?;
            let field = self.static_field(self.class(&images, c"LE.AssetManagement", c"GlobalAssets")?, c"_storage_MasterAffixesList", "AffixList")?;
            let mut master = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(field, (&mut master as *mut *mut raw::Object).cast());
            let master_root = NonNull::new(master).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = master_root {
                self.check_object(master_class, root.target().ok_or_else(|| unsupported("master root target"))?)?;
            }
            Ok(RuntimeStatus {
                online: online != 0,
                manager: manager_address,
                features_on: 0,
            })
        }
    }
}
