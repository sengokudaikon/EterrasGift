//! Observe an already completed locale operation. Initialization/selection getters are never called.

use super::{Class, DiscoveryError, Ptr, RuntimeAccess, unsupported};
use crate::{
    il2cpp::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, StrongRoot, ValueTypeContract, raw},
    locale::GameLocale,
};
use std::{ffi::CStr, ptr::NonNull};

type Root<'a, 'r> = StrongRoot<'a, 'r>;

fn target(root: &Root<'_, '_>) -> Result<NonNull<raw::Object>, DiscoveryError> {
    root.target().ok_or_else(|| unsupported("locale root target"))
}

fn completed(handle_version: i32, operation_version: i32, status: i32, references: i32) -> Result<(), DiscoveryError> {
    if handle_version != operation_version || status != 1 || references <= 0 {
        Err(DiscoveryError::NotReady("selected locale operation is absent, stale or incomplete"))
    } else {
        Ok(())
    }
}

impl RuntimeAccess {
    // All internal helpers inherit read_game_locale's attached-runtime and exclusive graph contract.
    unsafe fn locale_system(&self, name: &CStr) -> Result<Class, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let image = (self.corlib)();
            if image.is_null() {
                return Err(unsupported("locale corlib"));
            }
            NonNull::new((self.class_from_name)(image, c"System".as_ptr(), name.as_ptr()))
                .map(NonNull::as_ptr)
                .ok_or_else(|| unsupported("locale core type"))
        }
    }

    unsafe fn locale_field_class(&self, field: Ptr) -> Result<Class, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let ty = (self.field_type)(field);
            if ty.is_null() || (self.type_byref)(ty) {
                return Err(unsupported("locale field type"));
            }
            NonNull::new((self.type_class)(ty)).map(NonNull::as_ptr).ok_or_else(|| unsupported("locale field class"))
        }
    }

    unsafe fn locale_field(&self, owner: Class, name: &CStr, expected: Class) -> Result<Ptr, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let field = self.field(owner, name)?;
            if (self.field_flags)(field) & 0x10 != 0 || self.locale_field_class(field)? != expected {
                return Err(unsupported(format!("locale field {} identity/storage", name.to_string_lossy())));
            }
            Ok(field)
        }
    }

    unsafe fn locale_named_class(&self, class: Class, namespace: &CStr, name: &CStr, value: bool) -> Result<(), DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            if class.is_null() {
                return Err(unsupported("locale named class"));
            }
            let actual_name = (self.class_name)(class);
            let actual_namespace = (self.class_namespace)(class);
            if actual_name.is_null()
                || actual_namespace.is_null()
                || CStr::from_ptr(actual_name) != name
                || CStr::from_ptr(actual_namespace) != namespace
                || (self.class_value)(class) != value
            {
                return Err(unsupported("locale named class identity"));
            }
            Ok(())
        }
    }

    unsafe fn locale_i32(&self, object: &Root<'_, '_>, owner: Class, name: &CStr) -> Result<i32, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            self.check_object(owner, target(object)?)?;
            let field = self.locale_field(owner, name, self.locale_system(c"Int32")?)?;
            let mut value = 0_i32;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut i32).cast());
            Ok(value)
        }
    }

    unsafe fn locale_ref<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        object: &Root<'_, '_>,
        owner: Class,
        name: &CStr,
        expected: Class,
    ) -> Result<Option<Root<'a, 'r>>, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            self.check_object(owner, target(object)?)?;
            if expected.is_null() || (self.class_value)(expected) {
                return Err(unsupported("locale reference class"));
            }
            let field = self.locale_field(owner, name, expected)?;
            let mut value = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(target(object)?.as_ptr(), field, (&mut value as *mut *mut raw::Object).cast());
            let root = NonNull::new(value).map(|pointer| attached.root(pointer)).transpose()?;
            if let Some(root) = &root {
                self.check_object(expected, target(root)?)?;
            }
            Ok(root)
        }
    }

    unsafe fn locale_no_cctor(&self, class: Class) -> Result<(), DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let mut iterator = std::ptr::null_mut();
            for _ in 0..100_000 {
                let method = (self.class_methods)(class, &mut iterator);
                if method.is_null() {
                    return Ok(());
                }
                let name = (self.method_name)(method);
                if name.is_null() || CStr::from_ptr(name) == c".cctor" {
                    return Err(unsupported("locale boxing class initializer"));
                }
            }
            Err(unsupported("locale boxing method enumeration limit"))
        }
    }

    unsafe fn locale_box<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        object: &Root<'_, '_>,
        owner: Class,
        name: &CStr,
        expected: Class,
    ) -> Result<Root<'a, 'r>, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            self.check_object(owner, target(object)?)?;
            let field = self.locale_field(owner, name, expected)?;
            if !(self.class_value)(expected) {
                return Err(unsupported("locale boxed field class"));
            }
            // Field::GetValueObject on an instance field boxes the value through Object::Box.
            // Object allocation runs Runtime::ClassInit; reject a managed .cctor before allocating.
            self.locale_no_cctor(expected)?;
            let boxed = NonNull::new((self.field_get_object)(field, target(object)?.as_ptr())).ok_or_else(|| unsupported("locale boxed field unavailable"))?;
            let root = attached.root(boxed)?;
            if (self.object_class)(target(&root)?.as_ptr()) != expected {
                return Err(unsupported("locale boxed field result class"));
            }
            Ok(root)
        }
    }

    unsafe fn locale_alive(&self, object: &Root<'_, '_>, owner: Class) -> Result<(), DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            self.check_object(owner, target(object)?)?;
            let field = self.locale_field(owner, c"m_CachedPtr", self.locale_system(c"IntPtr")?)?;
            let mut native = 0_usize;
            (self.field_get)(target(object)?.as_ptr(), field, (&mut native as *mut usize).cast());
            if native == 0 {
                return Err(DiscoveryError::NotReady("localization Unity object is destroyed"));
            }
            Ok(())
        }
    }

    unsafe fn locale_state_class(&self, class: Class) -> Result<(), DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            self.locale_named_class(class, c"UnityEngine.ResourceManagement.AsyncOperations", c"AsyncOperationStatus", true)?;
            if !(self.class_enum)(class) {
                return Err(unsupported("locale operation status enum"));
            }
            let ty = (self.enum_base)(class);
            if ty.is_null() || (self.type_byref)(ty) || (self.type_code)(ty) != 0x08 || (self.type_class)(ty) != self.locale_system(c"Int32")? {
                return Err(unsupported("locale operation status enum"));
            }
            Ok(())
        }
    }

    unsafe fn locale_text(&self, root: Option<&Root<'_, '_>>, string_class: Class) -> Result<String, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let Some(root) = root else {
                return Ok(String::new());
            };
            let object = target(root)?;
            if (self.object_class)(object.as_ptr()) != string_class {
                return Err(unsupported("locale string class"));
            }
            let length = (self.string_length)(object.as_ptr());
            if !(0..=4096).contains(&length) {
                return Err(unsupported("locale text length"));
            }
            let chars = (self.string_chars)(object.as_ptr());
            if chars.is_null() && length != 0 {
                return Err(unsupported("locale string characters"));
            }
            let mut units = Vec::with_capacity(length as usize);
            for index in 0..length as usize {
                units.push(chars.add(index).read());
            }
            let value = String::from_utf16(&units).map_err(|_| unsupported("locale UTF-16"))?;
            if value.len() > 4096 {
                return Err(unsupported("locale UTF-8 length"));
            }
            Ok(value)
        }
    }

    /// Observe the language of an already completed selected-locale operation and Application.version.
    /// This never invokes settings/selection/initialization getters or waits for asynchronous work.
    ///
    /// # Safety
    /// All `status` live-runtime/attachment/main-thread conditions apply. `api` must belong to the
    /// same runtime. Exclude concurrent graph/metadata mutation, pooled operation reuse, shutdown,
    /// and native Settings/Locale destruction until this entire copy completes. Parent roots retain
    /// unchanged strong references until each child is rooted. Instance-field boxing and getters
    /// must obey the evaluated nonmoving-GC semantics. The caller independently authorizes only
    /// this artifact's audited Locale.get_Identifier (16-byte/two-reference value), Identifier.get_Code
    /// (reference load), and Application.get_version (Unity engine version internal call), including
    /// its audited Application .cctor (CancellationTokenSource allocation/static store/write barrier),
    /// internal-call cache population, native-buffer/managed-string allocation and buffer cleanup,
    /// and runtime callbacks. This is read-only with respect to gameplay, not a zero-side-effect call.
    /// Metadata cannot establish those body permissions.
    /// Handle generic fields are boxed/read through public APIs; no Rust struct layout is copied.
    pub unsafe fn read_game_locale(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi) -> Result<GameLocale, DiscoveryError> {
        // SAFETY: inherits read_game_locale's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let status = self.status(attached)?;
            if status.online || status.manager == 0 {
                return Err(DiscoveryError::OfflineRequired);
            }
            let images = self.images()?;
            let settings_class = self.class(&images, c"UnityEngine.Localization.Settings", c"LocalizationSettings")?;
            let locale_class = self.class(&images, c"UnityEngine.Localization", c"Locale")?;
            let identifier_class = self.class(&images, c"UnityEngine.Localization", c"LocaleIdentifier")?;
            let settings_field = self.static_field(settings_class, c"s_Instance", "UnityEngine.Localization.Settings.LocalizationSettings")?;
            if self.locale_field_class(settings_field)? != settings_class {
                return Err(unsupported("locale settings static identity"));
            }
            let mut settings = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(settings_field, (&mut settings as *mut *mut raw::Object).cast());
            let settings = NonNull::new(settings).ok_or(DiscoveryError::NotReady("localization settings are not initialized"))?;
            let settings = attached.root(settings)?;
            self.locale_alive(&settings, settings_class)?;
            let handle_field = self.field(settings_class, c"m_SelectedLocaleAsync")?;
            let handle_class = self.locale_field_class(handle_field)?;
            self.locale_named_class(handle_class, c"UnityEngine.ResourceManagement.AsyncOperations", c"AsyncOperationHandle`1", true)?;
            if (self.type_code)((self.class_type)(handle_class)) != 0x15 {
                return Err(unsupported("locale closed operation handle"));
            }
            let mut alignment = 0;
            if (api.class_value_size)(handle_class, &mut alignment) != 24 || alignment != 8 {
                return Err(unsupported("locale operation handle layout"));
            }
            let handle = self.locale_box(attached, &settings, settings_class, c"m_SelectedLocaleAsync", handle_class)?;
            let operation_field = self.field(handle_class, c"m_InternalOp")?;
            let operation_class = self.locale_field_class(operation_field)?;
            self.locale_named_class(operation_class, c"UnityEngine.ResourceManagement.AsyncOperations", c"AsyncOperationBase`1", false)?;
            if (self.type_code)((self.class_type)(operation_class)) != 0x15 {
                return Err(unsupported("locale closed operation base"));
            }
            // This field's exact Locale identity proves the closed generic graph's result type.
            self.locale_field(operation_class, c"<Result>k__BackingField", locale_class)?;
            let operation = self
                .locale_ref(attached, &handle, handle_class, c"m_InternalOp", operation_class)?
                .ok_or(DiscoveryError::NotReady("selected locale handle is empty"))?;
            let handle_version = self.locale_i32(&handle, handle_class, c"m_Version")?;
            let operation_version = self.locale_i32(&operation, operation_class, c"m_Version")?;
            let references = self.locale_i32(&operation, operation_class, c"m_referenceCount")?;
            let state_field = self.field(operation_class, c"m_Status")?;
            let state_class = self.locale_field_class(state_field)?;
            self.locale_state_class(state_class)?;
            self.locale_field(operation_class, c"m_Status", state_class)?;
            let succeeded = self.field(state_class, c"Succeeded")?;
            if (self.field_flags)(succeeded) & 0x50 != 0x50 || self.locale_field_class(succeeded)? != state_class {
                return Err(unsupported("locale success enum literal"));
            }
            let mut success = 0_i32;
            (self.static_get)(succeeded, (&mut success as *mut i32).cast());
            if success != 1 {
                return Err(unsupported("locale success enum value"));
            }
            let mut state = 0_i32;
            (self.field_get)(target(&operation)?.as_ptr(), state_field, (&mut state as *mut i32).cast());
            completed(handle_version, operation_version, state, references)?;
            let locale = self
                .locale_ref(attached, &operation, operation_class, c"<Result>k__BackingField", locale_class)?
                .ok_or(DiscoveryError::NotReady("selected locale result is empty"))?;
            self.locale_alive(&locale, locale_class)?;
            // These are the only managed bodies authorized by this narrowly scoped operation.
            self.locale_no_cctor(identifier_class)?;
            let string_class = self.locale_system(c"String")?;
            let identifier_method = self.zero_arg_method(locale_class, c"get_Identifier", identifier_class, true, &api)?;
            let code_method = self.zero_arg_method(identifier_class, c"get_Code", string_class, true, &api)?;
            let application = self.class(&images, c"UnityEngine", c"Application")?;
            let version_method = self.zero_arg_method(application, c"get_version", string_class, false, &api)?;
            let context = InvocationContext::new(attached, api);
            let contract = ValueTypeContract::BoxedReturn {
                class: NonNull::new(identifier_class).ok_or_else(|| unsupported("locale identifier class"))?,
                size: 16,
                alignment: 8,
            };
            let identifier_method = context
                .bind_with_value_types(identifier_method, &[contract])
                .map_err(|error| unsupported(error.to_string()))?;
            let identifier = match identifier_method.invoke(Some(&locale), &mut []).map_err(|error| unsupported(error.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Boxed(root)) => root,
                _ => return Err(unsupported("locale identifier getter result/exception")),
            };
            let code_method = context.bind(code_method).map_err(|error| unsupported(error.to_string()))?;
            let language = match code_method.invoke(Some(&identifier), &mut []).map_err(|error| unsupported(error.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(root)) => self.locale_text(root.as_ref(), string_class)?,
                _ => return Err(unsupported("locale code getter result/exception")),
            };
            if language.trim().is_empty() {
                return Err(unsupported("game language is unavailable"));
            }
            let version_method = context.bind(version_method).map_err(|error| unsupported(error.to_string()))?;
            let game_version = match version_method.invoke(None, &mut []).map_err(|error| unsupported(error.to_string()))? {
                InvocationOutcome::Returned(ReturnValue::Reference(root)) => self.locale_text(root.as_ref(), string_class)?,
                _ => return Err(unsupported("game version getter result/exception")),
            };
            Ok(GameLocale { language, game_version })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Type;
    use super::*;
    use crate::il2cpp::ThreadContext;
    use std::cell::Cell;
    const PARENT: usize = 0x1000;
    const CHILD: usize = 0x2000;
    const OWNER: usize = 0x3000;
    const BOX_CLASS: usize = 0x4000;
    thread_local! {
        static ROOTS: Cell<usize> = const { Cell::new(0) };
        static FREES: Cell<usize> = const { Cell::new(0) };
        static BOXES: Cell<usize> = const { Cell::new(0) };
        static BYREF: Cell<bool> = const { Cell::new(false) };
        static FLAGS: Cell<i32> = const { Cell::new(0) };
        static FIELD_CLASS: Cell<usize> = const { Cell::new(BOX_CLASS) };
        static VALUE_CLASS: Cell<bool> = const { Cell::new(true) };
        static CCTOR: Cell<bool> = const { Cell::new(false) };
        static ROOT_FAILURE: Cell<bool> = const { Cell::new(false) };
        static WRONG_BOX: Cell<bool> = const { Cell::new(false) };
        static CHILD_METADATA: Cell<usize> = const { Cell::new(0) };
        static ENUM_BASE_CALLS: Cell<usize> = const { Cell::new(0) };
    }
    fn token<T>(address: usize) -> *mut T {
        std::ptr::without_provenance_mut(address)
    }
    unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
        token(0x10)
    }
    unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
        token(0x20)
    }
    unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
        panic!("borrowed fixture thread")
    }
    unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
        panic!("borrowed fixture thread")
    }
    unsafe extern "C-unwind" fn root_new(object: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        if object as usize == CHILD && ROOT_FAILURE.get() {
            return std::ptr::null_mut();
        }
        ROOTS.set(ROOTS.get() + 1);
        Box::into_raw(Box::new(object)).cast()
    }
    unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
        // SAFETY: StrongRoot supplies a live Box allocated by this fake root_new.
        unsafe { handle.cast::<*mut raw::Object>().read() }
    }
    unsafe extern "C-unwind" fn root_free(handle: Ptr) {
        // SAFETY: StrongRoot frees the Box-backed fake handle exactly once.
        unsafe {
            drop(Box::from_raw(handle.cast::<*mut raw::Object>()));
        }
        ROOTS.set(ROOTS.get() - 1);
        FREES.set(FREES.get() + 1);
    }
    unsafe extern "C-unwind" fn object_class(object: *mut raw::Object) -> Class {
        match object as usize {
            PARENT => token(OWNER),
            CHILD => {
                assert_eq!(ROOTS.get(), 2, "metadata must follow rooting");
                CHILD_METADATA.set(CHILD_METADATA.get() + 1);
                token(if WRONG_BOX.get() { 0x5000 } else { BOX_CLASS })
            }
            _ => panic!("unexpected locale fake object"),
        }
    }
    unsafe extern "C-unwind" fn assignable(expected: Class, actual: Class) -> bool {
        assert_eq!(expected as usize, OWNER);
        actual == expected
    }
    unsafe extern "C-unwind" fn field(owner: Class, _: *const std::ffi::c_char) -> Ptr {
        assert_eq!(owner as usize, OWNER);
        token(0x6000)
    }
    unsafe extern "C-unwind" fn field_type(_: Ptr) -> Type {
        token(0x7000)
    }
    unsafe extern "C-unwind" fn byref(_: Type) -> bool {
        BYREF.get()
    }
    unsafe extern "C-unwind" fn flags(_: Ptr) -> i32 {
        FLAGS.get()
    }
    unsafe extern "C-unwind" fn type_class(_: Type) -> Class {
        token(FIELD_CLASS.get())
    }
    unsafe extern "C-unwind" fn class_value(_: *const raw::Class) -> bool {
        VALUE_CLASS.get()
    }
    unsafe extern "C-unwind" fn methods(_: Class, _: *mut Ptr) -> *const raw::MethodInfo {
        if CCTOR.get() { token(0x8000) } else { std::ptr::null() }
    }
    unsafe extern "C-unwind" fn method_name(_: *const raw::MethodInfo) -> *const std::ffi::c_char {
        c".cctor".as_ptr()
    }
    unsafe extern "C-unwind" fn box_field(_: Ptr, object: *mut raw::Object) -> *mut raw::Object {
        assert_eq!(object as usize, PARENT);
        assert_eq!(ROOTS.get(), 1);
        BOXES.set(BOXES.get() + 1);
        token(CHILD)
    }
    unsafe extern "C-unwind" fn class_name(_: Class) -> *const std::ffi::c_char {
        c"AsyncOperationStatus".as_ptr()
    }
    unsafe extern "C-unwind" fn class_namespace(_: Class) -> *const std::ffi::c_char {
        c"UnityEngine.ResourceManagement.AsyncOperations".as_ptr()
    }
    unsafe extern "C-unwind" fn not_enum(_: *const raw::Class) -> bool {
        false
    }
    unsafe extern "C-unwind" fn enum_base(_: Class) -> Type {
        ENUM_BASE_CALLS.set(ENUM_BASE_CALLS.get() + 1);
        panic!("enum_base must never see non-enum class")
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        assert_eq!(ROOTS.get(), 0);
        FREES.set(0);
        BOXES.set(0);
        BYREF.set(false);
        FLAGS.set(0);
        FIELD_CLASS.set(BOX_CLASS);
        VALUE_CLASS.set(true);
        CCTOR.set(false);
        ROOT_FAILURE.set(false);
        WRONG_BOX.set(false);
        CHILD_METADATA.set(0);
        ENUM_BASE_CALLS.set(0);
        let mut access = RuntimeAccess::test_stubs();
        access.object_class = object_class;
        access.assignable = assignable;
        access.class_field = field;
        access.field_type = field_type;
        access.type_byref = byref;
        access.field_flags = flags;
        access.type_class = type_class;
        access.class_value = class_value;
        access.class_methods = methods;
        access.method_name = method_name;
        access.field_get_object = box_field;
        access.class_name = class_name;
        access.class_namespace = class_namespace;
        access.class_enum = not_enum;
        access.enum_base = enum_base;
        let api = raw::Api {
            domain_get: domain,
            thread_current: current,
            thread_attach: attach,
            thread_detach: detach,
            gchandle_new: root_new,
            gchandle_free: root_free,
            gchandle_get_target: root_target,
        };
        // SAFETY: authored metadata and Box-backed GC handles live on this one borrowed test thread.
        (access, unsafe { ThreadContext::new(api) })
    }

    #[test]
    fn field_box_is_rooted_before_metadata_and_its_handles_are_released() {
        let (access, context) = fixture();
        let attached = context.attach().unwrap();
        // SAFETY: the authored immutable fake graph recognizes this live parent token.
        let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
        // SAFETY: fake field metadata matches the expected immutable value class with no .cctor.
        let child = unsafe { access.locale_box(&attached, &parent, token(OWNER), c"handle", token(BOX_CLASS)) }.unwrap();
        assert_eq!(ROOTS.get(), 2);
        assert_eq!(BOXES.get(), 1);
        assert_eq!(CHILD_METADATA.get(), 1);
        drop(child);
        assert_eq!(ROOTS.get(), 1);
        drop(parent);
        assert_eq!(ROOTS.get(), 0);
        assert_eq!(FREES.get(), 2);
    }
    #[test]
    fn wrong_field_shape_or_class_initializer_refuses_before_boxing() {
        for invalid in 0..5 {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            // SAFETY: authored immutable parent token and valid handle implementation.
            let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
            match invalid {
                0 => BYREF.set(true),
                1 => FLAGS.set(0x10),
                2 => FIELD_CLASS.set(0x5000),
                3 => VALUE_CLASS.set(false),
                4 => CCTOR.set(true),
                _ => unreachable!(),
            }
            // SAFETY: invalid fake metadata must fail before invoking the public boxing export.
            assert!(unsafe { access.locale_box(&attached, &parent, token(OWNER), c"handle", token(BOX_CLASS)) }.is_err());
            assert_eq!(BOXES.get(), 0);
            assert_eq!(ROOTS.get(), 1);
            drop(parent);
            assert_eq!(ROOTS.get(), 0);
        }
    }
    #[test]
    fn failed_box_root_or_wrong_box_identity_never_leaks_handles() {
        for root_failure in [false, true] {
            let (access, context) = fixture();
            let attached = context.attach().unwrap();
            // SAFETY: authored immutable parent token and valid handle implementation.
            let parent = unsafe { attached.root(NonNull::new(token(PARENT)).unwrap()) }.unwrap();
            ROOT_FAILURE.set(root_failure);
            WRONG_BOX.set(!root_failure);
            // SAFETY: the fake export returns a live box whose root or identity is deliberately rejected.
            assert!(unsafe { access.locale_box(&attached, &parent, token(OWNER), c"handle", token(BOX_CLASS)) }.is_err());
            assert_eq!(BOXES.get(), 1);
            assert_eq!(ROOTS.get(), 1);
            assert_eq!(CHILD_METADATA.get(), usize::from(!root_failure));
            drop(parent);
            assert_eq!(ROOTS.get(), 0);
        }
    }
    #[test]
    fn a_struct_named_async_status_is_rejected_without_calling_enum_base() {
        let (access, _context) = fixture();
        // SAFETY: the fake class is a valid value-type metadata token, deliberately not an enum.
        assert!(unsafe { access.locale_state_class(token(BOX_CLASS)) }.is_err());
        assert_eq!(ENUM_BASE_CALLS.get(), 0);
    }
    #[test]
    fn only_successful_live_matching_operation_versions_are_ready() {
        completed(7, 7, 1, 1).unwrap();
        for (handle, operation, state, references) in [(7, 8, 1, 1), (7, 7, 0, 1), (7, 7, 2, 1), (7, 7, 1, 0), (7, 7, 1, -1)] {
            assert!(matches!(completed(handle, operation, state, references), Err(DiscoveryError::NotReady(_))));
        }
    }
}
