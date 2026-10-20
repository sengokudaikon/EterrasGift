//! Read an already produced exact property modifier; no producer, description or item call.
use super::{NumericDisplay, RuntimeAccess, Unavailable};
use crate::il2cpp::{AttachedThread, StrongRoot, raw};
use std::{ffi::CStr, ptr::NonNull};

/// Owned field observations and numeric text from one admitted synchronous operation.
/// The raw float bits are independent of any parsed or rounded display scalar.
#[derive(Debug, PartialEq, Eq)]
pub struct NumericModifierDisplay {
    pub value_bits: u32,
    pub minimum_bits: u32,
    pub maximum_bits: u32,
    pub varying: bool,
    pub modifier_type: i32,
    pub special_tag: u8,
    pub formatted: NumericDisplay,
}

impl RuntimeAccess {
    // Inherits format_numeric_modifier's initialized immutable runtime/metadata contract.
    unsafe fn modifier_field(&self, owner: super::Class, name: &CStr, expected: super::Class, code: i32, readonly: bool) -> Result<super::Ptr, Unavailable> {
        // SAFETY: inherits format_numeric_modifier's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let field = self.formatter_field(owner, name, expected, code)?;
            if ((self.field_flags)(field) & 0x20 != 0) != readonly {
                return Err(Unavailable::Contract("modifier field initonly contract"));
            }
            Ok(field)
        }
    }

    /// Copy exact Value/MinValue/MaxValue fields and format Value through the existing text adapter.
    /// Accepts only an already rooted exact ItemTooltipPropertyModifierInfo with exact StatPropertyInfo.
    /// No getter, constructor, tooltip builder, description, culture capture or parser is invoked.
    ///
    /// # Safety
    /// All tables, metadata, roots and attachment belong to the same retained source-audited
    /// initialized Unity6000.4 runtime with nonmoving GC. The caller independently establishes
    /// provenance and lifetime of this modifier through an admitted producer; a wire/address token
    /// cannot establish that authority. This function proves neither item selection nor producer
    /// arithmetic. Real main-thread execution, offline/target/generation/ready-registry admission,
    /// exclusion of shutdown, detach, native/reflection writers, managed reentry and profiler-driven
    /// graph or culture/provider/settings mutation are required for the whole synchronous span.
    /// No yielding, awaiting or UI waiting is allowed. The modifier, property info and override
    /// graph remain unchanged; roots alone do not synchronize them or prove current selection.
    /// Honor all of format_numeric_display's exact native formatter/virtual rounding/BCL body
    /// permissions, including utility/framework class initialization, cache/constant writes,
    /// barriers, globalization, locks, allocation and GC/profiler callbacks. Current culture and
    /// provider closure must independently be admitted; there is no locale/equality inference.
    /// Public field getters synchronously copy only declared widths and retain no native slots.
    /// Exceptions remain rooted data. Native faults/SEH, OOM recovery and rollback are not promised.
    pub unsafe fn format_numeric_modifier(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        modifier: &StrongRoot<'_, '_>,
    ) -> Result<NumericModifierDisplay, Unavailable> {
        if !modifier.belongs_to(attached) {
            return Err(Unavailable::Context);
        }
        // SAFETY: inherits format_numeric_modifier's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let images = self.images()?;
            let leaf = self.class(&images, c"LE.UI.Tooltips", c"ItemTooltipPropertyModifierInfo")?;
            let base = self.class(&images, c"LE.UI.Tooltips", c"ItemTooltipModifierInfo")?;
            let property = self.class(&images, c"", c"BasePropertyInfo")?;
            let stat = self.class(&images, c"", c"StatPropertyInfo")?;
            let stats = self.class(&images, c"", c"BaseStats")?;
            let mod_type = self.catalog_nested(stats, c"ModType")?;
            for class in [leaf, base, property, stat, stats, mod_type] {
                if (self.class_image)(class) != images.game {
                    return Err(Unavailable::Contract("modifier exact game image class"));
                }
            }
            for class in [leaf, base, property, stat, stats] {
                if (self.class_value)(class) || (api.class_is_generic)(class) || (api.class_is_inflated)(class) {
                    return Err(Unavailable::Contract("modifier closed reference classes"));
                }
            }
            if (self.class_parent)(leaf) != base {
                return Err(Unavailable::Contract("modifier exact direct parent"));
            }
            let object = self.formatter_core(c"Object", 0x1c)?;
            if (self.class_parent)(base) != object {
                return Err(Unavailable::Contract("modifier exact corlib Object base"));
            }
            self.exact(modifier, leaf)?;
            let single = self.formatter_core(c"Single", 0x0c)?;
            let byte = self.formatter_core(c"Byte", 0x05)?;
            let i32_class = self.formatter_core(c"Int32", 0x08)?;
            self.formatter_enum(mod_type, i32_class)?;
            let value_field = self.modifier_field(base, c"<Value>k__BackingField", single, 0x0c, false)?;
            let minimum_field = self.modifier_field(leaf, c"<MinValue>k__BackingField", single, 0x0c, true)?;
            let maximum_field = self.modifier_field(leaf, c"<MaxValue>k__BackingField", single, 0x0c, true)?;
            let property_field = self.modifier_field(leaf, c"_propertyInfo", property, 0x12, true)?;
            let mod_type_field = self.modifier_field(leaf, c"_modType", mod_type, 0x11, true)?;
            let tag_field = self.modifier_field(leaf, c"_specialTag", byte, 0x05, true)?;
            let target = modifier.target().ok_or(Unavailable::Context)?;
            let mut child = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target.as_ptr(), property_field, (&mut child as *mut *mut raw::Object).cast());
            let child = NonNull::new(child).ok_or(Unavailable::Contract("null modifier property info"))?;
            // No metadata lookup, managed call or allocation between reference copy and strong root.
            let property = attached.root(child).map_err(super::super::DiscoveryError::from)?;
            self.exact(&property, stat)?;
            let mut value = 0_f32;
            let mut minimum = 0_f32;
            let mut maximum = 0_f32;
            let mut modifier_type = 0_i32;
            let mut special_tag = 0_u8;
            for (field, slot) in [(value_field, &mut value), (minimum_field, &mut minimum), (maximum_field, &mut maximum)] {
                (self.field_get)(target.as_ptr(), field, (slot as *mut f32).cast());
            }
            (self.field_get)(target.as_ptr(), mod_type_field, (&mut modifier_type as *mut i32).cast());
            (self.field_get)(target.as_ptr(), tag_field, (&mut special_tag as *mut u8).cast());
            if !value.is_finite() || !minimum.is_finite() || !maximum.is_finite() || !(0..=3).contains(&modifier_type) {
                return Err(Unavailable::Input);
            }
            // Finite C++ float inequality: both signed zeros compare equal; no endpoint sorting.
            let varying = minimum != maximum;
            let formatted = self.format_numeric_display(attached, api, &property, value, modifier_type, special_tag)?;
            drop(property);
            Ok(NumericModifierDisplay {
                value_bits: value.to_bits(),
                minimum_bits: minimum.to_bits(),
                maximum_bits: maximum.to_bits(),
                varying,
                modifier_type,
                special_tag,
                formatted,
            })
        }
    }
}

#[cfg(test)]
mod tests;
