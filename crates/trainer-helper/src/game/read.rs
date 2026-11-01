//! Reading game objects: typed copies of fields of rooted objects, each after its type, owner and storage are checked.
//! Nothing here calls a game method or writes. `collections` reads lists, arrays, statics and nested definitions.

mod collections;

use super::{Class, GameError, Ptr, Root, RuntimeAccess, copy_values, metadata_text, target, unsupported};
use eg_trainer_wire::fields::ObservedObjectIdentity;
use il2cpp_runtime::{AttachedThread, raw};
use std::ffi::CStr;

pub(crate) fn decode_text(units: &[u16], maximum: usize) -> Result<String, GameError> {
    if units.len() > maximum {
        return Err(unsupported("text length"));
    }
    let text = String::from_utf16(units).map_err(|_| unsupported("text UTF-16"))?;
    if text.len() > maximum {
        return Err(unsupported("text UTF-8 length"));
    }
    Ok(text)
}

impl RuntimeAccess {
    pub(crate) unsafe fn enumeration(&self, object: &Root<'_, '_>, name: &CStr, enum_name: &CStr) -> Result<i32, GameError> {
        // SAFETY: The rooted receiver's enum name and underlying Int32 metadata are checked before copying into an i32 local.
        unsafe {
            let object_class = (self.object_class)(target(object)?.as_ptr());
            if object_class.is_null() {
                return Err(unsupported("enum object class"));
            }
            let field = self.field(object_class, name)?;
            let class = self.field_class(field, "enum")?;
            let actual_name = (self.class_name)(class);
            // The field itself identifies its enum, including a nested or namespaced enum that
            // cannot be resolved by class_from_name(image, "", simpleName). Recheck both the
            // accepted semantic name and the enum storage before copying into an i32 slot.
            if (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0 || actual_name.is_null() || metadata_text(actual_name) != enum_name {
                return Err(unsupported("enum field name/storage"));
            }
            self.enum_storage(class, c"Int32", 0x08)?;
            let mut value = 0_i32;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut i32).cast());
            Ok(value)
        }
    }

    pub(crate) unsafe fn exact(&self, object: &Root<'_, '_>, expected: Class) -> Result<(), GameError> {
        // SAFETY: The object is rooted and both class descriptors belong to this retained runtime.
        unsafe {
            if (self.object_class)(target(object)?.as_ptr()) != expected {
                return Err(unsupported("unexpected object type"));
            }
            Ok(())
        }
    }

    pub(crate) unsafe fn text(&self, attached: &AttachedThread<'_>, object: &Root<'_, '_>, name: &CStr) -> Result<String, GameError> {
        // SAFETY: The string object is rooted; class, length and character storage are checked before the bounded UTF-16 copy.
        unsafe {
            let class = self.system_class(c"String")?;
            let Some(text) = self.reference(attached, object, name, class)? else {
                return Ok(String::new());
            };
            self.owned_text(Some(&text), class, 4096)
        }
    }

    pub(crate) unsafe fn owned_text(&self, text: Option<&Root<'_, '_>>, class: Class, maximum: usize) -> Result<String, GameError> {
        // SAFETY: the caller retains the runtime and root; exact String class and bounded
        // public length are checked before copying its initialized UTF-16 buffer.
        unsafe {
            let Some(text) = text else {
                return Ok(String::new());
            };
            self.exact(text, class)?;
            let object = target(text)?;
            let length = usize::try_from((self.string_length)(object.as_ptr())).map_err(|_| unsupported("negative string length"))?;
            if length > maximum {
                return Err(unsupported("string length"));
            }
            let chars = (self.string_chars)(object.as_ptr());
            if chars.is_null() && length != 0 {
                return Err(unsupported("string characters"));
            }
            decode_text(&copy_values(chars, length), maximum)
        }
    }
    pub(crate) unsafe fn list_class(&self, class: Class) -> Result<(), GameError> {
        // SAFETY: class is retained metadata; all dependent exports are queried only
        // after its null check. This validates a closed framework List before field access.
        unsafe {
            if class.is_null() || (self.class_value)(class) || (self.class_image)(class).cast_const() != (self.corlib)() || (self.type_code)((self.class_type)(class)) != 0x15 {
                return Err(unsupported("closed List class"));
            }
            let name = (self.class_name)(class);
            let namespace = (self.class_namespace)(class);
            if name.is_null() || namespace.is_null() || metadata_text(name) != c"List`1" || metadata_text(namespace) != c"System.Collections.Generic" {
                return Err(unsupported("framework List identity"));
            }
            Ok(())
        }
    }
    pub(crate) unsafe fn enum_storage(&self, class: Class, primitive: &CStr, code: i32) -> Result<(), GameError> {
        // SAFETY: both classes belong to retained metadata; the enum's exact non-byref
        // underlying primitive is established before any field copy into a local slot.
        unsafe {
            if class.is_null() || !(self.class_enum)(class) {
                return Err(unsupported("enum class"));
            }
            let base = (self.enum_base)(class);
            if base.is_null() || (self.type_byref)(base) || (self.type_code)(base) != code || (self.type_class)(base) != self.system_class(primitive)? {
                return Err(unsupported("enum underlying storage"));
            }
            Ok(())
        }
    }

    pub(crate) unsafe fn array_class(&self, class: Class, element: Class) -> Result<(), GameError> {
        // SAFETY: The rooted array's runtime class and exact element class are checked through retained metadata exports.
        unsafe {
            if class.is_null() || (self.class_rank)(class) != 1 || (self.type_code)((self.class_type)(class)) != 0x1d || (self.class_element)(class) != element {
                return Err(unsupported("SZARRAY element/rank"));
            }
            Ok(())
        }
    }

    pub(crate) unsafe fn array_count(&self, array: &Root<'_, '_>, element: Class, maximum: u32) -> Result<u32, GameError> {
        // SAFETY: The rooted array's public length is cross-checked against the supported header before bounded element reads.
        unsafe {
            let object = target(array)?;
            self.array_class((self.object_class)(object.as_ptr()), element)?;
            if (self.array_header_size)() != 32 || (self.array_length_offset)() != 24 {
                return Err(unsupported("array header"));
            }
            let count = (self.array_length)(object.as_ptr());
            if count > maximum {
                return Err(unsupported("array length"));
            }
            Ok(count)
        }
    }
}

/// Field reads by resolved field, for objects read again and again in one call.
impl RuntimeAccess {
    /// `owner.name`, declared on `owner` itself (not inherited, not shadowed) with exactly the type `expected`, static
    /// or instance as `is_static` says, and with a real offset.
    ///
    /// # Safety
    /// `owner` and `expected` are class metadata of this live runtime.
    pub(crate) unsafe fn declared_field(&self, owner: Class, name: &CStr, expected: Class, is_static: bool) -> Result<Ptr, GameError> {
        // SAFETY: Owner metadata is live; storage flags and the exact class determine the output width.
        unsafe {
            let field = self.field(owner, name)?;
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.type_byref)(ty)
                || (self.type_class)(ty) != expected
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL) != if is_static { raw::FIELD_ATTRIBUTE_STATIC } else { 0 }
                || (self.field_offset)(field) == usize::MAX
            {
                return Err(unsupported(format!("declared field {} type/owner/storage", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    /// The object a static reference field holds, unrooted: root it before looking at it.
    ///
    /// # Safety
    /// `field` is a validated static reference field of this initialized runtime.
    pub(crate) unsafe fn static_reference(&self, field: Ptr) -> *mut raw::Object {
        // SAFETY: The validated static reference field writes one pointer-sized local slot before rooting.
        unsafe {
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(field, (&mut value as *mut *mut raw::Object).cast());
            value
        }
    }

    /// The object a reference field of `object` holds, unrooted: root it before looking at it.
    ///
    /// # Safety
    /// `field` is a validated reference field of `object`'s class.
    pub(crate) unsafe fn reference_at(&self, object: &Root<'_, '_>, field: Ptr) -> Result<*mut raw::Object, GameError> {
        // SAFETY: The rooted receiver and validated reference field remain live through the pointer copy.
        unsafe {
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut *mut raw::Object).cast());
            Ok(value)
        }
    }

    /// The raw bits of a `System.Single` field of `object`.
    ///
    /// # Safety
    /// `field` is a validated instance `System.Single` field of `object`'s class.
    pub(crate) unsafe fn single_bits(&self, object: &Root<'_, '_>, field: Ptr) -> Result<u32, GameError> {
        // SAFETY: The rooted receiver's validated Single field writes exactly one f32 local.
        unsafe {
            let mut value = 0_f32;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut f32).cast());
            Ok(value.to_bits())
        }
    }

    /// A Unity object's identity, its managed wrapper and native object, or `None` once the native object is gone.
    /// The native address is compared, never dereferenced.
    ///
    /// # Safety
    /// `object` is rooted; `object_class` is `UnityEngine.Object` and `cached` its validated `m_CachedPtr` field.
    pub(crate) unsafe fn alive(&self, object: &Root<'_, '_>, object_class: Class, cached: Ptr) -> Result<Option<ObservedObjectIdentity>, GameError> {
        // SAFETY: The rooted Unity wrapper has a metadata-validated IntPtr field; its native address is copied, never dereferenced.
        unsafe {
            let object = target(object)?;
            self.check_object(object_class, object)?;
            let mut native = 0_usize;
            (self.field_get)(object.as_ptr(), cached, (&mut native as *mut usize).cast());
            if native == 0 {
                return Ok(None);
            }
            ObservedObjectIdentity::new(object.as_ptr() as u64, native as u64)
                .map(Some)
                .map_err(|_| unsupported("nonzero object identity"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::test_fakes::{self, frees, roots, token};
    use il2cpp_runtime::ThreadContext;
    use std::cell::Cell;
    use std::ptr::NonNull;

    const PARENT: usize = 0x1000;
    const CHILD: usize = 0x2000;
    const PARENT_CLASS: usize = 0x3000;
    const CHILD_CLASS: usize = 0x4000;
    const BYTE_CLASS: usize = 0x5000;
    const WORD_CLASS: usize = 0x6000;
    const ENUM_CLASS: usize = 0xa000;
    const INT_CLASS: usize = 0xb000;
    const ENUM_BASE_TYPE: usize = 0xd000;
    thread_local! {
        static FIELD_CLASS: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static FIELD_READS: Cell<usize> = const { Cell::new(0) };
        static ASSIGNABLE: Cell<bool> = const { Cell::new(true) };
        static RANK: Cell<i32> = const { Cell::new(1) };
        static TYPE_CODE: Cell<i32> = const { Cell::new(0x1d) };
        static ELEMENT: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static ENUM_NAME: Cell<&'static CStr> = const { Cell::new(c"ClassRequirement") };
        static ENUM_TYPE_NAME: Cell<&'static CStr> = const { Cell::new(c"Fixture.Owner/ClassRequirement") };
        static IS_ENUM: Cell<bool> = const { Cell::new(true) };
        static ENUM_BASE_CODE: Cell<i32> = const { Cell::new(0x08) };
        static ENUM_BASE_CLASS: Cell<usize> = const { Cell::new(INT_CLASS) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static GLOBAL_ENUM_LOOKUPS: Cell<usize> = const { Cell::new(0) };
        static VALUE_CLASS: Cell<bool> = const { Cell::new(false) };
        static STORES: Cell<usize> = const { Cell::new(0) };
    }
    unsafe extern "C-unwind" fn corlib() -> *const std::ffi::c_void {
        token::<std::ffi::c_void>(0x7000).cast_const()
    }
    unsafe extern "C-unwind" fn class_from_name(_: *const std::ffi::c_void, namespace: *const std::ffi::c_char, name: *const std::ffi::c_char) -> Class {
        // SAFETY: production calls supply static null-terminated metadata names.
        unsafe {
            match (CStr::from_ptr(namespace), CStr::from_ptr(name)) {
                (namespace, name) if namespace == c"System" && name == c"Byte" => token(BYTE_CLASS),
                (namespace, name) if namespace == c"System" && name == c"Int32" => token(INT_CLASS),
                (namespace, name) if namespace == c"" && name == c"ClassRequirement" => {
                    GLOBAL_ENUM_LOOKUPS.set(GLOBAL_ENUM_LOOKUPS.get() + 1);
                    std::ptr::null_mut()
                }
                _ => panic!("unexpected fake class lookup"),
            }
        }
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        match object as usize {
            PARENT => token(PARENT_CLASS),
            CHILD => {
                // Any child metadata query must happen after its strong root was acquired.
                assert_eq!(roots(), 2);
                token(CHILD_CLASS)
            }
            _ => panic!("unexpected fake object"),
        }
    }
    unsafe extern "C-unwind" fn class_value(_: *const raw::Class) -> bool {
        VALUE_CLASS.get()
    }
    unsafe extern "C-unwind" fn class_field(class: Class, _: *const std::ffi::c_char) -> Ptr {
        assert_eq!(class as usize, PARENT_CLASS);
        token(0x8000)
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> crate::game::Type {
        token(0x9000)
    }
    unsafe extern "C-unwind" fn type_byref(_: crate::game::Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn field_flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn type_class(ty: crate::game::Type) -> Class {
        token(if ty as usize == ENUM_BASE_TYPE { ENUM_BASE_CLASS.get() } else { FIELD_CLASS.get() })
    }
    unsafe extern "C-unwind" fn field_get(object: *mut raw::Object, _: Ptr, destination: Ptr) {
        assert_eq!(object as usize, PARENT);
        FIELD_READS.set(FIELD_READS.get() + 1);
        match FIELD_CLASS.get() {
            CHILD_CLASS => {
                // SAFETY: the checked reference reader supplied one writable pointer-sized slot.
                unsafe {
                    destination.cast::<*mut raw::Object>().write(token(CHILD));
                }
            }
            ENUM_CLASS => {
                // SAFETY: the enum reader verified Int32 backing before supplying an i32 slot.
                unsafe {
                    destination.cast::<i32>().write(-123_456_789);
                }
            }
            _ => panic!("field_get called before validating destination width"),
        }
    }
    unsafe extern "C-unwind" fn assignable(expected: Class, actual: Class) -> bool {
        assert_eq!(expected, actual);
        assert!(matches!(expected as usize, CHILD_CLASS | PARENT_CLASS));
        ASSIGNABLE.get()
    }
    unsafe extern "C-unwind" fn field_parent(_: Ptr) -> Class {
        token(PARENT_CLASS)
    }
    unsafe extern "C-unwind" fn store_reference(object: *mut raw::Object, field: *mut raw::FieldInfo, value: Ptr) {
        assert_eq!(object as usize, PARENT);
        assert_eq!(field as usize, 0x8000);
        assert_eq!(value as usize, CHILD, "reference ABI requires the object, not a pointer slot");
        assert_eq!(roots(), 2);
        STORES.set(STORES.get() + 1);
    }
    unsafe extern "C-unwind" fn rank(_: *const raw::Class) -> i32 {
        RANK.get()
    }
    unsafe extern "C-unwind" fn class_type(_: Class) -> crate::game::Type {
        token(0x9000)
    }
    unsafe extern "C-unwind" fn type_code(ty: crate::game::Type) -> i32 {
        if ty as usize == ENUM_BASE_TYPE { ENUM_BASE_CODE.get() } else { TYPE_CODE.get() }
    }
    unsafe extern "C-unwind" fn class_element(_: Class) -> Class {
        token(ELEMENT.get())
    }
    unsafe extern "C-unwind" fn enum_name(class: Class) -> *const std::ffi::c_char {
        assert_eq!(class as usize, ENUM_CLASS);
        ENUM_NAME.get().as_ptr()
    }
    unsafe extern "C-unwind" fn is_enum(class: *const raw::Class) -> bool {
        assert_eq!(class as usize, ENUM_CLASS);
        IS_ENUM.get()
    }
    unsafe extern "C-unwind" fn enum_base(class: Class) -> crate::game::Type {
        assert_eq!(class as usize, ENUM_CLASS);
        token(ENUM_BASE_TYPE)
    }
    unsafe extern "C-unwind" fn type_name(_: crate::game::Type) -> *mut std::ffi::c_char {
        ENUM_TYPE_NAME.get().to_owned().into_raw()
    }
    unsafe extern "C-unwind" fn runtime_free(allocation: Ptr) {
        // SAFETY: the runtime-owned C string came from this fake type_name's CString::into_raw.
        unsafe {
            drop(std::ffi::CString::from_raw(allocation.cast()));
        }
    }

    fn fixture() -> (RuntimeAccess, ThreadContext) {
        let context = test_fakes::thread();
        FIELD_CLASS.set(CHILD_CLASS);
        FIELD_READS.set(0);
        ASSIGNABLE.set(true);
        RANK.set(1);
        TYPE_CODE.set(0x1d);
        ELEMENT.set(CHILD_CLASS);
        ENUM_NAME.set(c"ClassRequirement");
        ENUM_TYPE_NAME.set(c"Fixture.Owner/ClassRequirement");
        IS_ENUM.set(true);
        ENUM_BASE_CLASS.set(INT_CLASS);
        ENUM_BASE_CODE.set(0x08);
        FLAGS.set(0);
        BYREF.set(false);
        GLOBAL_ENUM_LOOKUPS.set(0);
        VALUE_CLASS.set(false);
        STORES.set(0);
        let mut access = RuntimeAccess::test_stubs();
        access.corlib = corlib;
        access.class_from_name = class_from_name;
        access.object_class = object_class;
        access.class_value = class_value;
        access.class_field = class_field;
        access.field_type = field_type;
        access.type_byref = type_byref;
        access.field_flags = field_flags;
        access.field_parent = field_parent;
        access.type_class = type_class;
        access.field_get = field_get;
        access.assignable = assignable;
        access.class_rank = rank;
        access.class_type = class_type;
        access.type_code = type_code;
        access.class_element = class_element;
        access.class_name = enum_name;
        access.class_enum = is_enum;
        access.enum_base = enum_base;
        access.type_name = type_name;
        access.runtime_free = runtime_free;
        (access, context)
    }

    #[test]
    fn reference_store_checks_types_mutability_owner_and_root_origin_before_calling_export() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored object identities and Box-backed GC handles; no game memory.
        unsafe {
            let parent = attached.root(NonNull::new(token(PARENT)).unwrap()).unwrap();
            let child = attached.root(NonNull::new(token(CHILD)).unwrap()).unwrap();
            let writes = raw::FieldWriteApi::test_export(store_reference);
            for flags in [0x10, 0x20, 0x40] {
                FLAGS.set(flags);
                assert!(access.set_reference(&writes, &attached, &parent, c"child", &child).is_err());
            }
            FLAGS.set(0);
            BYREF.set(true);
            assert!(access.set_reference(&writes, &attached, &parent, c"child", &child).is_err());
            BYREF.set(false);
            VALUE_CLASS.set(true);
            TYPE_CODE.set(0x11);
            assert!(access.set_reference(&writes, &attached, &parent, c"child", &child).is_err());
            VALUE_CLASS.set(false);
            assert!(access.set_reference(&writes, &attached, &parent, c"child", &child).is_err());
            TYPE_CODE.set(0x1d);
            FIELD_CLASS.set(WORD_CLASS);
            assert!(access.set_reference(&writes, &attached, &parent, c"child", &child).is_err());
            FIELD_CLASS.set(CHILD_CLASS);
            ASSIGNABLE.set(false);
            assert!(access.set_reference(&writes, &attached, &parent, c"child", &child).is_err());
            ASSIGNABLE.set(true);
            // A distinct context using the same fake exports is still a different
            // provenance boundary. Construct without resetting the active roots.
            let other_runtime = ThreadContext::new(test_fakes::api());
            let other_attached = other_runtime.attach().unwrap();
            assert!(access.set_reference(&writes, &other_attached, &parent, c"child", &child).is_err());
            assert_eq!(STORES.get(), 0);
            access.set_reference(&writes, &attached, &parent, c"child", &child).unwrap();
            assert_eq!(STORES.get(), 1);
        }
        assert_eq!(roots(), 0);
    }

    #[test]
    fn reference_roots_child_before_metadata_and_releases_every_handle() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: fake objects are immutable identity tokens recognized by the authored exports.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        // SAFETY: all fake runtime references belong to the same attached test thread; no graph races.
        let child = unsafe { access.reference(&attached, &parent, c"child", token(CHILD_CLASS)) }.unwrap().unwrap();
        assert_eq!(roots(), 2);
        drop(child);
        assert_eq!(roots(), 1);
        assert_eq!(frees(), 1);
        drop(parent);
        assert_eq!(roots(), 0);
        assert_eq!(frees(), 2);
    }
    #[test]
    fn reference_validation_error_releases_new_child_root() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored immutable fake objects and valid test-runtime handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        ASSIGNABLE.set(false);
        // SAFETY: the fake API returns a rooted object but deliberately rejects its class contract.
        assert!(unsafe { access.reference(&attached, &parent, c"child", token(CHILD_CLASS)) }.is_err());
        assert_eq!(roots(), 1);
        assert_eq!(frees(), 1);
        drop(parent);
        assert_eq!(roots(), 0);
    }
    #[test]
    fn failed_child_root_does_not_free_or_leak_the_parent() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored immutable fake objects and valid test-runtime handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        test_fakes::fail_root(CHILD);
        // SAFETY: the fake root export deliberately returns failure, before child metadata access.
        assert!(matches!(
            // SAFETY: the fake root export deliberately returns failure, before child metadata access.
            unsafe { access.reference(&attached, &parent, c"child", token(CHILD_CLASS)) },
            Err(il2cpp_runtime::MetadataError::Runtime(il2cpp_runtime::RuntimeError::RootFailed))
        ));
        assert_eq!(roots(), 1);
        assert_eq!(frees(), 0);
        drop(parent);
        assert_eq!(roots(), 0);
        assert_eq!(frees(), 1);
    }
    #[test]
    fn wider_scalar_metadata_is_rejected_before_writing_destination() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: authored immutable fake objects and valid test-runtime handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        FIELD_CLASS.set(WORD_CLASS);
        // SAFETY: metadata deliberately reports UInt16 where the reader requires exact Byte.
        assert!(unsafe { access.primitive::<u8>(&parent, c"value") }.is_err());
        assert_eq!(FIELD_READS.get(), 0);
        drop(parent);
        assert_eq!(roots(), 0);
    }
    #[test]
    fn enum_field_resolves_nested_or_namespaced_type_without_a_global_class() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: the authored runtime exposes this immutable fake object to valid handle APIs.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        FIELD_CLASS.set(ENUM_CLASS);
        // SAFETY: both arguments are valid static null-terminated strings for this fake runtime.
        assert!(unsafe { (access.class_from_name)(token(0x7000), c"".as_ptr(), c"ClassRequirement".as_ptr()) }.is_null());
        GLOBAL_ENUM_LOOKUPS.set(0);
        for name in [c"Fixture.Owner/ClassRequirement", c"Fixture.Owner+ClassRequirement", c"Fixture.Namespace.ClassRequirement"] {
            ENUM_TYPE_NAME.set(name);
            // SAFETY: the field carries a valid Int32 enum, even though no global class exists.
            assert_eq!(unsafe { access.enumeration(&parent, c"classReq", c"ClassRequirement") }.unwrap(), -123_456_789);
        }
        assert_eq!(FIELD_READS.get(), 3);
        assert_eq!(GLOBAL_ENUM_LOOKUPS.get(), 0);
        drop(parent);
        assert_eq!(roots(), 0);
    }
    #[test]
    fn enum_name_kind_base_and_storage_are_checked_before_destination_write() {
        for invalid in 0..6 {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            // SAFETY: authored immutable fake object and valid test-runtime handle APIs.
            let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
            FIELD_CLASS.set(ENUM_CLASS);
            match invalid {
                0 => ENUM_NAME.set(c"OtherEnum"),
                1 => IS_ENUM.set(false),
                2 => ENUM_BASE_CLASS.set(WORD_CLASS),
                3 => ENUM_BASE_CODE.set(0x05),
                4 => FLAGS.set(0x10),
                5 => BYREF.set(true),
                _ => unreachable!(),
            }
            // SAFETY: fake metadata deliberately violates one contract. None authorizes an i32 write.
            assert!(unsafe { access.enumeration(&parent, c"classReq", c"ClassRequirement") }.is_err());
            assert_eq!(FIELD_READS.get(), 0);
            assert_eq!(GLOBAL_ENUM_LOOKUPS.get(), 0);
            drop(parent);
            assert_eq!(roots(), 0);
        }
    }
    #[test]
    fn array_metadata_rejects_wrong_rank_array_kind_and_element() {
        let (access, _context) = fixture();
        // SAFETY: authored metadata exports accept these class identity tokens; no array data is read.
        unsafe {
            access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).unwrap();
            RANK.set(2);
            assert!(access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).is_err());
            RANK.set(1);
            TYPE_CODE.set(0x14);
            assert!(access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).is_err());
            TYPE_CODE.set(0x1d);
            ELEMENT.set(BYTE_CLASS);
            assert!(access.array_class(token(PARENT_CLASS), token(CHILD_CLASS)).is_err());
        }
    }
    #[test]
    fn strict_text_checks_both_encodings() {
        assert_eq!(decode_text(&[0x41, 0xd83d, 0xde00], 4096).unwrap(), "A😀");
        assert!(decode_text(&[0xd800], 4096).is_err());
        assert!(decode_text(&[0xdc00], 4096).is_err());
        assert!(decode_text(&vec![0x41; 4097], 4096).is_err());
        assert!(decode_text(&vec![0x4e00; 1366], 4096).is_err());
        assert!(decode_text(&vec![0x41; 4096], 4096).is_ok());
    }
}
