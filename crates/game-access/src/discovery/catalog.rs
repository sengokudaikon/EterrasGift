//! Bounded, rooted reads of game definition graphs (lists, arrays, nested classes, statics), shared by writes,
//! value mappings, unique and implicit descriptions. Singleton getters and formatter bodies are not invoked. The
//! catalog itself comes from the game's files (`eg-trainer`'s game data), never from these reads.

use super::metadata_text;
use super::{Class, DiscoveryError, Images, RuntimeAccess, forge::Root, target, unsupported};
use eg_il2cpp_runtime::{AttachedThread, metadata::Primitive, raw};
use std::{ffi::CStr, ptr::NonNull};

fn collection_count(count: i32, capacity: u32, maximum: u32) -> Result<usize, DiscoveryError> {
    // A retained List backing store may be larger than its active population.
    if count < 0 || count as u32 > maximum || count as u32 > capacity || capacity > 65536 {
        return Err(unsupported("catalog collection count/capacity"));
    }
    Ok(count as usize)
}

impl RuntimeAccess {
    // Every helper inherits the admitted main-thread, initialized metadata and exclusive graph
    // access contract of its catalog operation. All child references remain in rooted parents
    // until their own roots have been acquired; Unity native object lifetime is checked separately.
    pub(super) unsafe fn catalog_nested(&self, parent: Class, name: &CStr) -> Result<Class, DiscoveryError> {
        // SAFETY: The parent is retained class metadata; nested descriptors and names are inspected within a bounded enumeration.
        unsafe {
            if parent.is_null() {
                return Err(unsupported("catalog nested owner"));
            }
            let mut iterator = std::ptr::null_mut();
            let mut found = std::ptr::null_mut();
            for _ in 0..4096 {
                let class = (self.class_nested)(parent, &mut iterator);
                if class.is_null() {
                    return NonNull::new(found)
                        .map(NonNull::as_ptr)
                        .ok_or_else(|| unsupported(format!("catalog nested {}", name.to_string_lossy())));
                }
                let actual = (self.class_name)(class);
                if actual.is_null() {
                    return Err(unsupported("catalog nested name"));
                }
                if metadata_text(actual) == name {
                    if !found.is_null() && found != class {
                        return Err(unsupported("catalog ambiguous nested class"));
                    }
                    found = class;
                }
            }
            Err(unsupported("catalog nested enumeration limit"))
        }
    }

    pub(super) unsafe fn catalog_static<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        owner: Class,
        name: &CStr,
        expected: Class,
    ) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: The validated static reference field writes one pointer-sized local, which is rooted before inspection.
        unsafe {
            if expected.is_null() || (self.class_value)(expected) {
                return Err(unsupported("catalog static reference class"));
            }
            let field = self.field(owner, name)?;
            let flags = (self.field_flags)(field);
            if flags & raw::FIELD_ATTRIBUTE_STATIC == 0
                || flags & raw::FIELD_ATTRIBUTE_LITERAL != 0
                || (self.field_offset)(field) == usize::MAX
                || self.field_class(field, "catalog")? != expected
            {
                return Err(unsupported("catalog static reference storage/identity"));
            }
            let mut pointer = std::ptr::null_mut::<raw::Object>();
            // Static value reads do not initialize the owning class or load assets.
            (self.static_get)(field, (&mut pointer as *mut *mut raw::Object).cast());
            let root = NonNull::new(pointer).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = &root {
                self.check_object(expected, target(root)?)?;
            }
            Ok(root)
        }
    }

    pub(super) unsafe fn catalog_reference<'a, 'r>(&self, attached: &'a AttachedThread<'r>, parent: &Root<'_, '_>, name: &CStr) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: The receiver is rooted and the reference field has validated storage and expected target class.
        unsafe {
            let owner = (self.object_class)(target(parent)?.as_ptr());
            if owner.is_null() {
                return Err(unsupported("catalog reference owner"));
            }
            let field = self.field(owner, name)?;
            Ok(self.reference(attached, parent, name, self.field_class(field, "catalog")?)?)
        }
    }

    pub(super) unsafe fn catalog_array<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        array: &Root<'_, '_>,
        element: Class,
        maximum: u32,
    ) -> Result<Vec<Option<Root<'a, 'r>>>, DiscoveryError> {
        // SAFETY: The array is rooted; exact reference element type and bounded length precede every element pointer copy.
        unsafe {
            if element.is_null() || (self.class_value)(element) || maximum > 65536 {
                return Err(unsupported("catalog reference array element/bound"));
            }
            let count = self.array_count(array, element, maximum)?;
            let mut entries = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                let pointer = super::read_value(
                    target(array)?
                        .as_ptr()
                        .cast::<u8>()
                        .add(32 + index * std::mem::size_of::<usize>())
                        .cast::<*mut raw::Object>(),
                );
                let root = NonNull::new(pointer).map(|pointer| attached.root(pointer)).transpose()?;
                if let Some(root) = &root {
                    self.check_object(element, target(root)?)?;
                }
                entries.push(root);
            }
            Ok(entries)
        }
    }

    unsafe fn catalog_list_storage<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        list: &Root<'_, '_>,
        element: Class,
        maximum: u32,
    ) -> Result<(Root<'a, 'r>, usize), DiscoveryError> {
        // SAFETY: The list is rooted; its metadata-validated backing array and size are rooted and bounded before iteration.
        unsafe {
            if element.is_null() || maximum > 65536 {
                return Err(unsupported("catalog list element/bound"));
            }
            let class = (self.object_class)(target(list)?.as_ptr());
            self.list_class(class)?;
            let count: i32 = self.primitive(list, c"_size")?;
            let array_field = self.field(class, c"_items")?;
            let array_class = self.field_class(array_field, "catalog")?;
            self.array_class(array_class, element)?;
            let array = self
                .reference(attached, list, c"_items", array_class)?
                .ok_or_else(|| unsupported("catalog List storage is missing"))?;
            self.exact(&array, array_class)?;
            let capacity = self.array_count(&array, element, 65536)?;
            Ok((array, collection_count(count, capacity, maximum)?))
        }
    }

    pub(super) unsafe fn catalog_list<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        list: &Root<'_, '_>,
        element: Class,
        maximum: u32,
    ) -> Result<Vec<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: The validated list's rooted backing array remains live while bounded element references are copied and rooted.
        unsafe {
            if element.is_null() || (self.class_value)(element) {
                return Err(unsupported("catalog reference List element"));
            }
            let (array, count) = self.catalog_list_storage(attached, list, element, maximum)?;
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                let pointer = super::read_value(
                    target(&array)?
                        .as_ptr()
                        .cast::<u8>()
                        .add(32 + index * std::mem::size_of::<usize>())
                        .cast::<*mut raw::Object>(),
                );
                let root = attached.root(NonNull::new(pointer).ok_or_else(|| unsupported("catalog null List entry"))?)?;
                self.check_object(element, target(&root)?)?;
                values.push(root);
            }
            Ok(values)
        }
    }

    pub(super) unsafe fn catalog_values<T: Primitive>(&self, attached: &AttachedThread<'_>, list: &Root<'_, '_>, element: Class, maximum: u32) -> Result<Vec<T>, DiscoveryError> {
        // SAFETY: The rooted value array has exact primitive element width and a bounded count before reading its elements.
        unsafe {
            let expected = self.system_class(T::NAME)?;
            if element.is_null() {
                return Err(unsupported("catalog value List element"));
            }
            if element != expected {
                if !(self.class_enum)(element) {
                    return Err(unsupported("catalog value List storage"));
                }
                let base = (self.enum_base)(element);
                if base.is_null() || (self.type_byref)(base) || (self.type_class)(base) != expected {
                    return Err(unsupported("catalog enum List storage"));
                }
            }
            let (array, count) = self.catalog_list_storage(attached, list, element, maximum)?;
            Ok(super::copy_values(target(&array)?.as_ptr().cast::<u8>().add(32).cast::<T>(), count))
        }
    }

    // `_attached` is not read: holding it proves this thread is attached for the reads below.
    pub(super) unsafe fn catalog_live(&self, _attached: &AttachedThread<'_>, images: &Images, object: &Root<'_, '_>) -> Result<bool, DiscoveryError> {
        // SAFETY: The rooted Unity wrapper's validated native pointer is copied and compared without dereferencing the native address.
        unsafe {
            let unity_object = self.class(images, c"UnityEngine", c"Object")?;
            self.check_object(unity_object, target(object)?)?;
            let field = self.field(unity_object, c"m_CachedPtr")?;
            if self.field_class(field, "catalog")? != self.system_class(c"IntPtr")? || (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0 {
                return Err(unsupported("catalog Unity object native pointer"));
            }
            let mut value = 0_usize;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut usize).cast());
            Ok(value != 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ptr, Type};
    use super::*;
    use eg_il2cpp_runtime::ThreadContext;
    use std::cell::Cell;

    const CHILD: usize = 0x1000;
    const SECOND: usize = 0x2000;
    const CHILD_CLASS: usize = 0x3000;
    const ARRAY_CLASS: usize = 0x4000;
    const BYTE_CLASS: usize = 0x5000;
    const ENUM_CLASS: usize = 0x6000;
    const BASE_TYPE: usize = 0x7000;
    thread_local! {
        static ROOTS: Cell<usize> = const { Cell::new(0) };
        static FREES: Cell<usize> = const { Cell::new(0) };
        static ARRAY: Cell<usize> = const { Cell::new(0) };
        static FAIL_SECOND: Cell<bool> = const { Cell::new(false) };
        static RANK: Cell<i32> = const { Cell::new(1) };
        static ELEMENT: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static READS: Cell<usize> = const { Cell::new(0) };
        static FLAGS: Cell<i32> = const { Cell::new(0x10) };
        static FIELD_CLASS: Cell<usize> = const { Cell::new(CHILD_CLASS) };
        static IS_ENUM: Cell<bool> = const { Cell::new(true) };
        static BASE_CLASS: Cell<usize> = const { Cell::new(BYTE_CLASS) };
        static ENUM_BASE_CALLS: Cell<usize> = const { Cell::new(0) };
    }
    fn token<T>(value: usize) -> *mut T {
        std::ptr::without_provenance_mut(value)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(1)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(2)
    }
    unsafe extern "C-unwind" fn root_new(object: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        if object as usize == SECOND && FAIL_SECOND.get() {
            return std::ptr::null_mut();
        }
        ROOTS.set(ROOTS.get() + 1);
        Box::into_raw(Box::new(object)).cast()
    }
    unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
        // SAFETY: the live handle is a Box allocated by root_new.
        unsafe { handle.cast::<*mut raw::Object>().read() }
    }
    unsafe extern "C-unwind" fn root_free(handle: Ptr) {
        // SAFETY: StrongRoot frees this fake handle once after its final target access.
        unsafe {
            drop(Box::from_raw(handle.cast::<*mut raw::Object>()));
        }
        ROOTS.set(ROOTS.get() - 1);
        FREES.set(FREES.get() + 1);
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        if object as usize == ARRAY.get() {
            token(ARRAY_CLASS)
        } else {
            assert!(ROOTS.get() >= if ARRAY.get() == 0 { 1 } else { 2 }, "children must be rooted before metadata");
            token(CHILD_CLASS)
        }
    }
    unsafe extern "C-unwind" fn class_value(class: *const raw::Class) -> bool {
        matches!(class as usize, BYTE_CLASS | ENUM_CLASS)
    }
    unsafe extern "C-unwind" fn rank(_: *const raw::Class) -> i32 {
        RANK.get()
    }
    unsafe extern "C-unwind" fn class_type(_: Class) -> Type {
        token(0x8000)
    }
    unsafe extern "C-unwind" fn type_code(ty: Type) -> i32 {
        if ty as usize == BASE_TYPE { 0x05 } else { 0x1d }
    }
    unsafe extern "C-unwind" fn element(_: Class) -> Class {
        token(ELEMENT.get())
    }
    unsafe extern "C-unwind" fn header() -> u32 {
        32
    }
    unsafe extern "C-unwind" fn length_offset() -> u32 {
        24
    }
    unsafe extern "C-unwind" fn array_length(_: *mut raw::Object) -> u32 {
        3
    }
    unsafe extern "C-unwind" fn assignable(expected: Class, actual: Class) -> bool {
        expected == actual
    }
    unsafe extern "C-unwind" fn field(_: Class, _: *const std::ffi::c_char) -> Ptr {
        token(0x9000)
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> Type {
        token(0xa000)
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn offset(_: Ptr) -> usize {
        0
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        false
    }
    unsafe extern "C-unwind" fn type_class(ty: Type) -> Class {
        token(if ty as usize == BASE_TYPE { BASE_CLASS.get() } else { FIELD_CLASS.get() })
    }
    unsafe extern "C-unwind" fn static_get(_: Ptr, destination: Ptr) {
        READS.set(READS.get() + 1);
        // SAFETY: checked reference metadata supplies one writable pointer slot.
        unsafe {
            destination.cast::<*mut raw::Object>().write(token(CHILD));
        }
    }
    unsafe extern "C-unwind" fn field_get(_: *mut raw::Object, _: Ptr, destination: Ptr) {
        READS.set(READS.get() + 1);
        // SAFETY: the Byte enum reader verified the exact byte storage before this call.
        unsafe {
            destination.cast::<u8>().write(255);
        }
    }
    unsafe extern "C-unwind" fn name(_: Class) -> *const std::ffi::c_char {
        c"SP".as_ptr()
    }
    unsafe extern "C-unwind" fn class_enum(_: *const raw::Class) -> bool {
        IS_ENUM.get()
    }
    unsafe extern "C-unwind" fn enum_base(_: Class) -> Type {
        ENUM_BASE_CALLS.set(ENUM_BASE_CALLS.get() + 1);
        token(BASE_TYPE)
    }
    unsafe extern "C-unwind" fn corlib() -> Ptr {
        token(0xb000)
    }
    unsafe extern "C-unwind" fn class_from_name(_: Ptr, _: *const std::ffi::c_char, name: *const std::ffi::c_char) -> Class {
        // SAFETY: production supplies a static, nul-terminated primitive name.
        assert_eq!(unsafe { CStr::from_ptr(name) }, c"Byte");
        token(BYTE_CLASS)
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        ROOTS.set(0);
        FREES.set(0);
        ARRAY.set(0);
        FAIL_SECOND.set(false);
        RANK.set(1);
        ELEMENT.set(CHILD_CLASS);
        READS.set(0);
        FLAGS.set(0x10);
        FIELD_CLASS.set(CHILD_CLASS);
        IS_ENUM.set(true);
        BASE_CLASS.set(BYTE_CLASS);
        ENUM_BASE_CALLS.set(0);
        let mut access = RuntimeAccess::test_stubs();
        access.object_class = object_class;
        access.class_value = class_value;
        access.class_rank = rank;
        access.class_type = class_type;
        access.type_code = type_code;
        access.class_element = element;
        access.array_header_size = header;
        access.array_length_offset = length_offset;
        access.array_length = array_length;
        access.assignable = assignable;
        access.class_field = field;
        access.field_type = field_type;
        access.field_flags = flags;
        access.field_offset = offset;
        access.type_byref = byref;
        access.type_class = type_class;
        access.static_get = static_get;
        access.field_get = field_get;
        access.class_name = name;
        access.class_enum = class_enum;
        access.enum_base = enum_base;
        access.corlib = corlib;
        access.class_from_name = class_from_name;
        let ownership = raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: <raw::ThreadAttach as super::super::TestExportStub>::test_stub(),
            thread_detach: <raw::ThreadDetach as super::super::TestExportStub>::test_stub(),
            gchandle_new: root_new,
            gchandle_free: root_free,
            gchandle_get_target: root_target,
        };
        // SAFETY: authored functions implement this borrowed test thread and Box-backed handles.
        (access, unsafe { ThreadContext::new(ownership) })
    }

    #[test]
    fn retained_capacity_and_active_count_have_independent_bounds() {
        assert_eq!(collection_count(3, 4096, 8).unwrap(), 3);
        for (count, capacity, maximum) in [(-1, 4, 8), (9, 16, 8), (4, 3, 8), (0, 65537, 8)] {
            assert!(collection_count(count, capacity, maximum).is_err());
        }
    }

    #[test]
    fn static_metadata_refuses_literal_instance_and_wrong_width_before_copy() {
        for case in 0..3 {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            match case {
                0 => FLAGS.set(0),
                1 => FLAGS.set(0x50),
                _ => FIELD_CLASS.set(BYTE_CLASS),
            }
            // SAFETY: deliberately malformed authored metadata must fail before a static field read.
            assert!(unsafe { access.catalog_static(&attached, token(0xc000), c"storage", token(CHILD_CLASS)) }.is_err());
            assert_eq!(READS.get(), 0);
            assert_eq!(ROOTS.get(), 0);
        }
    }

    #[test]
    fn numeric_list_element_width_is_proved_before_any_storage_query() {
        for non_enum in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            // SAFETY: immutable fake identity is rooted through this test's Box-backed handle API.
            let list = unsafe { attached.root(NonNull::new(token(CHILD)).unwrap()) }.unwrap();
            IS_ENUM.set(!non_enum);
            BASE_CLASS.set(CHILD_CLASS);
            // SAFETY: deliberately wrong enum storage must fail before class/List/array metadata
            // or slot access. Unprovided exports panic if the reader reaches that later stage.
            assert!(unsafe { access.catalog_values::<u8>(&attached, &list, token(ENUM_CLASS), 8) }.is_err());
            assert_eq!(READS.get(), 0);
            assert_eq!(ENUM_BASE_CALLS.get(), usize::from(!non_enum));
            assert_eq!(ROOTS.get(), 1);
        }
    }

    #[test]
    fn nullable_array_entries_are_rooted_and_partial_failure_releases_children() {
        for fail in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            let mut storage = [0_usize, 0, 0, 3, CHILD, 0, SECOND];
            let pointer = NonNull::new(storage.as_mut_ptr().cast::<raw::Object>()).unwrap();
            ARRAY.set(pointer.as_ptr() as usize);
            // SAFETY: storage is a live immutable authored x64 reference array for this operation.
            let array = unsafe { attached.root(pointer) }.unwrap();
            FAIL_SECOND.set(fail);
            // SAFETY: exports validate this exact authored array layout and rooted identity graph.
            let result = unsafe { access.catalog_array(&attached, &array, token(CHILD_CLASS), 3) };
            if fail {
                assert!(result.is_err());
                assert_eq!(ROOTS.get(), 1);
                assert_eq!(FREES.get(), 1);
            } else {
                let entries = result.unwrap();
                assert_eq!(entries.len(), 3);
                assert!(entries[1].is_none());
                assert_eq!(ROOTS.get(), 3);
                drop(entries);
            }
            drop(array);
            assert_eq!(ROOTS.get(), 0);
        }
    }

    #[test]
    fn wrong_array_shape_is_rejected_before_touching_unavailable_storage() {
        for wrong_rank in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            ARRAY.set(0xd000);
            // SAFETY: the fake API recognizes this identity; its array bytes are deliberately unavailable.
            let array = unsafe { attached.root(NonNull::new(token(0xd000)).unwrap()) }.unwrap();
            if wrong_rank {
                RANK.set(2);
            } else {
                ELEMENT.set(BYTE_CLASS);
            }
            // SAFETY: invalid metadata must fail before attempting any raw data or header read.
            assert!(unsafe { access.catalog_array(&attached, &array, token(CHILD_CLASS), 3) }.is_err());
            assert_eq!(ROOTS.get(), 1);
        }
    }
}
