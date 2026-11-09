//! Last Epoch's runtime, through [`il2cpp_runtime`], in layers:
//! - `discovery` resolves the game's image, classes and the dispatcher; it reads no object;
//! - `read` (liveness in the game's types) and `apply` (checked one-time edits) sit on the runtime crate's typed reads; retained gateways apply persistent effects;
//! - `accessors` give typed access to one game object each: the player, the character's stats, the held item, play
//!   presence, the locale, the item limits. They know nothing of what the trainer offers;
//! - `features` are what the trainer offers, built on the accessors: stat edits, forge edits, tooltips, hooks.
//!
//! Fields are resolved, never carried as game-version offsets. [`RuntimeAccess`] is the runtime's [`Exports`]
//! with the game's own cached bindings beside it; every generic lookup is the runtime crate's.

// Entry-point contracts cover the shared runtime assumptions. Keep local notes
// for bounds, ownership and ABI details rather than repeating those contracts.

use il2cpp_runtime::metadata::{Class, MetadataError, Method, Ptr, array_data, copy_values, instance_offset, metadata_text, read_value, target, write_bytes};
pub(crate) use il2cpp_runtime::metadata::{MethodError, MethodQuery, Others, Params, Returns};
#[cfg(test)]
use il2cpp_runtime::metadata::{TestExportStub, Type};
#[cfg(test)]
use il2cpp_runtime::test_fakes;
use il2cpp_runtime::{Exports, RuntimeError, StrongRoot, raw};
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
mod invoke;
mod read;

pub use accessors::presence;
pub use apply::WriteError;
pub use discovery::DiscoveredRuntime;
use discovery::Images;
#[cfg(any(test, feature = "fixture"))]
pub(crate) use features::damage;
#[cfg(test)]
pub(crate) use features::death_drops;
#[cfg(test)]
pub(crate) use features::{
    any_waypoint, encounter_counts, gains, morditas_timer, player_only, prophecy_uses, results, results::no_mana_cost, reveal_objective, runes_glyphs_to_99, shards_to_99,
    skip_quest_echoes, spawner_group,
};
pub(crate) use features::{registry, registry::GameplayHookTarget, registry::ScalarBinding};

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
    api: Exports,
    caches: Option<&'static Caches>,
}

/// Metadata resolved once per runtime: class, field and method descriptors are immutable once the runtime is up, and
/// the queries that run on every poll (status, presence) or every lookup (the images) would otherwise resolve them on
/// the game's main thread each time. Object state is never cached.
#[derive(Default)]
pub(crate) struct Caches {
    presence: OnceLock<presence::PresenceBindings>,
    status: OnceLock<accessors::player::StatusBindings>,
    images: discovery::ImageCache,
}

/// A value from a cache, or made for one call by an access without one.
pub(crate) enum Held<T: 'static> {
    Cached(&'static T),
    Local(T),
}

impl<T> Deref for Held<T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Cached(value) => value,
            Self::Local(value) => value,
        }
    }
}

impl RuntimeAccess {
    /// The value in `slot`, made by `make` the first time.
    fn cached<T>(&self, slot: fn(&'static Caches) -> &'static OnceLock<T>, make: impl FnOnce() -> Result<T, GameError>) -> Result<Held<T>, GameError> {
        let Some(caches) = self.caches else {
            return make().map(Held::Local);
        };
        let slot = slot(caches);
        if let Some(value) = slot.get() {
            return Ok(Held::Cached(value));
        }
        let _ = slot.set(make()?);
        Ok(Held::Cached(slot.get().expect("set just above")))
    }
}

impl Deref for RuntimeAccess {
    type Target = Exports;
    fn deref(&self) -> &Exports {
        &self.api
    }
}

// Tests replace single exports with fakes.
#[cfg(test)]
impl std::ops::DerefMut for RuntimeAccess {
    fn deref_mut(&mut self) -> &mut Exports {
        &mut self.api
    }
}

impl RuntimeAccess {
    /// Resolve the complete metadata/status export table.
    ///
    /// # Safety
    /// As [`Exports::resolve`].
    pub unsafe fn resolve(resolver: impl FnMut(&'static CStr) -> Option<NonNull<c_void>>) -> Result<Self, GameError> {
        Ok(Self {
            // SAFETY: the caller establishes export identity, ABI and lifetime.
            api: unsafe { Exports::resolve(resolver) }?,
            // One per resolved runtime, kept for the process: the helper never frees what the game may still reach.
            caches: Some(Box::leak(Box::default())),
        })
    }

    /// Every export a stub that panics when reached, and nothing cached.
    #[cfg(test)]
    fn test_stubs() -> Self {
        Self {
            api: Exports::test_stubs(),
            caches: None,
        }
    }
}
