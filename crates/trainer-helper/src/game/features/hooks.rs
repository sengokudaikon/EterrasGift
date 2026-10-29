//! Helper-owned patch groups and method contracts. Each feature supplies its own targets and behavior.
use super::stat::StatFeature;
use crate::game::*;
use crate::hooks::leaf::LeafEffect;
use crate::hooks::postfix::PostfixEffect;
use eg_trainer_wire::hooks::GameplayHook;

/// Ownership of a set of entry patches. Several controls can share a group, and one group can patch several methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupId {
    MysteriousRift,
    Offhand,
    Damage,
    AttackRate,
    LeechRate,
    ZeroManaCost,
    MaxSkillLevel,
    ItemRequirements,
    SetBonus,
    FreeSkillPoints,
    FreePassivePoints,
    PassiveChain,
    FreeWeaverPoints,
    ForgeLevel,
    WovenEnchanter,
    PickupRange,
    HunterCost,
    EternityCache,
    FavorCache,
    Nemesis,
    OmenWindows,
    ClassExperimentals,
    RunePrison,
}
impl GroupId {
    pub(crate) const COUNT: usize = 23;
}

#[derive(Clone, Copy)]
pub(crate) enum Behavior {
    Leaf,
    Postfix,
    Damage,
    Scalar(&'static StatFeature),
    Patch,
}

type ResolveTargets = unsafe fn(&RuntimeAccess, NonNull<u8>, usize) -> Result<Vec<GameplayHookTarget>, GameError>;

pub(crate) struct HookFeature {
    pub group: GroupId,
    pub behavior: Behavior,
    pub targets: ResolveTargets,
}

pub(crate) fn definition(hook: GameplayHook) -> &'static HookFeature {
    match hook {
        GameplayHook::MysteriousRift => &super::mysterious_rift::FEATURE,
        GameplayHook::Offhand => &super::offhand::FEATURE,
        GameplayHook::IncomingDamage | GameplayHook::DamageDealt => &super::damage::FEATURE,
        GameplayHook::AttackRate => &super::attack_rate::FEATURE,
        GameplayHook::LeechRate => &super::leech_rate::FEATURE,
        GameplayHook::ZeroManaCost => &super::zero_mana_cost::FEATURE,
        GameplayHook::MaxSkillLevel => &super::patches::max_skill_level::FEATURE,
        GameplayHook::ItemRequirements => &super::patches::item_requirements::FEATURE,
        GameplayHook::SetBonus => &super::patches::set_bonus::FEATURE,
        GameplayHook::FreeSkillPoints => &super::patches::free_skill_points::FEATURE,
        GameplayHook::FreePassivePoints => &super::patches::free_passive_points::FEATURE,
        GameplayHook::PassiveChain => &super::patches::passive_chain::FEATURE,
        GameplayHook::FreeWeaverPoints => &super::patches::free_weaver_points::FEATURE,
        GameplayHook::ForgeLevel => &super::patches::forge_level::FEATURE,
        GameplayHook::WovenEnchanter => &super::patches::woven_enchanter::FEATURE,
        GameplayHook::PickupRange => &super::patches::pickup_range::FEATURE,
        GameplayHook::HunterCost => &super::patches::hunter_cost::FEATURE,
        GameplayHook::EternityCache => &super::patches::eternity_cache::FEATURE,
        GameplayHook::FavorCache => &super::patches::favor_cache::FEATURE,
        GameplayHook::Nemesis => &super::patches::nemesis::FEATURE,
        GameplayHook::OmenWindows => &super::patches::omen_windows::FEATURE,
        GameplayHook::ClassExperimentals => &super::patches::class_experimentals::FEATURE,
        GameplayHook::RunePrison => &super::patches::rune_prison::FEATURE,
    }
}

pub(crate) struct MethodSpec {
    pub class: &'static CStr,
    pub method: &'static CStr,
    pub instance: bool,
    pub params: &'static [&'static str],
    pub returns: &'static str,
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
