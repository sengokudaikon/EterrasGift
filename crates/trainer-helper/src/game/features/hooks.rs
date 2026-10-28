use super::damage::{DamageLayout, OffenseLayout};
use crate::game::*;
use crate::hooks::leaf::LeafEffect;
use eg_il2cpp_runtime::AttachedThread;
use eg_trainer_wire::hooks::GameplayHook;

pub struct GameplayHookTarget {
    pub name: String,
    pub address: NonNull<u8>,
    pub bytes: [u8; 16],
    pub effect: LeafEffect,
    pub receiver_class: Option<NonNull<c_void>>,
    pub linked_offset: Option<u32>,
}

impl RuntimeAccess {
    /// Resolve the concrete targets of one feature without invoking their bodies.
    ///
    /// # Safety
    /// Same retained metadata, attached thread, MethodInfo layout and readable
    /// GameAssembly span contract as `discover`. Exclusive patch ownership is
    /// additionally required before using the returned code addresses.
    pub unsafe fn gameplay_hook_targets(
        &self,
        // Not read: holding it proves this thread is attached, which the metadata and static reads below need.
        _attached: &AttachedThread<'_>,
        hook: GameplayHook,
        module_base: NonNull<u8>,
        module_size: usize,
    ) -> Result<Vec<GameplayHookTarget>, GameError> {
        let damage = if hook.is_damage() {
            // SAFETY: attached main thread, initialized game statics and retained metadata.
            Some(unsafe { self.damage_layout()? })
        } else {
            None
        };
        // Only concrete implemented features belong here.
        let specs: &[(&CStr, &CStr, bool, &[&str], LeafEffect)] = match hook {
            GameplayHook::IncomingDamage | GameplayHook::DamageDealt => &[(c"BaseHealth", c"HealthDamage", true, &["System.Single"], damage.unwrap().effect())],
            GameplayHook::MysteriousRift => &[(c"TimeBeastTracker", c"NextEncounterShouldBeEggs", true, &[], LeafEffect::SelectableBoolean)],
            GameplayHook::Offhand => &[
                (
                    c"OneSlotItemContainerWithCharacterRequirements",
                    c"alternateSufficientBaseTypeConditionFulfilled",
                    true,
                    &["System.Int32"],
                    LeafEffect::Boolean(true),
                ),
                (
                    c"OffhandItemContainer",
                    c"IncompatibleDueTo2hWeapon",
                    false,
                    &["AdditionalBaseTypeProvider", "ItemData", "ItemData"],
                    LeafEffect::Boolean(false),
                ),
                (
                    c"LinkedOneSlotItemContainer",
                    c"incompatibleWithItemInLinkedSlot",
                    true,
                    &["ItemData"],
                    LeafEffect::Boolean(false),
                ),
                (
                    c"OffhandItemContainer",
                    c"EnforceWeaponLimitations",
                    true,
                    &["System.Boolean", "UnityEngine.Vector2Int"],
                    LeafEffect::Void,
                ),
            ],
        };
        // SAFETY: caller supplies immutable metadata and a bounded retained module.
        unsafe {
            let images = self.images()?;
            let mut targets = Vec::new();
            for &(class, name, instance, params, ref effect) in specs {
                let owner = self.class(&images, c"", class)?;
                let scoped = class == c"OneSlotItemContainerWithCharacterRequirements" || class == c"LinkedOneSlotItemContainer";
                let receiver_class = if scoped {
                    let receiver = self.class(&images, c"", c"OffhandItemContainer")?;
                    if (self.class_value)(receiver) || !(self.assignable)(owner, receiver) {
                        return Err(unsupported("offhand receiver inheritance"));
                    }
                    NonNull::new(receiver.cast())
                } else {
                    None
                };
                let linked_offset = if class == c"LinkedOneSlotItemContainer" {
                    let field = self.field(owner, c"linkedSlot")?;
                    let offset = (self.field_offset)(field);
                    if (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
                        || self.field_class(field, "hook")? != owner
                        || !(16..=16384).contains(&offset)
                        || offset % 8 != 0
                    {
                        return Err(unsupported("offhand linked-slot reference layout"));
                    }
                    Some(offset as u32)
                } else {
                    None
                };
                let mut found = None;
                for method in self.methods(owner) {
                    let method = method?;
                    let actual = (self.method_name)(method);
                    if actual.is_null() {
                        return Err(unsupported("null hook method name"));
                    }
                    if metadata_text(actual) != name || (self.method_params)(method) as usize != params.len() {
                        continue;
                    }
                    let returns = (self.method_return)(method);
                    let mut implementation = 0;
                    let return_name = match effect {
                        LeafEffect::Void => "System.Void",
                        LeafEffect::Generated(_) => "System.Single", // the damage leaf
                        _ => "System.Boolean",
                    };
                    if found.is_some()
                        || (self.method_instance)(method) != instance
                        || (self.method_generic)(method)
                        || (self.method_inflated)(method)
                        || (self.method_flags)(method, &mut implementation) & (raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT) != 0
                        || returns.is_null()
                        || (self.type_byref)(returns)
                        || self.type_name(returns)? != return_name
                    {
                        return Err(unsupported(format!("hook contract: {}", name.to_string_lossy())));
                    }
                    for (index, expected) in params.iter().enumerate() {
                        let param = (self.method_param)(method, index as u32);
                        if param.is_null() || (self.type_byref)(param) || self.type_name(param)? != *expected {
                            return Err(unsupported(format!("hook parameter: {}", name.to_string_lossy())));
                        }
                    }
                    found = Some(method);
                }
                let method = found.ok_or_else(|| unsupported(format!("missing hook: {}", name.to_string_lossy())))?;
                let address = read_value::<*mut u8>(method.cast());
                (address as usize)
                    .checked_sub(module_base.as_ptr() as usize)
                    .filter(|offset| offset.checked_add(16).is_some_and(|end| end <= module_size))
                    .ok_or_else(|| unsupported("hook module range"))?;
                targets.push(GameplayHookTarget {
                    name: format!("{}.{}", class.to_string_lossy(), name.to_string_lossy()),
                    address: NonNull::new(address).ok_or_else(|| unsupported("hook address"))?,
                    bytes: read_value(address.cast()),
                    effect: effect.clone(),
                    receiver_class,
                    linked_offset,
                });
            }
            Ok(targets)
        }
    }
}

impl RuntimeAccess {
    // Called only after offline status admission. No instance or static getters are
    // invoked by gameplay threads; IL2CPP owns these static-storage allocations.
    unsafe fn damage_layout(&self) -> Result<DamageLayout, GameError> {
        // SAFETY: same initialized metadata, attached-thread and readable field contract as status.
        unsafe {
            let images = self.images()?;
            let health = self.class(&images, c"", c"BaseHealth")?;
            let actor = self.class(&images, c"", c"Actor")?;
            let field = self.field(health, c"actor")?;
            let offset = (self.field_offset)(field);
            if (self.class_value)(health)
                || (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
                || self.field_class(field, "hook")? != actor
                || !(16..=16384).contains(&offset)
                || offset % 8 != 0
            {
                return Err(unsupported("health actor reference layout"));
            }
            let player_class = self.class(&images, c"", c"PlayerFinder")?;
            let player = self.static_field(player_class, c"playerActorComponent", "Actor")?;
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
            let player_storage = self.hook_static_storage(player_class, player, 8)?;
            let online_storage = self.hook_static_storage(environment, online, 1)?;
            Ok(DamageLayout {
                player_storage,
                online_storage,
                actor_offset: offset as u32,
                offense: Some(OffenseLayout {
                    current_health: self.hook_field(health, c"currentHealth", "System.Single", 4)?,
                    alignment_manager: self.hook_field(actor, c"alignmentManager", "AlignmentManager", 8)?,
                    alignment: self.hook_field(self.class(&images, c"", c"AlignmentManager")?, c"alignment", "Alignment", 8)?,
                    alignment_id: self.hook_field(self.class(&images, c"", c"Alignment")?, c"alignmentID", "System.Byte", 1)?,
                    is_foe: self.hook_field(self.class(&images, c"", c"Alignment")?, c"isFoe", "System.Boolean[]", 8)?,
                }),
            })
        }
    }
    unsafe fn hook_field(&self, owner: Class, name: &CStr, expected: &str, alignment: usize) -> Result<u32, GameError> {
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
                return Err(unsupported(format!("damage field layout: {}", name.to_string_lossy())));
            }
            Ok(offset as u32)
        }
    }
    unsafe fn hook_static_storage(&self, owner: Class, field: Ptr, alignment: usize) -> Result<NonNull<c_void>, GameError> {
        // SAFETY: static field type/flags validated by caller; initialize through the
        // public field API, then obtain the owning storage through the public class API.
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
