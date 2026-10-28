//! The local player: the offline gate, the item manager `PlayerFinder` holds, the player's actor and the actor's stats,
//! each rooted for one call and identified by its managed wrapper and native object. No getter, refresh or setter runs.
use crate::game::apply::WriteError;
use crate::game::{Class, GameError, Images, Ptr, Root, RuntimeAccess, read_value, target, unsupported};
use eg_il2cpp_runtime::{AttachedThread, raw};
use eg_trainer_wire::{
    fields::{AttackLeechUnavailable as Unavailable, ObservedObjectIdentity},
    protocol::RuntimeStatus,
};
use std::ptr::NonNull;

/// Why the player could not be read: `Unavailable` is the game's own state (online, loading, changed), `Contract` a
/// runtime that is not the supported one.
#[derive(Debug)]
pub(crate) enum PlayerError {
    Unavailable(Unavailable),
    Contract(GameError),
}

impl From<GameError> for PlayerError {
    fn from(value: GameError) -> Self {
        Self::Contract(value)
    }
}

impl From<eg_il2cpp_runtime::MetadataError> for PlayerError {
    fn from(value: eg_il2cpp_runtime::MetadataError) -> Self {
        Self::Contract(value.into())
    }
}

impl From<eg_il2cpp_runtime::RuntimeError> for PlayerError {
    fn from(value: eg_il2cpp_runtime::RuntimeError) -> Self {
        Self::Contract(GameError::Runtime(value))
    }
}

impl From<PlayerError> for WriteError {
    fn from(error: PlayerError) -> Self {
        match error {
            PlayerError::Contract(error) => error.into(),
            PlayerError::Unavailable(reason) => match reason {
                Unavailable::OfflineRequired | Unavailable::ManagerAbsent => Self::Offline,
                Unavailable::ManagerChanged | Unavailable::GraphChanged => Self::Stale(format!("Current field graph changed: {reason:?}")),
                Unavailable::UnsupportedStatsClass => Self::Refused("Unsupported dynamic stats class".into()),
                reason => Self::NotReady(format!("Current field graph is unavailable: {reason:?}")),
            },
        }
    }
}

pub(crate) fn unavailable(reason: Unavailable) -> PlayerError {
    PlayerError::Unavailable(reason)
}

/// The classes and fields the player is read through.
pub(crate) struct PlayerFields {
    pub(crate) images: Images,
    pub(crate) single: Class,
    pub(crate) base_stats: Class,
    manager_class: Class,
    actor_class: Class,
    online: Ptr,
    manager: Ptr,
    actor: Ptr,
    cached: Ptr,
    object_class: Class,
    manager_actor: Ptr,
    actor_manager: Ptr,
    actor_stats: Ptr,
    stats_actor: Ptr,
}

/// The player for one call: its item manager, its actor and the actor's stats, rooted, with their identities.
pub(crate) struct Player<'a, 'r> {
    pub(crate) fields: PlayerFields,
    manager: Root<'a, 'r>,
    actor: Root<'a, 'r>,
    pub(crate) stats: Root<'a, 'r>,
    pub(crate) manager_identity: ObservedObjectIdentity,
    pub(crate) actor_identity: ObservedObjectIdentity,
    pub(crate) stats_identity: ObservedObjectIdentity,
}

impl RuntimeAccess {
    /// A dead manager revokes the whole player, unlike a dead actor or stats cache.
    ///
    /// # Safety
    /// As [`RuntimeAccess::alive`].
    unsafe fn live_manager(&self, manager: &Root<'_, '_>, object_class: Class, cached: Ptr) -> Result<ObservedObjectIdentity, PlayerError> {
        // SAFETY: same already-rooted graph contract as `alive`.
        match unsafe { self.alive(manager, object_class, cached) } {
            Ok(Some(identity)) => Ok(identity),
            Ok(None) => Err(unavailable(Unavailable::ManagerAbsent)),
            Err(error) => Err(error.into()),
        }
    }

    /// The managed wrapper and native object of a live actor or stats object.
    ///
    /// # Safety
    /// As [`RuntimeAccess::alive`].
    unsafe fn live(&self, object: &Root<'_, '_>, object_class: Class, cached: Ptr) -> Result<ObservedObjectIdentity, PlayerError> {
        // SAFETY: same already-rooted graph contract as `alive`.
        unsafe { self.alive(object, object_class, cached) }?.ok_or_else(|| unavailable(Unavailable::NativeObjectDestroyed))
    }

    /// The game must not be in online play.
    ///
    /// # Safety
    /// `online` is the validated static Boolean `GameplayEnvironment._isOnlinePlay` of this initialized runtime.
    unsafe fn offline(&self, online: Ptr) -> Result<(), PlayerError> {
        // SAFETY: The initialized runtime's validated static Boolean is copied into a byte, then checked for canonical representation.
        unsafe {
            let mut value = 1_u8;
            (self.static_get)(online, (&mut value as *mut u8).cast());
            match value {
                0 => Ok(()),
                1 => Err(unavailable(Unavailable::OfflineRequired)),
                _ => Err(unsupported("player online Boolean representation").into()),
            }
        }
    }

    /// Resolve the player's classes and fields, passing the offline gate on the way. Each field is resolved from its
    /// known declaring owner; inherited substitution and derived shadowing are refused.
    ///
    /// # Safety
    /// As [`RuntimeAccess::player`].
    pub(crate) unsafe fn player_fields(&self) -> Result<PlayerFields, PlayerError> {
        // SAFETY: Class and field metadata is retained and immutable; only the validated static Boolean is copied.
        unsafe {
            if !cfg!(target_arch = "x86_64") || std::mem::size_of::<usize>() != 8 || std::mem::size_of::<f32>() != 4 || std::mem::size_of::<u8>() != 1 {
                return Err(unsupported("player x64 scalar ABI").into());
            }
            let images = self.images()?;
            let boolean = self.system_class(c"Boolean")?;
            let single = self.system_class(c"Single")?;
            let int_ptr = self.system_class(c"IntPtr")?;
            for (class, code) in [(boolean, 0x02), (single, 0x0c), (int_ptr, 0x18)] {
                let ty = (self.class_type)(class);
                if !(self.class_value)(class) || ty.is_null() || (self.type_byref)(ty) || (self.type_code)(ty) != code {
                    return Err(unsupported("player primitive code/value class").into());
                }
            }
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.declared_field(environment, c"_isOnlinePlay", boolean, true)?;
            self.offline(online)?;
            let finder = self.class(&images, c"", c"PlayerFinder")?;
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let actor_class = self.class(&images, c"", c"Actor")?;
            let base_stats = self.class(&images, c"", c"BaseStats")?;
            let object_class = self.class(&images, c"UnityEngine", c"Object")?;
            // Reference slots are copied into pointer-sized storage only after rejecting value-type substitutions.
            for class in [finder, manager_class, actor_class, base_stats, object_class] {
                if (self.class_value)(class) {
                    return Err(unsupported("player reference class").into());
                }
            }
            Ok(PlayerFields {
                single,
                base_stats,
                manager_class,
                actor_class,
                object_class,
                online,
                cached: self.declared_field(object_class, c"m_CachedPtr", int_ptr, false)?,
                manager: self.declared_field(finder, c"localItemContainersManager", manager_class, true)?,
                actor: self.declared_field(finder, c"playerActorComponent", actor_class, true)?,
                manager_actor: self.declared_field(manager_class, c"actor", actor_class, false)?,
                actor_manager: self.declared_field(actor_class, c"itemContainersManager", manager_class, false)?,
                actor_stats: self.declared_field(actor_class, c"stats", base_stats, false)?,
                stats_actor: self.declared_field(base_stats, c"actor", actor_class, false)?,
                images,
            })
        }
    }

    /// Root the player through `fields`: the manager, which must be `expected_manager`, its actor and the actor's
    /// stats, each live and pointing back at the others.
    ///
    /// # Safety
    /// All discover/runtime/attachment contracts apply. The attachment belongs to this exact runtime and stays owned by
    /// this OS thread. Call only on Unity's initialized main thread after the admitted dispatcher's original returns,
    /// or under an independently admitted owned-fixture Unity main-thread Update callback satisfying every condition
    /// here, with no jobs, reentrancy or shutdown. Nonmoving GC and the public field-get/reference/root ABI match the
    /// validated runtime. `PlayerFinder` and `GameplayEnvironment` statics are already initialized by the game. The
    /// caller excludes every other change to these statics, reachable references, scalar fields and native lifetimes
    /// through the call. Strong roots preserve managed wrappers, not native lifetime. `expected_manager` is an
    /// identity token, never dereferenced; tokens do not stay unique after object death.
    pub(crate) unsafe fn player<'a, 'r>(&self, attached: &'a AttachedThread<'r>, fields: PlayerFields, expected_manager: u64) -> Result<Player<'a, 'r>, PlayerError> {
        // SAFETY: The caller excludes graph destruction; every returned reference is rooted before its class or fields are inspected.
        unsafe {
            let manager = NonNull::new(self.static_reference(fields.manager)).ok_or_else(|| unavailable(Unavailable::ManagerAbsent))?;
            let manager = attached.root(manager)?;
            self.check_object(fields.manager_class, target(&manager)?)?;
            let manager_identity = self.live_manager(&manager, fields.object_class, fields.cached)?;
            if expected_manager == 0 || manager_identity.wrapper_token() != expected_manager {
                return Err(unavailable(Unavailable::ManagerChanged));
            }
            let actor = NonNull::new(self.static_reference(fields.actor)).ok_or_else(|| unavailable(Unavailable::ActorCacheAbsent))?;
            let actor = attached.root(actor)?;
            self.check_object(fields.actor_class, target(&actor)?)?;
            let actor_identity = self.live(&actor, fields.object_class, fields.cached)?;
            if self.reference_at(&manager, fields.manager_actor)? != target(&actor)?.as_ptr() || self.reference_at(&actor, fields.actor_manager)? != target(&manager)?.as_ptr() {
                return Err(unavailable(Unavailable::GraphChanged));
            }
            let stats = NonNull::new(self.reference_at(&actor, fields.actor_stats)?).ok_or_else(|| unavailable(Unavailable::StatsAbsent))?;
            let stats = attached.root(stats)?;
            self.check_object(fields.base_stats, target(&stats)?)?;
            let stats_identity = self.live(&stats, fields.object_class, fields.cached)?;
            if self.reference_at(&stats, fields.stats_actor)? != target(&actor)?.as_ptr() {
                return Err(unavailable(Unavailable::GraphChanged));
            }
            Ok(Player {
                fields,
                manager,
                actor,
                stats,
                manager_identity,
                actor_identity,
                stats_identity,
            })
        }
    }

    /// The player is still the one rooted: offline, the same manager, actor and stats, all live and linked. Current
    /// statics are only compared; a replacement is never dereferenced.
    ///
    /// # Safety
    /// As [`RuntimeAccess::player`], with `player`'s roots still held.
    pub(crate) unsafe fn player_unchanged(&self, player: &Player<'_, '_>) -> Result<(), PlayerError> {
        // SAFETY: Existing roots remain live; fresh static references and native tokens are comparison data only.
        unsafe {
            let fields = &player.fields;
            self.offline(fields.online)?;
            let current_manager = self.static_reference(fields.manager);
            if current_manager.is_null() {
                return Err(unavailable(Unavailable::ManagerAbsent));
            }
            if current_manager != target(&player.manager)?.as_ptr() || self.live_manager(&player.manager, fields.object_class, fields.cached)? != player.manager_identity {
                return Err(unavailable(Unavailable::ManagerChanged));
            }
            if self.static_reference(fields.actor) != target(&player.actor)?.as_ptr()
                || self.reference_at(&player.manager, fields.manager_actor)? != target(&player.actor)?.as_ptr()
                || self.reference_at(&player.actor, fields.actor_manager)? != target(&player.manager)?.as_ptr()
                || self.reference_at(&player.actor, fields.actor_stats)? != target(&player.stats)?.as_ptr()
                || self.reference_at(&player.stats, fields.stats_actor)? != target(&player.actor)?.as_ptr()
                || self.live(&player.actor, fields.object_class, fields.cached)? != player.actor_identity
                || self.live(&player.stats, fields.object_class, fields.cached)? != player.stats_identity
            {
                return Err(unavailable(Unavailable::GraphChanged));
            }
            Ok(())
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
    pub unsafe fn status(&self, attached: &AttachedThread<'_>) -> Result<RuntimeStatus, GameError> {
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
