//! Source-bound numeric property capture without managed calls or lazy singleton initialization.

use super::{
    Class, DiscoveryError, Images, RuntimeAccess,
    forge::{Root, target},
    unsupported,
};
use crate::{
    catalog::{AffixTier, Availability, MAX_DESCRIPTIONS, MAX_DISPLAY_ORDER, MAX_PROPERTIES, NumericProperty, UniqueNumericData},
    il2cpp::AttachedThread,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) struct RawProperty {
    pub property: u8,
    pub tags: i32,
    pub special_tag: u8,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub set_property: bool,
    pub minimum: f32,
    pub maximum: f32,
    pub roll_id: i32,
}

#[derive(Clone, Copy)]
struct Presentation {
    rounding: i32,
    percentage: bool,
    tenth: bool,
    less: bool,
}

pub(super) struct PropertyCatalog<'a, 'r> {
    _master: Root<'a, 'r>,
    definitions: Vec<(u8, Root<'a, 'r>)>,
    default: Option<Root<'a, 'r>>,
    overrides: Class,
    cache: BTreeMap<(usize, i32, u8), Availability<Presentation>>,
}

#[derive(Clone, Copy)]
pub(super) struct NumericBudget {
    used: usize,
}
impl NumericBudget {
    pub(super) fn new() -> Self {
        // Conservatively covers locale/framing and three maximum-sized raw name strings per
        // page entry; names are copied by the surrounding identity reader.
        Self {
            used: 16384 + 3 * crate::catalog::MAX_TEXT_BYTES * crate::catalog::MAX_PAGE_ENTRIES,
        }
    }
    pub(super) fn charge(&mut self, bytes: usize) -> Result<(), DiscoveryError> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|used| *used <= crate::protocol::MAX_RESPONSE_SIZE)
            .ok_or_else(|| unsupported("catalog numeric page exceeds conservative 1 MiB copy budget"))?;
        Ok(())
    }
}

pub(super) struct NumericCapture<'a, 'r> {
    properties: Availability<PropertyCatalog<'a, 'r>>,
    budget: NumericBudget,
}

fn unavailable<T>(reason: impl Into<String>) -> Availability<T> {
    Availability::Unavailable { reason: reason.into() }
}

fn ordinary_property(property: u8) -> bool {
    !matches!(property, 43 | 58 | 98 | 123 | 130)
}

impl RuntimeAccess {
    // Helpers inherit read_catalog_page's initialized main-thread/exclusive graph contract.
    // Missing optional PropertyList is a capability absence; a present malformed type is not.
    pub(super) unsafe fn catalog_numeric<'a, 'r>(&self, attached: &'a AttachedThread<'r>, images: &Images) -> Result<NumericCapture<'a, 'r>, DiscoveryError> {
        // SAFETY: same initialized-runtime and immutable admitted graph as this helper's caller.
        Ok(NumericCapture {
            properties: unsafe { self.catalog_properties(attached, images)? },
            budget: NumericBudget::new(),
        })
    }

    pub(super) unsafe fn catalog_affix_numeric(
        &self,
        attached: &AttachedThread<'_>,
        images: &Images,
        definition: &Root<'_, '_>,
        tiers: &[Root<'_, '_>],
        capture: &mut NumericCapture<'_, '_>,
    ) -> Result<Availability<Vec<AffixTier>>, DiscoveryError> {
        unsafe {
            let properties = match &mut capture.properties {
                Availability::Unavailable { reason } => return Ok(unavailable(reason.clone())),
                Availability::Available(properties) => properties,
            };
            let owner = self.class(images, c"", c"AffixList")?;
            let multi = self.catalog_nested(owner, c"MultiAffix")?;
            let property_class = self.catalog_nested(owner, c"AffixProperty")?;
            let pair_class = self.catalog_nested(owner, c"RollPair")?;
            let multi = (self.object_class)(target(definition)?.as_ptr()) == multi;
            let property_roots = if multi {
                let Some(list) = self.catalog_reference(attached, definition, c"affixProperties")? else {
                    return Ok(unavailable("Multi-affix property list is absent"));
                };
                let roots = self.catalog_list(attached, &list, property_class, MAX_PROPERTIES as u32)?;
                for root in &roots {
                    self.exact(root, property_class)?;
                }
                roots
            } else {
                Vec::new()
            };
            let count = if multi { property_roots.len() } else { 1 };
            let mut output = Vec::with_capacity(tiers.len());
            let mut budget = capture.budget;
            budget.charge(4)?;
            for tier in tiers {
                let extras = if count > 1 {
                    let Some(list) = self.catalog_reference(attached, tier, c"extraRolls")? else {
                        return Ok(unavailable("Multi-affix extra tier ranges are absent"));
                    };
                    let roots = self.catalog_list(attached, &list, pair_class, MAX_PROPERTIES as u32)?;
                    for root in &roots {
                        self.exact(root, pair_class)?;
                    }
                    if roots.len() < count - 1 {
                        return Ok(unavailable("Multi-affix extra tier ranges do not cover every property"));
                    }
                    roots
                } else {
                    Vec::new()
                };
                let mut values = Vec::with_capacity(count);
                budget.charge(4)?;
                for index in 0..count {
                    budget.charge(64)?;
                    let pair = if index == 0 { tier } else { &extras[index - 1] };
                    let object = if multi { &property_roots[index] } else { definition };
                    let raw = self.catalog_raw_property(object, self.primitive(pair, c"minRoll")?, self.primitive(pair, c"maxRoll")?, false)?;
                    match self.catalog_numeric_property(attached, properties, raw)? {
                        Availability::Unavailable { reason } => return Ok(unavailable(reason)),
                        Availability::Available(value) => values.push(value),
                    }
                }
                output.push(AffixTier { properties: values });
            }
            capture.budget = budget;
            Ok(Availability::Available(output))
        }
    }

    pub(super) unsafe fn catalog_unique_numeric(
        &self,
        attached: &AttachedThread<'_>,
        images: &Images,
        definition: &Root<'_, '_>,
        capture: &mut NumericCapture<'_, '_>,
    ) -> Result<Availability<UniqueNumericData>, DiscoveryError> {
        unsafe {
            let metadata = match &mut capture.properties {
                Availability::Unavailable { reason } => return Ok(unavailable(reason.clone())),
                Availability::Available(properties) => properties,
            };
            let mod_class = self.class(images, c"", c"UniqueItemMod")?;
            let description_class = self.class(images, c"", c"ItemTooltipDescription")?;
            let entry_class = self.class(images, c"", c"UniqueModDisplayListEntry")?;
            let Some(mods) = self.catalog_reference(attached, definition, c"mods")? else {
                return Ok(unavailable("Unique modifier list is absent"));
            };
            let mut budget = capture.budget;
            budget.charge(12)?;
            let mut properties = Vec::new();
            for object in self.catalog_list(attached, &mods, mod_class, MAX_PROPERTIES as u32)? {
                self.exact(&object, mod_class)?;
                budget.charge(64)?;
                let raw = self.catalog_raw_property(&object, self.primitive(&object, c"value")?, self.primitive(&object, c"maxValue")?, true)?;
                match self.catalog_numeric_property(attached, metadata, raw)? {
                    Availability::Unavailable { reason } => return Ok(unavailable(reason)),
                    Availability::Available(value) => properties.push(value),
                }
            }
            let Some(list) = self.catalog_reference(attached, definition, c"tooltipDescriptions")? else {
                return Ok(unavailable("Unique tooltip description list is absent"));
            };
            let mut descriptions = Vec::new();
            for object in self.catalog_list(attached, &list, description_class, MAX_DESCRIPTIONS as u32)? {
                self.exact(&object, description_class)?;
                let text = self.text(attached, &object, c"description")?;
                budget.charge(4 + text.len())?;
                descriptions.push(text);
            }
            let Some(list) = self.catalog_reference(attached, definition, c"tooltipEntries")? else {
                return Ok(unavailable("Unique tooltip display-order list is absent"));
            };
            let mut display_order = Vec::new();
            for object in self.catalog_list(attached, &list, entry_class, MAX_DISPLAY_ORDER as u32)? {
                self.exact(&object, entry_class)?;
                budget.charge(4)?;
                display_order.push(i32::from(self.primitive::<u8>(&object, c"modDisplay")?));
            }
            capture.budget = budget;
            Ok(Availability::Available(UniqueNumericData {
                properties,
                descriptions,
                display_order,
            }))
        }
    }

    pub(super) unsafe fn catalog_properties<'a, 'r>(&self, attached: &'a AttachedThread<'r>, images: &Images) -> Result<Availability<PropertyCatalog<'a, 'r>>, DiscoveryError> {
        unsafe {
            let mut owner = (self.class_from_name)(images.game, c"".as_ptr(), c"PropertyList".as_ptr());
            if owner.is_null() {
                for image in &images.all {
                    let candidate = (self.class_from_name)(*image, c"".as_ptr(), c"PropertyList".as_ptr());
                    super::record_class(&mut owner, candidate, c"PropertyList")?;
                }
            }
            if owner.is_null() {
                return Ok(unavailable("This runtime does not expose the optional PropertyList numeric capability"));
            }
            let ready = self.static_field(owner, c"instanceFound", "System.Boolean")?;
            if self.field_class(ready)? != self.system_class(c"Boolean")? {
                return Err(unsupported("numeric PropertyList readiness class"));
            }
            let mut value = 0_u8;
            (self.static_get)(ready, (&mut value as *mut u8).cast());
            match value {
                0 => return Ok(unavailable("PropertyList definitions are not already initialized")),
                1 => {}
                _ => return Err(unsupported("numeric readiness Boolean representation")),
            }
            let Some(master) = self.catalog_static(attached, owner, c"instance", owner)? else {
                return Ok(unavailable("PropertyList instance is absent"));
            };
            self.exact(&master, owner)?;
            if !self.catalog_live(attached, images, &master)? {
                return Ok(unavailable("PropertyList native object is destroyed"));
            }
            let info_class = self.class(images, c"", c"StatPropertyInfo")?;
            let key_class = self.class(images, c"", c"PropertyInfoWithLocKey")?;
            let base_class = self.class(images, c"", c"BasePropertyInfo")?;
            if (self.class_parent)(info_class) != key_class || (self.class_parent)(key_class) != base_class {
                return Err(unsupported("numeric property info hierarchy"));
            }
            let overrides = self.catalog_nested(info_class, c"MoreRoundingOverride")?;
            let default = self.reference(attached, &master, c"defaultProperty", info_class)?;
            if let Some(default) = &default {
                self.exact(default, info_class)?;
            }
            let Some(list) = self.catalog_reference(attached, &master, c"propertyInfoList")? else {
                return Ok(unavailable("PropertyList property info list is absent"));
            };
            let mut definitions = Vec::new();
            for info in self.catalog_list(attached, &list, info_class, 4096)? {
                self.exact(&info, info_class)?;
                let property = self.catalog_enum_u8(&info, c"property", c"SP")?;
                // Keep duplicates and order: native selection returns the first matching property.
                definitions.push((property, info));
            }
            Ok(Availability::Available(PropertyCatalog {
                _master: master,
                definitions,
                default,
                overrides,
                cache: BTreeMap::new(),
            }))
        }
    }

    pub(super) unsafe fn catalog_numeric_property(
        &self,
        attached: &AttachedThread<'_>,
        properties: &mut PropertyCatalog<'_, '_>,
        raw: RawProperty,
    ) -> Result<Availability<NumericProperty>, DiscoveryError> {
        unsafe {
            if !raw.minimum.is_finite() || !raw.maximum.is_finite() || !(-1..=7).contains(&raw.roll_id) {
                return Err(unsupported("numeric property finite range/roll identity"));
            }
            if !ordinary_property(raw.property) {
                return Ok(unavailable(format!(
                    "SP {} requires a separate property-info registry that has not been admitted",
                    raw.property
                )));
            }
            if !(0..=3).contains(&raw.modifier_type) {
                return Ok(unavailable(format!("The game's rounding getter throws for modifier type {}", raw.modifier_type)));
            }
            let selected = properties
                .definitions
                .iter()
                .find(|(property, _)| *property == raw.property)
                .map(|(_, root)| root)
                .or(properties.default.as_ref());
            let Some(info) = selected else {
                return Ok(unavailable("The selected PropertyList entry and default property are both absent"));
            };
            let key = (target(info)?.as_ptr() as usize, raw.modifier_type, raw.special_tag);
            let presentation = if let Some(value) = properties.cache.get(&key) {
                value.clone()
            } else {
                let rounding = match raw.modifier_type {
                    0 => self.enumeration(info, c"roundingForAdded", c"PropertyRounding")?,
                    1 => 0,
                    2 | 3 => {
                        let Some(list) = self.catalog_reference(attached, info, c"moreRoundingOverrides")? else {
                            let reason = "The game's StatPropertyInfo rounding getter requires a nonnull override list";
                            properties.cache.insert(key, unavailable(reason));
                            return Ok(unavailable(reason));
                        };
                        let mut selected = None;
                        for entry in self.catalog_list(attached, &list, properties.overrides, 4096)? {
                            self.exact(&entry, properties.overrides)?;
                            if self.primitive::<u8>(&entry, c"specialTag")? == raw.special_tag {
                                selected = Some(self.enumeration(&entry, c"roundingForMore", c"PropertyRounding")?);
                                break;
                            }
                        }
                        if let Some(value) = selected {
                            value
                        } else {
                            self.enumeration(info, c"roundingForMore", c"PropertyRounding")?
                        }
                    }
                    _ => unreachable!(),
                };
                let value = Availability::Available(Presentation {
                    rounding,
                    percentage: self.boolean(info, c"displayAddedAsPercentage")?,
                    tenth: self.boolean(info, c"displayAddedAsTenthOfValue")?,
                    less: self.boolean(info, c"lessIsBetter")?,
                });
                properties.cache.insert(key, value.clone());
                value
            };
            Ok(match presentation {
                Availability::Unavailable { reason } => Availability::Unavailable { reason },
                Availability::Available(presentation) => Availability::Available(NumericProperty {
                    property: raw.property,
                    tags: raw.tags,
                    special_tag: raw.special_tag,
                    extra_tag: raw.extra_tag,
                    modifier_type: raw.modifier_type,
                    set_property: raw.set_property,
                    minimum: raw.minimum,
                    maximum: raw.maximum,
                    roll_id: raw.roll_id,
                    rounding: presentation.rounding,
                    display_added_as_percentage: presentation.percentage,
                    display_added_as_tenth: presentation.tenth,
                    less_is_better: presentation.less,
                }),
            })
        }
    }

    pub(super) unsafe fn catalog_raw_property(&self, object: &Root<'_, '_>, minimum: f32, maximum: f32, unique: bool) -> Result<RawProperty, DiscoveryError> {
        unsafe {
            let roll_id = if unique && self.boolean(object, c"canRoll")? {
                i32::from(self.primitive::<u8>(object, c"rollID")?)
            } else {
                -1
            };
            let raw = RawProperty {
                property: self.catalog_enum_u8(object, c"property", c"SP")?,
                tags: self.enumeration(object, c"tags", c"AT")?,
                special_tag: self.primitive(object, c"specialTag")?,
                extra_tag: self.primitive(object, c"extraTag")?,
                modifier_type: self.enumeration(object, if unique { c"type" } else { c"modifierType" }, c"ModType")?,
                set_property: if unique { false } else { self.boolean(object, c"setProperty")? },
                minimum,
                maximum,
                roll_id,
            };
            if !raw.minimum.is_finite() || !raw.maximum.is_finite() || !(-1..=7).contains(&raw.roll_id) {
                return Err(unsupported("numeric property finite range/roll identity"));
            }
            Ok(raw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ptr, Type};
    use super::*;
    use crate::il2cpp::{ThreadContext, raw};
    use std::{
        cell::Cell,
        ffi::{CStr, CString, c_char},
        ptr::NonNull,
    };

    const INFO: usize = 0x1000;
    const SECOND: usize = 0x2000;
    const DEFAULT: usize = 0x3000;
    const MASTER: usize = 0x4000;
    const INFO_CLASS: usize = 0x5000;
    const ENUM_CLASS: usize = 0x6000;
    const BOOL_CLASS: usize = 0x7000;
    const INT_CLASS: usize = 0x8000;
    const BASE_TYPE: usize = 0x9000;
    const ADDED: usize = 1;
    const MORE: usize = 2;
    const PERCENT: usize = 3;
    const TENTH: usize = 4;
    const LESS: usize = 5;
    const OVERRIDES: usize = 6;
    const READY: usize = 7;
    thread_local! {
        static ROOTS: Cell<usize> = const { Cell::new(0) };
        static READS: Cell<usize> = const { Cell::new(0) };
        static BASE_CLASS: Cell<usize> = const { Cell::new(INT_CLASS) };
        static IS_ENUM: Cell<bool> = const { Cell::new(true) };
        static ENUM_BASE_CALLS: Cell<usize> = const { Cell::new(0) };
        static PROPERTY_PRESENT: Cell<bool> = const { Cell::new(false) };
        static READINESS: Cell<u8> = const { Cell::new(0) };
    }
    fn token<T>(value: usize) -> *mut T {
        std::ptr::without_provenance_mut(value)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(0x10)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(0x20)
    }
    unsafe extern "C-unwind" fn root_new(object: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        ROOTS.set(ROOTS.get() + 1);
        Box::into_raw(Box::new(object)).cast()
    }
    unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
        // SAFETY: this handle is a live Box allocated by root_new.
        unsafe { handle.cast::<*mut raw::Object>().read() }
    }
    unsafe extern "C-unwind" fn root_free(handle: Ptr) {
        // SAFETY: StrongRoot frees this Box-backed authored handle exactly once.
        unsafe {
            drop(Box::from_raw(handle.cast::<*mut raw::Object>()));
        }
        ROOTS.set(ROOTS.get() - 1);
    }
    unsafe extern "C-unwind" fn object_class(_: *mut raw::Object) -> Class {
        assert_eq!(ROOTS.get(), 4, "selected info must remain rooted with its catalog parents");
        token(INFO_CLASS)
    }
    unsafe extern "C-unwind" fn field(_: Class, name: *const c_char) -> Ptr {
        // SAFETY: caller supplies a static nul-terminated field name.
        let name = unsafe { CStr::from_ptr(name) };
        token(match name {
            n if n == c"roundingForAdded" => ADDED,
            n if n == c"roundingForMore" => MORE,
            n if n == c"displayAddedAsPercentage" => PERCENT,
            n if n == c"displayAddedAsTenthOfValue" => TENTH,
            n if n == c"lessIsBetter" => LESS,
            n if n == c"moreRoundingOverrides" => OVERRIDES,
            n if n == c"instanceFound" => READY,
            _ => panic!("unexpected field"),
        })
    }
    unsafe extern "C-unwind" fn field_type(field: Ptr) -> Type {
        field.cast()
    }
    unsafe extern "C-unwind" fn flags(field: Ptr) -> i32 {
        if field as usize == READY { 0x10 } else { 0 }
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        0
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        false
    }
    unsafe extern "C-unwind" fn type_class(ty: Type) -> Class {
        token(match ty as usize {
            ADDED | MORE => ENUM_CLASS,
            PERCENT | TENTH | LESS | READY => BOOL_CLASS,
            OVERRIDES => 0xa000,
            BASE_TYPE => BASE_CLASS.get(),
            _ => panic!("unexpected type"),
        })
    }
    unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, field: Ptr, destination: Ptr) {
        READS.set(READS.get() + 1);
        // SAFETY: typed field metadata proves the exact destination width before this fake copy.
        unsafe {
            match field as usize {
                ADDED | MORE => destination.cast::<i32>().write(match object as usize {
                    INFO => 101,
                    SECOND => 202,
                    DEFAULT => 303,
                    _ => panic!("unexpected info"),
                }),
                PERCENT | LESS => destination.cast::<u8>().write(1),
                TENTH => destination.cast::<u8>().write(0),
                OVERRIDES => destination.cast::<*mut raw::Object>().write(std::ptr::null_mut()),
                _ => panic!("unexpected field read"),
            }
        }
    }
    unsafe extern "C-unwind" fn static_get(_: Ptr, destination: Ptr) {
        READS.set(READS.get() + 1);
        // SAFETY: exact Boolean static metadata proved a one-byte destination slot.
        unsafe {
            destination.cast::<u8>().write(READINESS.get());
        }
    }
    unsafe extern "C-unwind" fn name(_: Class) -> *const c_char {
        c"PropertyRounding".as_ptr()
    }
    unsafe extern "C-unwind" fn is_enum(_: *const raw::Class) -> bool {
        IS_ENUM.get()
    }
    unsafe extern "C-unwind" fn enum_base(_: Class) -> Type {
        ENUM_BASE_CALLS.set(ENUM_BASE_CALLS.get() + 1);
        token(BASE_TYPE)
    }
    unsafe extern "C-unwind" fn class_value(_: *const raw::Class) -> bool {
        false
    }
    unsafe extern "C-unwind" fn type_name(ty: Type) -> *mut c_char {
        (if ty as usize == READY { c"System.Boolean" } else { c"PropertyRounding" }).to_owned().into_raw()
    }
    unsafe extern "C-unwind" fn free(pointer: Ptr) {
        // SAFETY: pointer is the exact CString returned by the fake type_name.
        unsafe {
            drop(CString::from_raw(pointer.cast()));
        }
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        token(0xb000)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, _: *const c_char, name: *const c_char) -> Class {
        // SAFETY: caller passes a static nul-terminated metadata name.
        match unsafe { CStr::from_ptr(name) } {
            n if n == c"Boolean" => token(BOOL_CLASS),
            n if n == c"Int32" => token(INT_CLASS),
            n if n == c"PropertyList" => {
                if PROPERTY_PRESENT.get() {
                    token(0xc000)
                } else {
                    std::ptr::null_mut()
                }
            }
            _ => panic!("unexpected class lookup"),
        }
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        ROOTS.set(0);
        READS.set(0);
        BASE_CLASS.set(INT_CLASS);
        IS_ENUM.set(true);
        ENUM_BASE_CALLS.set(0);
        PROPERTY_PRESENT.set(false);
        READINESS.set(0);
        let mut access = RuntimeAccess::test_stubs();
        access.object_class = object_class;
        access.class_field = field;
        access.field_type = field_type;
        access.field_flags = flags;
        access.field_offset = offset;
        access.type_byref = byref;
        access.type_class = type_class;
        access.field_get = field_get;
        access.static_get = static_get;
        access.class_name = name;
        access.class_enum = is_enum;
        access.enum_base = enum_base;
        access.class_value = class_value;
        access.type_name = type_name;
        access.runtime_free = free;
        access.corlib = corlib;
        access.class_from_name = class_from_name;
        let api = raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: <raw::ThreadAttach as super::super::TestExportStub>::test_stub(),
            thread_detach: <raw::ThreadDetach as super::super::TestExportStub>::test_stub(),
            gchandle_new: root_new,
            gchandle_get_target: root_target,
            gchandle_free: root_free,
        };
        // SAFETY: authored functions implement this borrowed thread and Box-backed handles.
        (access, unsafe { ThreadContext::new(api) })
    }
    unsafe fn catalog<'a, 'r>(attached: &'a AttachedThread<'r>) -> PropertyCatalog<'a, 'r> {
        // SAFETY: immutable fake objects are recognized and retained by this test's handle API.
        unsafe {
            PropertyCatalog {
                _master: attached.root(NonNull::new(token(MASTER)).unwrap()).unwrap(),
                definitions: vec![
                    (6, attached.root(NonNull::new(token(INFO)).unwrap()).unwrap()),
                    (6, attached.root(NonNull::new(token(SECOND)).unwrap()).unwrap()),
                ],
                default: Some(attached.root(NonNull::new(token(DEFAULT)).unwrap()).unwrap()),
                overrides: token(0xd000),
                cache: BTreeMap::new(),
            }
        }
    }
    fn raw(property: u8, modifier_type: i32) -> RawProperty {
        RawProperty {
            property,
            tags: i32::MIN,
            special_tag: 255,
            extra_tag: i32::MAX,
            modifier_type,
            set_property: true,
            minimum: 20.0,
            maximum: -10.0,
            roll_id: -1,
        }
    }

    #[test]
    fn missing_optional_class_and_not_ready_flag_do_not_query_asset_or_numeric_graph() {
        for (present, ready) in [(false, 0), (true, 0), (true, 2)] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            PROPERTY_PRESENT.set(present);
            READINESS.set(ready);
            let images = Images {
                game: token(0xe000),
                all: vec![token(0xe000)],
            };
            // SAFETY: fake metadata either reports absent capability/readiness or malformed Bool.
            let result = unsafe { access.catalog_properties(&attached, &images) };
            if ready == 2 {
                assert!(result.is_err());
            } else {
                assert!(matches!(result.unwrap(), Availability::Unavailable { .. }));
            }
            assert_eq!(READS.get(), usize::from(present));
            assert_eq!(ROOTS.get(), 0);
        }
    }
    #[test]
    fn first_duplicate_wins_default_is_used_and_cached_flags_do_not_repeat_runtime_reads() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: immutable authored property roots have this test runtime's exact field metadata.
        let mut catalog = unsafe { catalog(&attached) };
        for (property, expected) in [(6, 101), (99, 303)] {
            // SAFETY: fake metadata proves exact enum/Bool slots; ranges are copied without reordering.
            let Availability::Available(value) = unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(property, 0)) }.unwrap() else {
                panic!("expected numeric");
            };
            assert_eq!(value.rounding, expected);
            assert_eq!((value.minimum, value.maximum), (20.0, -10.0));
            assert!(value.display_added_as_percentage && value.less_is_better);
            assert!(!value.display_added_as_tenth);
        }
        assert_eq!(READS.get(), 8);
        // SAFETY: repeated selection uses the owned presentation cache while roots stay alive.
        assert!(matches!(
            unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(6, 0)) }.unwrap(),
            Availability::Available(_)
        ));
        assert_eq!(READS.get(), 8);
        drop(catalog);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn increased_uses_hundredth_without_reading_an_enum_rounding_field() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored rooted immutable property graph and exact Bool fields.
        let mut catalog = unsafe { catalog(&attached) };
        // SAFETY: INCREASED's audited branch returns literal Hundredth0.
        let Availability::Available(value) = unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(6, 1)) }.unwrap() else {
            panic!("expected numeric");
        };
        assert_eq!(value.rounding, 0);
        assert_eq!(ENUM_BASE_CALLS.get(), 0);
        assert_eq!(READS.get(), 3);
    }
    #[test]
    fn unknown_special_and_throwing_modifier_semantics_stay_explicitly_unavailable() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored roots remain live; rejected selector branches must not read metadata.
        let mut catalog = unsafe { catalog(&attached) };
        for property in [43, 58, 98, 123, 130] {
            // SAFETY: known special selector needs another registry and is refused before field access.
            assert!(matches!(
                unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(property, 0)) }.unwrap(),
                Availability::Unavailable { .. }
            ));
        }
        // SAFETY: undefined modifier branch is refused instead of invoking the throwing game body.
        assert!(matches!(
            unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(6, i32::MIN)) }.unwrap(),
            Availability::Unavailable { .. }
        ));
        assert_eq!(READS.get(), 0);
    }
    #[test]
    fn null_more_override_list_is_unavailable_and_never_fabricated_as_empty() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: valid fake reference metadata supplies null for the actual required override list.
        let mut catalog = unsafe { catalog(&attached) };
        for _ in 0..2 {
            // SAFETY: MORE's audited native getter would throw on null list, so numeric remains unknown.
            assert!(matches!(
                unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(6, 2)) }.unwrap(),
                Availability::Unavailable { .. }
            ));
        }
        assert_eq!(READS.get(), 1);
    }
    #[test]
    fn wrong_enum_backing_and_non_enum_never_write_numeric_destination() {
        for non_enum in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            BASE_CLASS.set(BOOL_CLASS);
            IS_ENUM.set(!non_enum);
            // SAFETY: authored immutable roots with deliberately invalid rounding metadata.
            let mut catalog = unsafe { catalog(&attached) };
            // SAFETY: exact destination width must be proven before any public field copy.
            assert!(unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw(6, 0)) }.is_err());
            assert_eq!(READS.get(), 0);
            assert_eq!(ENUM_BASE_CALLS.get(), usize::from(!non_enum));
        }
    }
    #[test]
    fn nonfinite_numbers_and_unrepresentable_rolls_refuse_before_metadata_access() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored roots are valid, but scalar values intentionally violate the owned contract.
        let mut catalog = unsafe { catalog(&attached) };
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut raw = raw(6, 0);
            raw.minimum = value;
            // SAFETY: invalid finite range must fail before reading property metadata.
            assert!(unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw) }.is_err());
        }
        let mut raw = raw(6, 0);
        raw.roll_id = 8;
        // SAFETY: unique roll bytes outside the eight supported slots have no admitted meaning.
        assert!(unsafe { access.catalog_numeric_property(&attached, &mut catalog, raw) }.is_err());
        assert_eq!(READS.get(), 0);
    }
    #[test]
    fn numeric_copy_budget_reserves_names_and_rejects_aggregate_or_integer_overflow() {
        let mut budget = NumericBudget::new();
        assert!(budget.used > 16384);
        budget.charge(crate::protocol::MAX_RESPONSE_SIZE - budget.used).unwrap();
        assert!(budget.charge(1).is_err());
        assert!(NumericBudget::new().charge(usize::MAX).is_err());
    }
}
