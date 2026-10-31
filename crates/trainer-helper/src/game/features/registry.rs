//! Helper-owned patch groups and method contracts. Each feature supplies its own targets and behavior.
use super::stat::StatFeature;
use crate::game::*;
use crate::hooks::leaf::LeafEffect;
use crate::hooks::postfix::PostfixEffect;
pub(crate) use eg_trainer_wire::hooks::GameplayHook;
mod shared;
pub(crate) use shared::{ControlSetting, OFF, SharedSettings};

#[derive(Clone, Copy)]
pub(crate) enum Behavior {
    Leaf,
    Postfix,
    SharedLeaf(&'static SharedSettings),
    Scalar(&'static StatFeature),
    Patch,
}

type ResolveTargets = unsafe fn(&RuntimeAccess, NonNull<u8>, usize) -> Result<Vec<GameplayHookTarget>, GameError>;

/// A hook feature: the controls it serves, how it hooks, and how it finds its targets. Several controls can share
/// one feature (the two damage settings), and one feature can patch several methods.
pub(crate) struct HookFeature {
    pub controls: &'static [GameplayHook],
    pub behavior: Behavior,
    pub targets: Targets,
}

/// How a feature finds what it changes: its own resolver, or the code patches it lists.
pub(crate) enum Targets {
    Resolve(ResolveTargets),
    Patches(&'static [super::patches::PatchSpec]),
}

impl HookFeature {
    /// A feature that only patches code, as `specs` lists it.
    pub(crate) const fn patches(controls: &'static [GameplayHook], specs: &'static [super::patches::PatchSpec]) -> Self {
        Self {
            controls,
            behavior: Behavior::Patch,
            targets: Targets::Patches(specs),
        }
    }

    /// # Safety
    /// Attached immutable runtime metadata, a retained readable GameAssembly of `size` bytes at `module`, and exclusive
    /// ownership of the code it may patch, as each resolver states.
    pub(crate) unsafe fn targets(&self, access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
        // SAFETY: the caller's contract is each resolver's.
        unsafe {
            match self.targets {
                Targets::Resolve(resolve) => resolve(access, module, size),
                Targets::Patches(specs) => access.patch_targets(specs, module, size),
            }
        }
    }
}

/// Every hook feature. Its place here is its group: the slot that owns its patches while the helper runs.
pub(crate) const FEATURES: &[&HookFeature] = &[
    &super::mysterious_rift::FEATURE,
    &super::any_offhand::FEATURE,
    &super::damage::FEATURE,
    &super::fixed_attack_rate::FEATURE,
    &super::fixed_leech_rate::FEATURE,
    &super::results::no_mana_cost::FEATURE,
    &super::patches::skills_to_level_20::FEATURE,
    &super::patches::equip_without_requirements::FEATURE,
    &super::patches::full_set_bonus::FEATURE,
    &super::patches::unlimited_skill_points::FEATURE,
    &super::patches::unlimited_passive_points::FEATURE,
    &super::patches::any_mastery_passives::FEATURE,
    &super::patches::unlimited_weaver_points::FEATURE,
    &super::patches::forge_above_level::FEATURE,
    &super::patches::unlimited_woven_enchanter::FEATURE,
    &super::patches::pickup_anywhere::FEATURE,
    &super::patches::free_hunter_vendor::FEATURE,
    &super::patches::endgame_cache::FEATURE,
    &super::patches::favor_cache::FEATURE,
    &super::patches::nemesis::FEATURE,
    &super::patches::omen_windows::FEATURE,
    &super::patches::class_experimentals::FEATURE,
    &super::patches::rune_prison_complex::FEATURE,
    &super::shards_to_99::FEATURE,
    &super::runes_glyphs_to_99::FEATURE,
    &super::results::max_random_rolls::FEATURE,
    &super::results::boss_loot_chance::FEATURE,
    &super::results::tier_7_affixes::FEATURE,
    &super::results::max_bonus_stability::FEATURE,
    &super::results::ability_speed::FEATURE,
    &super::results::one_echo_type::FEATURE,
    &super::gains::experience_multiplier::FEATURE,
    &super::gains::faction_gains::FEATURE,
    &super::gains::rank_up_per_gain::FEATURE,
    &super::player_only::several_primordials::FEATURE,
    &super::player_only::fixed_area::FEATURE,
    &super::player_only::refill_mana::FEATURE,
    &super::results::movement_speed::FEATURE,
];

/// Each control's group, checked when the crate builds: every control belongs to exactly one feature.
const GROUPS: [usize; GameplayHook::COUNT] = {
    const NONE: usize = usize::MAX;
    let mut groups = [NONE; GameplayHook::COUNT];
    let mut group = 0;
    while group < FEATURES.len() {
        let controls = FEATURES[group].controls;
        let mut index = 0;
        while index < controls.len() {
            assert!(groups[controls[index] as usize] == NONE, "a control belongs to two features");
            groups[controls[index] as usize] = group;
            index += 1;
        }
        group += 1;
    }
    let mut hook = 0;
    while hook < GameplayHook::COUNT {
        assert!(groups[hook] != NONE, "a control has no feature");
        hook += 1;
    }
    groups
};

/// The slot, its feature's place in `FEATURES`, that owns this control's patches.
pub(crate) const fn slot_of(hook: GameplayHook) -> usize {
    GROUPS[hook as usize]
}

pub(crate) fn definition(hook: GameplayHook) -> &'static HookFeature {
    FEATURES[slot_of(hook)]
}

pub(crate) struct MethodSpec {
    pub class: &'static CStr,
    pub method: &'static CStr,
    pub instance: bool,
    pub params: &'static [&'static str],
    pub returns: &'static str,
}
impl MethodSpec {
    /// Its native argument positions: the receiver when it has one, its parameters, and the MethodInfo.
    pub(crate) const fn positions(&self) -> u8 {
        let positions = self.params.len() + self.instance as usize + 1;
        assert!(positions <= u8::MAX as usize, "bounded explicit method contract");
        positions as u8
    }
}

pub(crate) struct ResolvedMethod {
    pub name: String,
    pub address: NonNull<u8>,
    pub bytes: [u8; 16],
}
impl ResolvedMethod {
    pub(crate) fn leaf(self, effect: LeafEffect, receiver_class: Option<NonNull<c_void>>, linked_offset: Option<u32>) -> GameplayHookTarget {
        GameplayHookTarget {
            method: self,
            effect: TargetEffect::Leaf {
                effect,
                receiver_class,
                linked_offset,
            },
        }
    }
    pub(crate) fn scalar(self, stack_argument: bool) -> GameplayHookTarget {
        GameplayHookTarget {
            method: self,
            effect: TargetEffect::Scalar { stack_argument },
        }
    }
    pub(crate) fn postfix(self, effect: PostfixEffect) -> GameplayHookTarget {
        GameplayHookTarget {
            method: self,
            effect: TargetEffect::Postfix(effect),
        }
    }
}

pub(crate) enum TargetEffect {
    Leaf {
        effect: LeafEffect,
        receiver_class: Option<NonNull<c_void>>,
        linked_offset: Option<u32>,
    },
    Scalar {
        stack_argument: bool,
    },
    Postfix(PostfixEffect),
    Patch(CodeChange),
}
pub(crate) struct CodeChange {
    pub address: NonNull<u8>,
    pub expected: Vec<u8>,
    pub replacement: Vec<u8>,
}
pub(crate) struct GameplayHookTarget {
    pub method: ResolvedMethod,
    pub effect: TargetEffect,
}

impl RuntimeAccess {
    /// Resolve one explicitly stated ABI without invoking its body. Feature-specific layouts and receiver rules
    /// belong to the feature that calls this validator; the return contract never depends on generated code.
    ///
    /// # Safety
    /// Attached thread, retained metadata and MethodInfo layout as `discover`; `module_base..+module_size` is the
    /// retained readable GameAssembly span. Exclusive patch ownership is required before using the returned address.
    pub(crate) unsafe fn hook_method(&self, spec: &MethodSpec, module_base: NonNull<u8>, module_size: usize) -> Result<ResolvedMethod, GameError> {
        // SAFETY: caller supplies immutable metadata and a bounded retained module.
        unsafe {
            let images = self.images()?;
            let owner = self.class(&images, c"", spec.class)?;
            self.hook_method_on(owner, spec, module_base, module_size)
        }
    }
    /// Same contract as hook_method; the caller selects an exact namespace or nested owner from retained metadata.
    pub(super) unsafe fn hook_method_on(&self, owner: Class, spec: &MethodSpec, module_base: NonNull<u8>, module_size: usize) -> Result<ResolvedMethod, GameError> {
        // SAFETY: explicit retained owner and the same exact signature/module contract as hook_method.
        unsafe {
            let method = self.hook_method_contract(owner, spec)?;
            let address = read_value::<*mut u8>(method.cast());
            (address as usize)
                .checked_sub(module_base.as_ptr() as usize)
                .filter(|offset| offset.checked_add(16).is_some_and(|end| end <= module_size))
                .ok_or_else(|| unsupported("hook module range"))?;
            Ok(ResolvedMethod {
                name: format!("{}.{}", spec.class.to_string_lossy(), spec.method.to_string_lossy()),
                address: NonNull::new(address).ok_or_else(|| unsupported("hook address"))?,
                bytes: read_value(address.cast()),
            })
        }
    }
    pub(super) unsafe fn hook_online_storage(&self, images: &Images) -> Result<NonNull<c_void>, GameError> {
        // SAFETY: admitted attached thread; initialize the process-owned Boolean static through the public API.
        unsafe {
            let environment = self.class(images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
            self.hook_static_storage(environment, online, 1)
        }
    }
    /// The storage of `PlayerFinder.playerActorComponent`, the actor in play, for code that compares a receiver with it.
    /// # Safety
    /// Same contract as `hook_online_storage`.
    pub(super) unsafe fn hook_player_storage(&self, images: &Images) -> Result<NonNull<c_void>, GameError> {
        // SAFETY: admitted attached thread; initialize the process-owned reference static through the public API.
        unsafe {
            let finder = self.class(images, c"", c"PlayerFinder")?;
            let player = self.static_field(finder, c"playerActorComponent", "Actor")?;
            self.hook_static_storage(finder, player, 8)
        }
    }
    /// Process-owned storage of the current local item manager; no getter runs on the gameplay caller.
    /// # Safety
    /// Same contract as `hook_online_storage`.
    pub(super) unsafe fn hook_manager_storage(&self, images: &Images) -> Result<NonNull<c_void>, GameError> {
        // SAFETY: admitted attached thread; resolve and initialize the exact reference static through the public API.
        unsafe {
            let finder = self.class(images, c"", c"PlayerFinder")?;
            let manager = self.static_field(finder, c"localItemContainersManager", "ItemContainersManager")?;
            self.hook_static_storage(finder, manager, 8)
        }
    }
    // Resolve the complete parameter signature before checking the selected overload's ABI and uniqueness.
    pub(super) unsafe fn hook_method_contract(&self, owner: Class, spec: &MethodSpec) -> Result<Method, GameError> {
        // SAFETY: immutable attached metadata and class descriptor retained by the caller.
        unsafe {
            let mut found = None;
            for method in self.methods(owner) {
                let method = method?;
                let actual = (self.method_name)(method);
                if actual.is_null() {
                    return Err(unsupported("null hook method name"));
                }
                if metadata_text(actual) != spec.method || (self.method_params)(method) as usize != spec.params.len() {
                    continue;
                }
                let mut params_match = true;
                for (index, expected) in spec.params.iter().enumerate() {
                    let param = (self.method_param)(method, index as u32);
                    if param.is_null() {
                        return Err(unsupported("null hook parameter type"));
                    }
                    if (self.type_byref)(param) != expected.ends_with('&') || self.type_name(param)? != *expected {
                        params_match = false;
                        break;
                    }
                }
                if !params_match {
                    continue;
                }
                let returns = (self.method_return)(method);
                let mut implementation = 0;
                if found.is_some()
                    || (self.method_instance)(method) != spec.instance
                    || (self.method_generic)(method)
                    || (self.method_inflated)(method)
                    || (self.method_flags)(method, &mut implementation) & (raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT) != 0
                    || returns.is_null()
                    || (self.type_byref)(returns)
                    || self.type_name(returns)? != spec.returns
                {
                    return Err(unsupported(format!("hook contract: {}", spec.method.to_string_lossy())));
                }
                found = Some(method);
            }
            found.ok_or_else(|| unsupported(format!("missing hook: {}", spec.method.to_string_lossy())))
        }
    }
    /// The offset of `owner`'s instance field `name`, whose type's class is exactly `expected`, aligned to `alignment`
    /// in a reference object's instance storage.
    /// # Safety
    /// Immutable attached metadata; `owner` and `expected` are retained class descriptors.
    pub(super) unsafe fn hook_field_of(&self, owner: Class, name: &CStr, expected: Class, alignment: usize) -> Result<u32, GameError> {
        // SAFETY: exact non-byref field class and instance storage are checked before returning the aligned offset.
        unsafe {
            let field = self.instance_field_in(owner, name, expected, "hook")?;
            let offset = (self.field_offset)(field);
            if (self.class_value)(owner) || !(16..=16384).contains(&offset) || !offset.is_multiple_of(alignment) {
                return Err(unsupported(format!("hook field layout: {}", name.to_string_lossy())));
            }
            Ok(offset as u32)
        }
    }
    pub(super) unsafe fn hook_field(&self, owner: Class, name: &CStr, expected: &str, alignment: usize) -> Result<u32, GameError> {
        // SAFETY: immutable attached metadata, instance fields on readable managed objects.
        unsafe {
            let field = self.field(owner, name)?;
            let offset = (self.field_offset)(field);
            let ty = (self.field_type)(field);
            if (self.class_value)(owner)
                || (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
                || ty.is_null()
                || (self.type_byref)(ty)
                || self.type_name(ty)? != expected
                || !(16..=16384).contains(&offset)
                || !offset.is_multiple_of(alignment)
            {
                return Err(unsupported(format!("hook field layout: {}", name.to_string_lossy())));
            }
            Ok(offset as u32)
        }
    }
    pub(crate) unsafe fn hook_static_storage(&self, owner: Class, field: Ptr, alignment: usize) -> Result<NonNull<c_void>, GameError> {
        // SAFETY: static field type/flags validated by caller; initialize through the public field API, then obtain
        // the owning storage through the public class API. The primitive fits in this eight-byte snapshot.
        unsafe {
            let mut snapshot = 0u64;
            (self.static_get)(field, (&raw mut snapshot).cast());
            let offset = (self.field_offset)(field);
            let storage = (self.class_static_data)(owner);
            if storage.is_null() || offset > 16384 || !offset.is_multiple_of(alignment) {
                return Err(unsupported("hook static storage layout"));
            }
            let pointer = storage.cast::<u8>().add(offset);
            if !(pointer as usize).is_multiple_of(alignment) {
                return Err(unsupported("hook static storage alignment"));
            }
            Ok(NonNull::new(pointer.cast()).unwrap())
        }
    }
}

#[cfg(test)]
mod tests;
