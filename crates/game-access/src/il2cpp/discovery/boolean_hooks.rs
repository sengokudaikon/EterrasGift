use super::*;
use crate::hooks::constant::LeafReturn;
use eg_trainer_wire::hooks::BooleanHook;

pub struct BooleanHookTarget {
    pub name: String,
    pub address: NonNull<u8>,
    pub bytes: [u8; 16],
    pub answer: LeafReturn,
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
    pub unsafe fn boolean_hook_targets(
        &self,
        attached: &AttachedThread<'_>,
        hook: BooleanHook,
        module_base: NonNull<u8>,
        module_size: usize,
    ) -> Result<Vec<BooleanHookTarget>, DiscoveryError> {
        let _ = attached;
        // These are the two implemented features, not a universal hook registry.
        let specs: &[(&CStr, &CStr, bool, &[&str], LeafReturn)] = match hook {
            BooleanHook::MysteriousRift => &[(c"TimeBeastTracker", c"NextEncounterShouldBeEggs", true, &[], LeafReturn::SelectableBoolean)],
            BooleanHook::Offhand => &[
                (
                    c"OneSlotItemContainerWithCharacterRequirements",
                    c"alternateSufficientBaseTypeConditionFulfilled",
                    true,
                    &["System.Int32"],
                    LeafReturn::Boolean(true),
                ),
                (
                    c"OffhandItemContainer",
                    c"IncompatibleDueTo2hWeapon",
                    false,
                    &["AdditionalBaseTypeProvider", "ItemData", "ItemData"],
                    LeafReturn::Boolean(false),
                ),
                (
                    c"LinkedOneSlotItemContainer",
                    c"incompatibleWithItemInLinkedSlot",
                    true,
                    &["ItemData"],
                    LeafReturn::Boolean(false),
                ),
                (
                    c"OffhandItemContainer",
                    c"EnforceWeaponLimitations",
                    true,
                    &["System.Boolean", "UnityEngine.Vector2Int"],
                    LeafReturn::Void,
                ),
            ],
        };
        // SAFETY: caller supplies immutable metadata and a bounded retained module.
        unsafe {
            let images = self.images()?;
            let mut targets = Vec::new();
            for &(class, name, instance, params, answer) in specs {
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
                    if (self.field_flags)(field) & 0x10 != 0 || self.field_class(field)? != owner || !(16..=16384).contains(&offset) || offset % 8 != 0 {
                        return Err(unsupported("offhand linked-slot reference layout"));
                    }
                    Some(offset as u32)
                } else {
                    None
                };
                let mut found = None;
                for method in self.methods(owner)? {
                    let actual = (self.method_name)(method);
                    if actual.is_null() {
                        return Err(unsupported("null hook method name"));
                    }
                    if metadata_text(actual) != name || (self.method_params)(method) as usize != params.len() {
                        continue;
                    }
                    let returns = (self.method_return)(method);
                    let mut implementation = 0;
                    let return_name = if matches!(answer, LeafReturn::Void) { "System.Void" } else { "System.Boolean" };
                    if found.is_some()
                        || (self.method_instance)(method) != instance
                        || (self.method_generic)(method)
                        || (self.method_inflated)(method)
                        || (self.method_flags)(method, &mut implementation) & 0x2400 != 0
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
                targets.push(BooleanHookTarget {
                    name: format!("{}.{}", class.to_string_lossy(), name.to_string_lossy()),
                    address: NonNull::new(address).ok_or_else(|| unsupported("hook address"))?,
                    bytes: read_value(address.cast()),
                    answer,
                    receiver_class,
                    linked_offset,
                });
            }
            Ok(targets)
        }
    }
}
