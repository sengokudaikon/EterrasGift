//! Staged numeric text only: no description/localization getter, item construction or scalar parse.
mod modifier;
pub use modifier::NumericModifierDisplay;

use super::{Class, DiscoveryError, Ptr, RuntimeAccess, forge::Root};
use crate::il2cpp::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, StrongRoot, ValueTypeContract, raw};
use std::{ffi::CStr, ptr::NonNull};

const MAX_METHODS: usize = 4096;
const MAX_OVERRIDES: u32 = 4096;
const MAX_UNITS: usize = 4096;
const MAX_BYTES: usize = 16 * 1024;
#[derive(Debug, PartialEq, Eq)]
pub struct NumericDisplay {
    pub display: String,
    pub unit: String,
}
#[derive(Debug)]
pub enum NumericDisplayUnavailable {
    Input,
    Context,
    Contract(&'static str),
    Metadata(DiscoveryError),
    Invocation,
    Exception,
    Text,
}
impl std::fmt::Display for NumericDisplayUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Numeric display unavailable: {self:?}")
    }
}
impl std::error::Error for NumericDisplayUnavailable {}
impl From<DiscoveryError> for NumericDisplayUnavailable {
    fn from(error: DiscoveryError) -> Self {
        Self::Metadata(error)
    }
}
type Unavailable = NumericDisplayUnavailable;
fn input(value: f32, modifier: i32) -> Result<(), Unavailable> {
    if value.is_finite() && (0..=3).contains(&modifier) {
        Ok(())
    } else {
        Err(Unavailable::Input)
    }
}
fn decode(units: &[u16]) -> Result<NumericDisplay, Unavailable> {
    if units.len() > MAX_UNITS {
        return Err(Unavailable::Text);
    }
    let display = String::from_utf16(units).map_err(|_| Unavailable::Text)?;
    if display.len() > MAX_BYTES || display.contains('\0') {
        return Err(Unavailable::Text);
    }
    // Original propertyEffect derives only the actual final ASCII percent, without inventing one.
    let unit = if display.ends_with('%') { "%" } else { "" }.to_owned();
    Ok(NumericDisplay { display, unit })
}
impl RuntimeAccess {
    // All helpers inherit format_numeric_display's live immutable metadata and graph contract.
    unsafe fn formatter_hierarchy(&self, stat: Class, localized: Class, base: Class, entry: Class) -> Result<(), Unavailable> {
        unsafe {
            if [stat, localized, base, entry].iter().any(|class| class.is_null())
                || (self.class_parent)(stat) != localized
                || (self.class_parent)(localized) != base
                || [stat, localized, base, entry].iter().any(|class| (self.class_value)(*class))
            {
                return Err(Unavailable::Contract("exact reference property hierarchy"));
            }
            Ok(())
        }
    }
    unsafe fn formatter_core(&self, name: &CStr, code: i32) -> Result<Class, Unavailable> {
        unsafe {
            let image = (self.corlib)();
            if image.is_null() {
                return Err(Unavailable::Contract("missing corlib"));
            }
            let class = self.system_class(name)?;
            let ty = (self.class_type)(class);
            if (self.class_image)(class) != image || ty.is_null() || (self.type_byref)(ty) || (self.type_code)(ty) != code {
                return Err(Unavailable::Contract("exact core class/code"));
            }
            Ok(class)
        }
    }
    unsafe fn formatter_enum(&self, class: Class, i32_class: Class) -> Result<(), Unavailable> {
        unsafe {
            if class.is_null() || !(self.class_enum)(class) || !(self.class_value)(class) {
                return Err(Unavailable::Contract("exact Int32 enum class"));
            }
            let ty = (self.enum_base)(class);
            if ty.is_null() || (self.type_byref)(ty) || (self.type_code)(ty) != 0x08 || (self.type_class)(ty) != i32_class {
                return Err(Unavailable::Contract("exact Int32 enum backing"));
            }
            Ok(())
        }
    }
    unsafe fn formatter_field(&self, owner: Class, name: &CStr, expected: Class, code: i32) -> Result<Ptr, Unavailable> {
        unsafe {
            let field = self.field(owner, name)?;
            if (self.field_parent)(field) != owner || (self.field_flags)(field) & (0x10 | 0x40) != 0 {
                return Err(Unavailable::Contract("field declaring owner/instance storage"));
            }
            let ty = (self.field_type)(field);
            if ty.is_null() || (self.type_byref)(ty) || (self.type_class)(ty) != expected || (self.type_code)(ty) != code {
                return Err(Unavailable::Contract("field exact identity/width"));
            }
            Ok(field)
        }
    }
    unsafe fn formatter_rounding(&self, info: &Root<'_, '_>, owner: Class, name: &CStr, rounding: Class) -> Result<(), Unavailable> {
        unsafe {
            let field = self.formatter_field(owner, name, rounding, 0x11)?;
            let object = info.target().ok_or(Unavailable::Context)?;
            let mut value = 0_i32;
            // Exact nonbyref enum identity and verified Int32 backing precede four-byte copy.
            (self.field_get)(object.as_ptr(), field, (&mut value as *mut i32).cast());
            if !(0..=3).contains(&value) {
                return Err(Unavailable::Contract("unknown PropertyRounding value"));
            }
            Ok(())
        }
    }
    unsafe fn formatter_boolean(&self, info: &Root<'_, '_>, owner: Class, name: &CStr, boolean: Class) -> Result<(), Unavailable> {
        unsafe {
            let field = self.formatter_field(owner, name, boolean, 0x02)?;
            let mut value = 0_u8;
            (self.field_get)(info.target().ok_or(Unavailable::Context)?.as_ptr(), field, (&mut value as *mut u8).cast());
            if value > 1 {
                return Err(Unavailable::Contract("Boolean representation"));
            }
            Ok(())
        }
    }
    unsafe fn formatter_method(&self, api: raw::InvocationApi, owner: Class, string: Class, parameters: &[Class; 5]) -> Result<NonNull<raw::MethodInfo>, Unavailable> {
        unsafe {
            let mut iterator = std::ptr::null_mut();
            let mut found = None;
            for _ in 0..MAX_METHODS {
                let Some(method) = NonNull::new((self.class_methods)(owner, &mut iterator).cast_mut()) else {
                    return found.ok_or(Unavailable::Contract("missing exact five-argument formatter"));
                };
                let m = method.as_ptr();
                let name = (self.method_name)(m);
                if name.is_null() {
                    return Err(Unavailable::Contract("null method name"));
                }
                if CStr::from_ptr(name) != c"DisplayStringForValue" || (api.method_get_param_count)(m) != 5 {
                    continue;
                }
                let flags = (api.method_get_flags)(m, std::ptr::null_mut());
                if (api.method_get_class)(m) != owner
                    || (api.method_is_instance)(m)
                    || flags & 0x10 == 0
                    || flags & 0x400 != 0
                    || (api.method_is_generic)(m)
                    || (api.method_is_inflated)(m)
                {
                    continue;
                }
                let result = (api.method_get_return_type)(m);
                if result.is_null() || (api.type_is_byref)(result) || (api.class_from_type)(result) != string {
                    continue;
                }
                let mut matching = true;
                for (index, expected) in parameters.iter().enumerate() {
                    let ty = (api.method_get_param)(m, index as u32);
                    if ty.is_null() || (api.type_is_byref)(ty) || (api.class_from_type)(ty) != *expected {
                        matching = false;
                        break;
                    }
                }
                if matching && found.replace(method).is_some() {
                    return Err(Unavailable::Contract("ambiguous exact formatter"));
                }
            }
            Err(Unavailable::Contract("formatter method enumeration limit"))
        }
    }
    unsafe fn formatter_string(&self, attached: &AttachedThread<'_>, root: &StrongRoot<'_, '_>, string: Class) -> Result<NumericDisplay, Unavailable> {
        if !root.belongs_to(attached) {
            return Err(Unavailable::Context);
        }
        unsafe {
            let object = root.target().ok_or(Unavailable::Context)?;
            if (self.object_class)(object.as_ptr()) != string {
                return Err(Unavailable::Contract("exact returned String class"));
            }
            let length = (self.string_length)(object.as_ptr());
            if !(0..=MAX_UNITS as i32).contains(&length) {
                return Err(Unavailable::Text);
            }
            let chars = (self.string_chars)(object.as_ptr());
            if chars.is_null() && length != 0 {
                return Err(Unavailable::Text);
            }
            let mut units = Vec::with_capacity(length as usize);
            for index in 0..length as usize {
                units.push(chars.add(index).read());
            }
            decode(&units)
        }
    }
    /// Numeric text only; the fixed false suffix matches propertyEffect's ConvertValueToString.
    /// No descriptor/localization getter, item constructor/clone, culture mutation or parser is used.
    ///
    /// # Safety
    /// Caller binds all tables/metadata/attachment to the same retained source-audited Unity6000.4
    /// artifact with initialized runtime/nonmoving GC and valid modern array headers. Execute on
    /// its real main thread, excluding shutdown, reentrancy and concurrent graph/metadata mutation.
    /// The containing operation independently admits offline target/generation and fresh selected
    /// ready-registry identity; this internal text adapter does not establish that gameplay policy.
    /// The rooted input and override list graph must remain unchanged through invocation/copy.
    /// Authorize the exact native formatter/arithmetic/BCL closure, including the exact runtime
    /// StatPropertyInfo virtual GetRoundingForMore implementation. The active culture/provider's
    /// virtual GetFormat/NumberFormat closure must independently be authorized (derived CultureInfo
    /// cannot be silently admitted). Exclude culture/provider/settings mutation by other threads,
    /// managed callbacks and profilers throughout this operation. No Name/equality/provider-capture
    /// assertion is made here, and returned text conveys no parsed-mapping capability.
    /// Explicitly permit framework/game utility class initializers, static cache/array/constant
    /// writes and barriers, globalization, locks, allocations and GC/profiler callbacks; these are
    /// not zero-effect calls. Native field getters must synchronously copy only declared widths.
    /// Exceptions are rooted data; arbitrary native faults/SEH and rollback are not recovered.
    pub unsafe fn format_numeric_display(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        property_info: &StrongRoot<'_, '_>,
        raw_value: f32,
        modifier_type: i32,
        special_tag: u8,
    ) -> Result<NumericDisplay, Unavailable> {
        input(raw_value, modifier_type)?;
        if !property_info.belongs_to(attached) {
            return Err(Unavailable::Context);
        }
        unsafe {
            let images = self.images()?;
            let stat = self.class(&images, c"", c"StatPropertyInfo")?;
            let base = self.class(&images, c"", c"BasePropertyInfo")?;
            let localized = self.class(&images, c"", c"PropertyInfoWithLocKey")?;
            let formatter = self.class(&images, c"", c"TooltipItemManager")?;
            let stats = self.class(&images, c"", c"BaseStats")?;
            let rounding = self.class(&images, c"", c"PropertyRounding")?;
            let modifier = self.catalog_nested(stats, c"ModType")?;
            let entry = self.catalog_nested(stat, c"MoreRoundingOverride")?;
            for class in [stat, localized, base, formatter, stats, rounding, modifier, entry] {
                if (self.class_image)(class) != images.game {
                    return Err(Unavailable::Contract("exact game image class"));
                }
            }
            self.formatter_hierarchy(stat, localized, base, entry)?;
            self.exact(property_info, stat)?;
            let single = self.formatter_core(c"Single", 0x0c)?;
            let byte = self.formatter_core(c"Byte", 0x05)?;
            let boolean = self.formatter_core(c"Boolean", 0x02)?;
            let i32_class = self.formatter_core(c"Int32", 0x08)?;
            let string = self.formatter_core(c"String", 0x0e)?;
            self.formatter_enum(rounding, i32_class)?;
            self.formatter_enum(modifier, i32_class)?;
            self.formatter_rounding(property_info, base, c"roundingForAdded", rounding)?;
            self.formatter_rounding(property_info, base, c"roundingForMore", rounding)?;
            for name in [c"displayAddedAsPercentage", c"displayAddedAsTenthOfValue"] {
                self.formatter_boolean(property_info, base, name, boolean)?;
            }
            let list_field = self.field(stat, c"moreRoundingOverrides")?;
            let list_class = self.field_class(list_field)?;
            self.formatter_field(stat, c"moreRoundingOverrides", list_class, 0x15)?;
            let list = self
                .reference(attached, property_info, c"moreRoundingOverrides", list_class)?
                .ok_or(Unavailable::Contract("missing rounding override list"))?;
            self.exact(&list, list_class)?;
            // Existing catalog_list validates exact closed corlib List/SZARRAY element and bounds.
            // Validate declared storage owners too, before any field getter reaches a stack slot.
            self.formatter_field(list_class, c"_size", i32_class, 0x08)?;
            let array_field = self.field(list_class, c"_items")?;
            let array = self.field_class(array_field)?;
            self.formatter_field(list_class, c"_items", array, 0x1d)?;
            self.array_class(array, entry)?;
            let entries = self.catalog_list(attached, &list, entry, MAX_OVERRIDES)?;
            for root in &entries {
                self.exact(root, entry)?;
                let field = self.formatter_field(entry, c"specialTag", byte, 0x05)?;
                let mut tag = 0_u8;
                (self.field_get)(root.target().ok_or(Unavailable::Context)?.as_ptr(), field, (&mut tag as *mut u8).cast());
                self.formatter_rounding(root, entry, c"roundingForMore", rounding)?;
            }
            let method = self.formatter_method(api, formatter, string, &[single, base, modifier, byte, boolean])?;
            let calls = InvocationContext::new(attached, api);
            let method = calls
                .bind_with_value_types(
                    method,
                    &[ValueTypeContract::I32Enum {
                        class: NonNull::new(modifier).ok_or(Unavailable::Contract("null ModType"))?,
                    }],
                )
                .map_err(|_| Unavailable::Invocation)?;
            // property_info, list and every override root remain live through call/result copy.
            let result = method
                .invoke(
                    None,
                    &mut [
                        Argument::Scalar(Scalar::F32(raw_value)),
                        Argument::Reference(Some(property_info)),
                        Argument::EnumI32 {
                            class: NonNull::new(modifier).ok_or(Unavailable::Contract("null ModType"))?,
                            value: modifier_type,
                        },
                        Argument::Scalar(Scalar::U8(special_tag)),
                        Argument::Scalar(Scalar::Boolean(false)),
                    ],
                )
                .map_err(|_| Unavailable::Invocation)?;
            let output = match result {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => self.formatter_string(attached, &root, string),
                InvocationOutcome::Returned(_) => Err(Unavailable::Contract("nonnull formatter String return")),
                InvocationOutcome::Exception(_root) => Err(Unavailable::Exception),
            };
            drop(entries);
            drop(list);
            output
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::TestExportStub;
    use super::*;
    use crate::il2cpp::ThreadContext;
    use std::{
        cell::{Cell, RefCell},
        ffi::{c_char, c_void},
    };
    const OWNER: usize = 0x1000;
    const EXPECTED: usize = 0x2000;
    const FIELD: usize = 0x3000;
    const TY: usize = 0x4000;
    const METHOD: usize = 0x5000;
    const METHOD2: usize = 0x5010;
    const STRING: usize = 0x6000;
    thread_local! {
        static MODE:Cell<u8>=const{Cell::new(0)};
        static READS:Cell<usize>=const{Cell::new(0)};
        static ROOTS:Cell<usize>=const{Cell::new(0)};
        static LENGTHS:Cell<usize>=const{Cell::new(0)};
        static ENUM_BASES:Cell<usize>=const{Cell::new(0)};
        static FAIL_ROOT:Cell<bool>=const{Cell::new(false)};
        static TEXT:RefCell<Vec<u16>>=const{RefCell::new(Vec::new())};
    }
    fn ptr<T>(v: usize) -> *mut T {
        std::ptr::without_provenance_mut(v)
    }
    fn nn<T>(v: usize) -> NonNull<T> {
        NonNull::new(ptr(v)).unwrap()
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        ptr(1)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        ptr(2)
    }
    unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
        panic!("already attached")
    }
    unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
        panic!("borrowed attachment")
    }
    unsafe extern "C-unwind" fn root(o: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        if FAIL_ROOT.get() {
            return std::ptr::null_mut();
        }
        ROOTS.set(ROOTS.get() + 1);
        o.cast()
    }
    unsafe extern "C-unwind" fn target(h: Ptr) -> *mut raw::Object {
        h.cast()
    }
    unsafe extern "C-unwind" fn free(_: Ptr) {
        ROOTS.set(ROOTS.get() - 1);
    }
    fn context() -> ThreadContext {
        // SAFETY: immutable authored opaque identities and pointer-valued roots on this test thread.
        unsafe {
            ThreadContext::new(raw::Api {
                domain_get: domain,
                thread_current: current,
                thread_attach: attach,
                thread_detach: detach,
                gchandle_new: root,
                gchandle_free: free,
                gchandle_get_target: target,
            })
        }
    }
    fn fixture() -> RuntimeAccess {
        assert_eq!(ROOTS.get(), 0);
        MODE.set(0);
        READS.set(0);
        LENGTHS.set(0);
        ENUM_BASES.set(0);
        FAIL_ROOT.set(false);
        TEXT.with_borrow_mut(|t| *t = "12,5🌿".encode_utf16().collect());
        let mut a = RuntimeAccess::test_stubs();
        a.class_field = field;
        a.field_parent = parent;
        a.field_flags = flags;
        a.field_type = field_type;
        a.type_byref = byref;
        a.type_class = class_type;
        a.type_code = code;
        a.field_get = get;
        a.class_enum = is_enum;
        a.class_value = is_value;
        a.enum_base = enum_base;
        a.class_methods = methods;
        a.method_name = method_name;
        a.object_class = object_class;
        a.string_length = length;
        a.string_chars = chars;
        a
    }
    unsafe extern "C-unwind" fn field(_: Class, _: *const c_char) -> Ptr {
        ptr(FIELD)
    }
    unsafe extern "C-unwind" fn parent(_: Ptr) -> Class {
        ptr(if MODE.get() == 1 { OWNER + 1 } else { OWNER })
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        match MODE.get() {
            2 => 0x10,
            3 => 0x40,
            _ => 0,
        }
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> super::super::Type {
        if MODE.get() == 4 { std::ptr::null() } else { ptr(TY) }
    }
    unsafe extern "C-unwind" fn byref(_: *const raw::Type) -> bool {
        MODE.get() == 5
    }
    unsafe extern "C-unwind" fn class_type(t: *const raw::Type) -> Class {
        if t as usize == TY + 1 {
            ptr(STRING)
        } else if t as usize >= TY + 10 {
            ptr(EXPECTED + (t as usize - TY - 10) * 0x10)
        } else {
            ptr(if MODE.get() == 6 { EXPECTED + 1 } else { EXPECTED })
        }
    }
    unsafe extern "C-unwind" fn code(_: *const raw::Type) -> i32 {
        match MODE.get() {
            7 => 0x0a,
            20 => 0x02,
            30..=34 => 0x08,
            _ => 0x11,
        }
    }
    unsafe extern "C-unwind" fn get(_: *mut raw::Object, _: Ptr, out: Ptr) {
        assert!(ROOTS.get() > 0);
        READS.set(READS.get() + 1);
        // SAFETY: the tested adapter has selected the exact authored field type before this copy.
        unsafe {
            if MODE.get() == 20 {
                out.cast::<u8>().write(2)
            } else {
                out.cast::<i32>().write(if MODE.get() == 8 { 4 } else { 3 })
            }
        }
    }
    unsafe extern "C-unwind" fn is_enum(_: *const raw::Class) -> bool {
        MODE.get() != 31
    }
    unsafe extern "C-unwind" fn is_value(_: *const raw::Class) -> bool {
        MODE.get() != 32
    }
    unsafe extern "C-unwind" fn enum_base(_: Class) -> *const raw::Type {
        ENUM_BASES.set(ENUM_BASES.get() + 1);
        if MODE.get() == 33 { std::ptr::null() } else { ptr(TY) }
    }
    unsafe extern "C-unwind" fn object_class(_: *mut raw::Object) -> Class {
        assert!(ROOTS.get() > 0, "root must precede metadata");
        ptr(if MODE.get() == 40 { STRING + 1 } else { STRING })
    }
    unsafe extern "C-unwind" fn length(_: *mut raw::Object) -> i32 {
        LENGTHS.set(LENGTHS.get() + 1);
        match MODE.get() {
            41 => -1,
            42 => 4097,
            _ => TEXT.with_borrow(|t| t.len() as i32),
        }
    }
    unsafe extern "C-unwind" fn chars(_: *mut raw::Object) -> *mut u16 {
        if MODE.get() == 43 {
            std::ptr::null_mut()
        } else {
            TEXT.with_borrow_mut(|t| t.as_mut_ptr())
        }
    }
    unsafe extern "C-unwind" fn methods(_: Class, iter: *mut Ptr) -> *const raw::MethodInfo {
        // SAFETY: bounded iterator storage supplied by formatter_method.
        let index = unsafe { *iter } as usize;
        // SAFETY: caller owns this writable metadata iterator slot; the stored next index is only a sentinel.
        unsafe { *iter = ptr(index + 1) };
        if index == 0 {
            ptr(METHOD)
        } else if index == 1 && MODE.get() == 50 {
            ptr(METHOD2)
        } else {
            std::ptr::null()
        }
    }
    unsafe extern "C-unwind" fn method_name(_: *const raw::MethodInfo) -> *const c_char {
        c"DisplayStringForValue".as_ptr()
    }
    unsafe extern "C-unwind" fn method_owner(_: *const raw::MethodInfo) -> Class {
        ptr(if MODE.get() == 51 { OWNER + 1 } else { OWNER })
    }
    unsafe extern "C-unwind" fn generic(_: *const raw::MethodInfo) -> bool {
        MODE.get() == 52
    }
    unsafe extern "C-unwind" fn inflated(_: *const raw::MethodInfo) -> bool {
        MODE.get() == 53
    }
    unsafe extern "C-unwind" fn instance(_: *const raw::MethodInfo) -> bool {
        MODE.get() == 54
    }
    unsafe extern "C-unwind" fn count(_: *const raw::MethodInfo) -> u32 {
        if MODE.get() == 55 { 4 } else { 5 }
    }
    unsafe extern "C-unwind" fn method_flags(_: *const raw::MethodInfo, _: *mut u32) -> u32 {
        match MODE.get() {
            56 => 0,
            57 => 0x410,
            _ => 0x10,
        }
    }
    unsafe extern "C-unwind" fn result(_: *const raw::MethodInfo) -> *const raw::Type {
        if MODE.get() == 58 { ptr(TY) } else { ptr(TY + 1) }
    }
    unsafe extern "C-unwind" fn parameter(_: *const raw::MethodInfo, index: u32) -> *const raw::Type {
        ptr(TY + 10 + index as usize + usize::from(MODE.get() == 59))
    }
    unsafe extern "C-unwind" fn unexpected_invoke(_: *const raw::MethodInfo, _: *mut c_void, _: *mut *mut c_void, _: *mut *mut raw::Exception) -> *mut raw::Object {
        panic!("unexpected invocation")
    }
    fn api() -> raw::InvocationApi {
        macro_rules! stub {
            ($t:ty) => {
                <$t as TestExportStub>::test_stub()
            };
        }
        raw::InvocationApi {
            runtime_invoke: unexpected_invoke,
            method_get_return_type: result,
            method_get_class: method_owner,
            method_is_generic: generic,
            method_is_inflated: inflated,
            method_is_instance: instance,
            method_get_param_count: count,
            method_get_param: parameter,
            method_get_flags: method_flags,
            type_get_type: stub!(raw::TypeGetType),
            type_is_byref: byref,
            class_from_type: class_type,
            class_is_valuetype: stub!(raw::ClassIsValueType),
            class_is_generic: stub!(raw::ClassIsGeneric),
            class_is_inflated: stub!(raw::ClassIsInflated),
            class_is_assignable_from: stub!(raw::ClassIsAssignableFrom),
            object_get_class: stub!(raw::ObjectGetClass),
            object_unbox: stub!(raw::ObjectUnbox),
            object_get_virtual_method: stub!(raw::ObjectGetVirtualMethod),
            get_corlib: stub!(raw::GetCorlib),
            class_from_name: stub!(raw::ClassFromName),
            class_is_enum: stub!(raw::ClassIsEnum),
            class_enum_basetype: stub!(raw::ClassEnumBasetype),
            class_value_size: stub!(raw::ClassValueSize),
        }
    }
    unsafe extern "C-unwind" fn hierarchy_parent(class: Class) -> Class {
        match class as usize {
            OWNER => ptr(if MODE.get() == 70 { OWNER + 0x20 } else { OWNER + 0x10 }),
            n if n == OWNER + 0x10 => ptr(if MODE.get() == 71 { OWNER + 0x30 } else { OWNER + 0x20 }),
            _ => panic!("only exact two reference parent links may be read"),
        }
    }
    unsafe extern "C-unwind" fn hierarchy_value(class: *const raw::Class) -> bool {
        MODE.get() == 72 && class as usize == OWNER + 0x10
    }
    #[test]
    fn exact_actual_intermediate_property_class_is_required_not_flattened() {
        let mut a = fixture();
        a.class_parent = hierarchy_parent;
        a.class_value = hierarchy_value;
        // SAFETY: fake metadata mirrors authoritative Stat -> WithLocKey -> Base chain.
        assert!(unsafe { a.formatter_hierarchy(ptr(OWNER), ptr(OWNER + 0x10), ptr(OWNER + 0x20), ptr(OWNER + 0x30)) }.is_ok());
        for mode in [70, 71, 72] {
            MODE.set(mode);
            // SAFETY: direct Base parent, wrong intermediate parent or value-type intermediary refuses.
            assert!(unsafe { a.formatter_hierarchy(ptr(OWNER), ptr(OWNER + 0x10), ptr(OWNER + 0x20), ptr(OWNER + 0x30)) }.is_err());
        }
        // SAFETY: null metadata must refuse before class-parent exports.
        assert!(unsafe { a.formatter_hierarchy(std::ptr::null_mut(), ptr(OWNER + 0x10), ptr(OWNER + 0x20), ptr(OWNER + 0x30)) }.is_err());
    }
    #[test]
    fn bad_numeric_input_and_foreign_context_refuse_before_metadata_or_invocation() {
        let a = fixture();
        let ctx = context();
        let attached = ctx.attach().unwrap();
        // SAFETY: opaque authored object remains valid in its fake root until drop.
        let r = unsafe { attached.root(nn(99)) }.unwrap();
        for (v, m) in [(f32::NAN, 0), (f32::INFINITY, 1), (f32::NEG_INFINITY, 2), (1.0, -1), (1.0, 4)] {
            // SAFETY: invalid scalars must refuse before all unprovided game exports.
            assert!(matches!(unsafe { a.format_numeric_display(&attached, api(), &r, v, m, 2) }, Err(Unavailable::Input)));
        }
        let other = context();
        let other_attached = other.attach().unwrap();
        // SAFETY: different context must refuse before opaque metadata is accessed.
        assert!(matches!(
            // SAFETY: different context must refuse before opaque metadata is accessed.
            unsafe { a.format_numeric_display(&other_attached, api(), &r, 1.0, 0, 2) },
            Err(Unavailable::Context)
        ));
        assert_eq!(READS.get(), 0);
    }
    #[test]
    fn wrong_owner_flags_type_byref_or_width_never_reach_scalar_destination() {
        let a = fixture();
        let ctx = context();
        let attached = ctx.attach().unwrap();
        // SAFETY: fake immutable rooted object and exact authored export table.
        let r = unsafe { attached.root(nn(99)) }.unwrap();
        for mode in 1..=7 {
            MODE.set(mode);
            // SAFETY: fake metadata violates one condition; no typed copy is permitted.
            assert!(unsafe { a.formatter_rounding(&r, ptr(OWNER), c"roundingForAdded", ptr(EXPECTED)) }.is_err());
            assert_eq!(READS.get(), 0);
        }
        MODE.set(0);
        // SAFETY: canonical enum-backed destination now permits exactly one Int32 copy.
        assert!(unsafe { a.formatter_rounding(&r, ptr(OWNER), c"roundingForAdded", ptr(EXPECTED)) }.is_ok());
        MODE.set(8);
        // SAFETY: unknown value is rejected after a correctly sized scalar copy.
        assert!(unsafe { a.formatter_rounding(&r, ptr(OWNER), c"roundingForAdded", ptr(EXPECTED)) }.is_err());
        assert_eq!(READS.get(), 2);
    }
    #[test]
    fn enum_kind_precedes_base_query_and_underlying_storage_is_exact() {
        let a = fixture();
        for mode in [31, 32] {
            MODE.set(mode);
            // SAFETY: enum-base export must not run on non-enum/nonvalue metadata.
            assert!(unsafe { a.formatter_enum(ptr(OWNER), ptr(EXPECTED)) }.is_err());
        }
        assert_eq!(ENUM_BASES.get(), 0);
        MODE.set(30);
        // SAFETY: exact authored core Int32 identity and width.
        assert!(unsafe { a.formatter_enum(ptr(OWNER), ptr(EXPECTED)) }.is_ok());
        MODE.set(33);
        // SAFETY: null underlying metadata is a typed refusal.
        assert!(unsafe { a.formatter_enum(ptr(OWNER), ptr(EXPECTED)) }.is_err());
        MODE.set(34);
        // SAFETY: class identity mismatch cannot be accepted by name/width alone.
        assert!(unsafe { a.formatter_enum(ptr(OWNER), ptr(EXPECTED + 1)) }.is_err());
        assert_eq!(ENUM_BASES.get(), 3);
    }
    #[test]
    fn noncanonical_boolean_uses_byte_storage_and_is_rejected() {
        let a = fixture();
        let ctx = context();
        let attached = ctx.attach().unwrap();
        // SAFETY: rooted fake object with exact Boolean field metadata.
        let r = unsafe { attached.root(nn(99)) }.unwrap();
        MODE.set(20);
        assert!(matches!(
            // SAFETY: fake field metadata specifies one byte; noncanonical Boolean data is rejected before use.
            unsafe { a.formatter_boolean(&r, ptr(OWNER), c"flag", ptr(EXPECTED)) },
            Err(Unavailable::Contract("Boolean representation"))
        ));
        assert_eq!(READS.get(), 1);
    }
    #[test]
    fn only_unique_exact_static_five_arg_method_is_selected() {
        let a = fixture();
        let parameters = std::array::from_fn(|i| ptr(EXPECTED + i * 0x10));
        // SAFETY: exact immutable authored signature.
        assert_eq!(unsafe { a.formatter_method(api(), ptr(OWNER), ptr(STRING), &parameters) }.unwrap(), nn(METHOD));
        for mode in 50..=59 {
            MODE.set(mode);
            // SAFETY: one incompatible/ambiguous fact in authored metadata.
            assert!(unsafe { a.formatter_method(api(), ptr(OWNER), ptr(STRING), &parameters) }.is_err());
        }
    }
    #[test]
    fn string_result_requires_same_context_exact_class_and_bound_before_characters() {
        let a = fixture();
        let ctx = context();
        let attached = ctx.attach().unwrap();
        // SAFETY: fake root keeps immutable authored UTF16 storage live.
        let r = unsafe { attached.root(nn(99)) }.unwrap();
        // SAFETY: fake roots retain immutable authored UTF-16 storage; wrong context/class/bounds must refuse before character access.
        assert_eq!(unsafe { a.formatter_string(&attached, &r, ptr(STRING)) }.unwrap().display, "12,5🌿");
        MODE.set(40);
        // SAFETY: fake roots retain immutable authored UTF-16 storage; wrong context/class/bounds must refuse before character access.
        assert!(unsafe { a.formatter_string(&attached, &r, ptr(STRING)) }.is_err());
        assert_eq!(LENGTHS.get(), 1, "wrong class refuses before length/chars");
        for mode in [41, 42, 43] {
            MODE.set(mode);
            // SAFETY: fake roots retain immutable authored UTF-16 storage; wrong context/class/bounds must refuse before character access.
            assert!(unsafe { a.formatter_string(&attached, &r, ptr(STRING)) }.is_err());
        }
        let other = context();
        let foreign = other.attach().unwrap();
        let lengths = LENGTHS.get();
        // SAFETY: fake roots retain immutable authored UTF-16 storage; wrong context/class/bounds must refuse before character access.
        assert!(matches!(unsafe { a.formatter_string(&foreign, &r, ptr(STRING)) }, Err(Unavailable::Context)));
        assert_eq!(LENGTHS.get(), lengths);
        FAIL_ROOT.set(true);
        // SAFETY: controlled handle failure preserves previously acquired result root.
        assert!(unsafe { attached.root(nn(100)) }.is_err());
        assert_eq!(ROOTS.get(), 1);
    }
    #[test]
    fn decoder_is_strict_bounded_owned_and_derives_only_actual_final_unit() {
        assert_eq!(
            decode(&" Ω🌿12,5%".encode_utf16().collect::<Vec<_>>()).unwrap(),
            NumericDisplay {
                display: " Ω🌿12,5%".into(),
                unit: "%".into()
            }
        );
        assert_eq!(decode(&"12% ".encode_utf16().collect::<Vec<_>>()).unwrap().unit, "");
        assert_eq!(decode(&[]).unwrap().display, "");
        for text in [vec![0xd800], vec![0], vec![b'1' as u16; 4097]] {
            assert!(decode(&text).is_err());
        }
        assert_eq!(decode(&vec![b'1' as u16; 4096]).unwrap().display.len(), 4096);
        assert_eq!(decode(&"🌿".repeat(2048).encode_utf16().collect::<Vec<_>>()).unwrap().display.len(), 8192);
    }
}
