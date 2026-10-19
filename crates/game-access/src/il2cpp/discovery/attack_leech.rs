//! Field-only attack/leech graph copy; no getters, refreshes or mutations.
use super::{
    Class, DiscoveryError, Ptr, RuntimeAccess,
    forge::{Root, target},
    unsupported,
};
use crate::{
    fields::{AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate},
    il2cpp::{AttachedThread, raw},
};
use std::{ffi::CStr, ptr::NonNull};

#[cfg(test)]
mod tests;

#[derive(Debug)]
enum AttackLeechReadError {
    Unavailable(AttackLeechUnavailable),
    Contract(DiscoveryError),
}

impl From<DiscoveryError> for AttackLeechReadError {
    fn from(value: DiscoveryError) -> Self {
        Self::Contract(value)
    }
}

impl From<crate::il2cpp::RuntimeError> for AttackLeechReadError {
    fn from(value: crate::il2cpp::RuntimeError) -> Self {
        Self::Contract(DiscoveryError::Runtime(value))
    }
}

fn unavailable(reason: AttackLeechUnavailable) -> AttackLeechReadError {
    AttackLeechReadError::Unavailable(reason)
}

impl RuntimeAccess {
    // Helpers inherit the complete read_attack_leech contract. Resolve from the
    // known declaring owner, reject inherited substitution and derived shadowing.
    unsafe fn attack_field(&self, owner: Class, name: &CStr, expected: Class, is_static: bool) -> Result<Ptr, DiscoveryError> {
        unsafe {
            let field = self.field(owner, name)?;
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.type_byref)(ty)
                || (self.type_class)(ty) != expected
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) & 0x50 != if is_static { 0x10 } else { 0 }
                || (self.field_offset)(field) == usize::MAX
            {
                return Err(unsupported(format!("attack/leech declared field {} type/owner/storage", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    unsafe fn attack_static_pointer(&self, field: Ptr) -> *mut raw::Object {
        unsafe {
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(field, (&mut value as *mut *mut raw::Object).cast());
            value
        }
    }

    unsafe fn attack_pointer(&self, object: &Root<'_, '_>, field: Ptr) -> Result<*mut raw::Object, DiscoveryError> {
        unsafe {
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut *mut raw::Object).cast());
            Ok(value)
        }
    }

    unsafe fn attack_alive(&self, root: &Root<'_, '_>, object_class: Class, cached: Ptr) -> Result<ObservedObjectIdentity, AttackLeechReadError> {
        unsafe {
            let object = target(root)?;
            self.check_object(object_class, object)?;
            let mut native = 0_usize;
            (self.field_get)(object.as_ptr(), cached, (&mut native as *mut usize).cast());
            if native == 0 {
                return Err(unavailable(AttackLeechUnavailable::NativeObjectDestroyed));
            }
            ObservedObjectIdentity::new(object.as_ptr() as u64, native as u64).map_err(|_| unsupported("attack/leech nonzero identity").into())
        }
    }

    unsafe fn attack_scalar(&self, stats: &Root<'_, '_>, field: Ptr) -> Result<ObservedRate, DiscoveryError> {
        unsafe {
            let mut value = 0_f32;
            (self.field_get)(target(stats)?.as_ptr(), field, (&mut value as *mut f32).cast());
            Ok(ObservedRate { raw_bits: value.to_bits() })
        }
    }

    unsafe fn attack_live_manager(&self, manager: &Root<'_, '_>, object_class: Class, cached: Ptr) -> Result<ObservedObjectIdentity, AttackLeechReadError> {
        // SAFETY: same already-rooted graph contract as attack_alive. A dead
        // manager revokes global admission, unlike a dead local actor/stats cache.
        match unsafe { self.attack_alive(manager, object_class, cached) } {
            Err(AttackLeechReadError::Unavailable(AttackLeechUnavailable::NativeObjectDestroyed)) => Err(unavailable(AttackLeechUnavailable::ManagerAbsent)),
            result => result,
        }
    }

    /// Observe populated fields only. No getPlayerActor, component lookup,
    /// managed cctor, scalar getter, stat refresh, setter or save call is invoked.
    ///
    /// # Safety
    /// All discover/runtime/attachment contracts apply. The attachment must
    /// belong to this exact runtime and stay owned by this OS thread. Call only on Unity's
    /// initialized main thread after the original admitted dispatcher returns.
    /// Nonmoving GC and public field-get/reference/root ABI must match the
    /// validated runtime. PlayerFinder/GameplayEnvironment static storage must
    /// already be initialized/populated by the game. Caller guarantees exclusive
    /// read access: no other thread changes these static/reachable references,
    /// scalar fields, or Unity native-object lifetime through the whole copy.
    /// Strong roots preserve managed wrappers, not native lifetime. No shutdown.
    /// API metadata setup/GC-handle bookkeeping is permitted, game bodies are not.
    /// The caller supplies expected_manager only as an identity comparison token,
    /// never as a dereferenceable pointer; host process/generation must bind the
    /// returned owned observation. Tokens do not stay unique after object death.
    pub unsafe fn read_attack_leech(&self, attached: &AttachedThread<'_>, expected_manager: u64) -> Result<AttackLeechRead, DiscoveryError> {
        // SAFETY: the caller establishes the documented complete graph contract.
        match unsafe { self.copy_attack_leech(attached, expected_manager) } {
            Ok(observation) => Ok(AttackLeechRead::Available(observation)),
            Err(AttackLeechReadError::Unavailable(reason)) => Ok(AttackLeechRead::Unavailable(reason)),
            Err(AttackLeechReadError::Contract(error)) => Err(error),
        }
    }

    // Internal helpers inherit read_attack_leech's complete graph/runtime contract.
    unsafe fn copy_attack_leech(&self, attached: &AttachedThread<'_>, expected_manager: u64) -> Result<AttackLeechObservation, AttackLeechReadError> {
        unsafe {
            if !cfg!(target_arch = "x86_64") || std::mem::size_of::<usize>() != 8 || std::mem::size_of::<f32>() != 4 || std::mem::size_of::<u8>() != 1 {
                return Err(unsupported("attack/leech x64 scalar ABI").into());
            }
            let images = self.images()?;
            let boolean = self.system_class(c"Boolean")?;
            let single = self.system_class(c"Single")?;
            let int_ptr = self.system_class(c"IntPtr")?;
            for (class, code) in [(boolean, 0x02), (single, 0x0c), (int_ptr, 0x18)] {
                let ty = (self.class_type)(class);
                if !(self.class_value)(class) || ty.is_null() || (self.type_byref)(ty) || (self.type_code)(ty) != code {
                    return Err(unsupported("attack/leech primitive code/value class").into());
                }
            }
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online_field = self.attack_field(environment, c"_isOnlinePlay", boolean, true)?;
            let offline = || -> Result<(), AttackLeechReadError> {
                let mut online = 1_u8;
                (self.static_get)(online_field, (&mut online as *mut u8).cast());
                if online > 1 {
                    return Err(unsupported("attack/leech Boolean representation").into());
                }
                if online != 0 {
                    return Err(unavailable(AttackLeechUnavailable::OfflineRequired));
                }
                Ok(())
            };
            offline()?;
            let finder = self.class(&images, c"", c"PlayerFinder")?;
            let manager_class = self.class(&images, c"", c"ItemContainersManager")?;
            let actor_class = self.class(&images, c"", c"Actor")?;
            let base_stats = self.class(&images, c"", c"BaseStats")?;
            let character_stats = self.class(&images, c"", c"CharacterStats")?;
            let object_class = self.class(&images, c"UnityEngine", c"Object")?;
            // Reference slots are copied into pointer-sized storage only after
            // rejecting value-type substitutions, before any static/field read.
            for class in [finder, manager_class, actor_class, base_stats, character_stats, object_class] {
                if (self.class_value)(class) {
                    return Err(unsupported("attack/leech reference class").into());
                }
            }
            let cached = self.attack_field(object_class, c"m_CachedPtr", int_ptr, false)?;
            let manager_field = self.attack_field(finder, c"localItemContainersManager", manager_class, true)?;
            let actor_field = self.attack_field(finder, c"playerActorComponent", actor_class, true)?;
            let manager_actor = self.attack_field(manager_class, c"actor", actor_class, false)?;
            let actor_manager = self.attack_field(actor_class, c"itemContainersManager", manager_class, false)?;
            let actor_stats = self.attack_field(actor_class, c"stats", base_stats, false)?;
            let stats_actor = self.attack_field(base_stats, c"actor", actor_class, false)?;
            let attack_field = self.attack_field(character_stats, c"attackRate", single, false)?;
            let leech_field = self.attack_field(base_stats, c"increasedLeechRate", single, false)?;

            let manager = NonNull::new(self.attack_static_pointer(manager_field)).ok_or_else(|| unavailable(AttackLeechUnavailable::ManagerAbsent))?;
            let manager = attached.root(manager)?;
            self.check_object(manager_class, target(&manager)?)?;
            let manager_identity = self.attack_live_manager(&manager, object_class, cached)?;
            if expected_manager == 0 || manager_identity.wrapper_token() != expected_manager {
                return Err(unavailable(AttackLeechUnavailable::ManagerChanged));
            }
            let actor = NonNull::new(self.attack_static_pointer(actor_field)).ok_or_else(|| unavailable(AttackLeechUnavailable::ActorCacheAbsent))?;
            let actor = attached.root(actor)?;
            self.check_object(actor_class, target(&actor)?)?;
            let actor_identity = self.attack_alive(&actor, object_class, cached)?;
            if self.attack_pointer(&manager, manager_actor)? != target(&actor)?.as_ptr() || self.attack_pointer(&actor, actor_manager)? != target(&manager)?.as_ptr() {
                return Err(unavailable(AttackLeechUnavailable::GraphChanged));
            }
            let stats = NonNull::new(self.attack_pointer(&actor, actor_stats)?).ok_or_else(|| unavailable(AttackLeechUnavailable::StatsAbsent))?;
            let stats = attached.root(stats)?;
            // Exact class is a deliberate initial adapter bound; unknown derived
            // layouts/types remain unsupported, even if an inherited field exists.
            self.check_object(base_stats, target(&stats)?)?;
            if (self.object_class)(target(&stats)?.as_ptr()) != character_stats {
                return Err(unavailable(AttackLeechUnavailable::UnsupportedStatsClass));
            }
            let stats_identity = self.attack_alive(&stats, object_class, cached)?;
            if self.attack_pointer(&stats, stats_actor)? != target(&actor)?.as_ptr() {
                return Err(unavailable(AttackLeechUnavailable::GraphChanged));
            }
            let attack_rate = self.attack_scalar(&stats, attack_field)?;
            let increased_leech_rate = self.attack_scalar(&stats, leech_field)?;
            // These checks detect drift, but do not replace the no-concurrent-
            // mutation/lifetime contract with an atomic-snapshot claim.
            offline()?;
            let current_manager = self.attack_static_pointer(manager_field);
            if current_manager.is_null() {
                return Err(unavailable(AttackLeechUnavailable::ManagerAbsent));
            }
            if current_manager != target(&manager)?.as_ptr() || self.attack_live_manager(&manager, object_class, cached)? != manager_identity {
                return Err(unavailable(AttackLeechUnavailable::ManagerChanged));
            }
            if self.attack_static_pointer(actor_field) != target(&actor)?.as_ptr()
                || self.attack_pointer(&manager, manager_actor)? != target(&actor)?.as_ptr()
                || self.attack_pointer(&actor, actor_manager)? != target(&manager)?.as_ptr()
                || self.attack_pointer(&actor, actor_stats)? != target(&stats)?.as_ptr()
                || self.attack_pointer(&stats, stats_actor)? != target(&actor)?.as_ptr()
                || self.attack_alive(&actor, object_class, cached)? != actor_identity
                || self.attack_alive(&stats, object_class, cached)? != stats_identity
            {
                return Err(unavailable(AttackLeechUnavailable::GraphChanged));
            }
            Ok(AttackLeechObservation {
                manager: manager_identity,
                actor: actor_identity,
                stats: stats_identity,
                attack_rate,
                increased_leech_rate,
            })
        }
    }
}
