//! Last Epoch's runtime, through [`il2cpp_runtime`], in layers:
//! - `discovery` resolves the game's image, classes and the dispatcher; it reads no object;
//! - `read` and `apply` share typed object reads and checked one-time edits; retained gateways apply persistent effects;
//! - `accessors` give typed access to one game object each: the player, the character's stats, the held item, play
//!   presence, the locale, the item limits. They know nothing of what the trainer offers;
//! - `features` are what the trainer offers, built on the accessors: stat edits, forge edits, tooltips, hooks.
//!
//! Fields are resolved, never carried as game-version offsets. [`RuntimeAccess`] is the runtime's [`MetadataApi`]
//! with the game's own cached bindings beside it; every generic lookup is the runtime crate's.

// Entry-point contracts cover the shared runtime assumptions. Keep local notes
// for bounds, ownership and ABI details rather than repeating those contracts.

use il2cpp_runtime::metadata::{Class, MetadataError, Method, Ptr, copy_values, metadata_text, read_value, target, write_bytes};
#[cfg(test)]
use il2cpp_runtime::metadata::{TestExportStub, Type};
use il2cpp_runtime::{MetadataApi, RuntimeError, StrongRoot, raw};
use std::{
    ffi::{CStr, c_void},
    ops::Deref,
    ptr::NonNull,
    sync::OnceLock,
};

mod accessors;
mod apply;
mod discovery;
mod dispatcher_thread;
mod features;
mod read;
#[cfg(test)]
mod test_fakes;

pub use accessors::presence;
pub use apply::WriteError;
pub use discovery::DiscoveredRuntime;
use discovery::Images;
#[cfg(any(test, feature = "fixture"))]
pub(crate) use features::damage;
#[cfg(test)]
pub(crate) use features::{any_waypoint, encounter_counts, gains, player_only, results, results::no_mana_cost, reveal_objective, runes_glyphs_to_99, shards_to_99};
pub(crate) use features::{registry, registry::GameplayHookTarget, stat::StatBinding};

/// A strong GC root on a managed object, held for one call.
pub(crate) type Root<'a, 'r> = StrongRoot<'a, 'r>;

/// A missing contract fails discovery as a whole; no partial layout is published.
#[derive(Debug)]
pub enum GameError {
    MissingExport(raw::MissingExport),
    Runtime(RuntimeError),
    NotReady(&'static str),
    OfflineRequired,
    Contract(String),
}

impl std::fmt::Display for GameError {
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
impl std::error::Error for GameError {}
impl From<RuntimeError> for GameError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}
impl From<MetadataError> for GameError {
    fn from(error: MetadataError) -> Self {
        match error {
            MetadataError::MissingExport(missing) => Self::MissingExport(missing),
            MetadataError::Runtime(error) => Self::Runtime(error),
            MetadataError::NotReady(reason) => Self::NotReady(reason),
            MetadataError::Contract(contract) => Self::Contract(contract),
        }
    }
}

fn unsupported(contract: impl Into<String>) -> GameError {
    GameError::Contract(contract.into())
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
    pub unsafe fn resolve(resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, GameError> {
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
