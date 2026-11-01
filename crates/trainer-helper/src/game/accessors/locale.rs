//! Observe an already completed locale operation. Initialization/selection getters are never called.

use crate::game::{Class, GameError, RuntimeAccess, target, unsupported};
use eg_trainer_wire::locale::GameLocale;
use il2cpp_runtime::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, StrongRoot, ValueTypeContract, raw};
use std::ptr::NonNull;

type Root<'a, 'r> = StrongRoot<'a, 'r>;

fn completed(handle_version: i32, operation_version: i32, status: i32, references: i32) -> Result<(), GameError> {
    if handle_version != operation_version || status != 1 || references <= 0 {
        Err(GameError::NotReady("selected locale operation is absent, stale or incomplete"))
    } else {
        Ok(())
    }
}

impl RuntimeAccess {
    // All internal helpers inherit read_game_locale's attached-runtime and exclusive graph contract.
    unsafe fn locale_alive(&self, object: &Root<'_, '_>, owner: Class) -> Result<(), GameError> {
        // SAFETY: The caller's contract is the runtime call's; the copied native address is never dereferenced.
        if !unsafe { self.unity_object_alive(object, owner, "locale") }? {
            return Err(GameError::NotReady("localization Unity object is destroyed"));
        }
        Ok(())
    }

    unsafe fn locale_state_class(&self, class: Class) -> Result<(), GameError> {
        // SAFETY: The operation class comes from retained metadata; the exact closed generic result type is checked before traversal.
        unsafe {
            self.check_class_named(class, c"UnityEngine.ResourceManagement.AsyncOperations", c"AsyncOperationStatus", true, "locale")?;
            self.enum_storage(class, c"Int32", raw::type_code::I4)?;
            Ok(())
        }
    }

    unsafe fn locale_text(&self, root: Option<&Root<'_, '_>>, string_class: Class) -> Result<String, GameError> {
        // SAFETY: The string object remains rooted through a bounded owned UTF-16 copy after exact class validation.
        Ok(unsafe { self.owned_text(root, string_class, 4096) }?)
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
    /// this artifact's Locale.get_Identifier (16-byte/two-reference value), Identifier.get_Code
    /// (reference load), and Application.get_version (Unity engine version internal call), including
    /// its Application .cctor (CancellationTokenSource allocation/static store/write barrier),
    /// internal-call cache population, native-buffer/managed-string allocation and buffer cleanup,
    /// and runtime callbacks. This is read-only with respect to gameplay, not a zero-side-effect call.
    /// Metadata cannot establish those body permissions.
    /// Handle generic fields are boxed/read through public APIs; no Rust struct layout is copied.
    pub unsafe fn read_game_locale(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi) -> Result<GameLocale, GameError> {
        // SAFETY: The caller supplies the immutable main-thread locale span; operation versions and readiness precede checked getter calls.
        unsafe {
            let status = self.status(attached)?;
            if status.online || status.manager == 0 {
                return Err(GameError::OfflineRequired);
            }
            let images = self.images()?;
            let settings_class = self.class(&images, c"UnityEngine.Localization.Settings", c"LocalizationSettings")?;
            let locale_class = self.class(&images, c"UnityEngine.Localization", c"Locale")?;
            let identifier_class = self.class(&images, c"UnityEngine.Localization", c"LocaleIdentifier")?;
            let settings_field = self.static_field(settings_class, c"s_Instance", "UnityEngine.Localization.Settings.LocalizationSettings")?;
            if self.field_class(settings_field, "locale")? != settings_class {
                return Err(unsupported("locale settings static identity"));
            }
            let mut settings = std::ptr::null_mut::<raw::Object>();
            (self.static_get)(settings_field, (&mut settings as *mut *mut raw::Object).cast());
            let settings = NonNull::new(settings).ok_or(GameError::NotReady("localization settings are not initialized"))?;
            let settings = attached.root(settings)?;
            self.locale_alive(&settings, settings_class)?;
            let handle_field = self.field(settings_class, c"m_SelectedLocaleAsync")?;
            let handle_class = self.field_class(handle_field, "locale")?;
            self.check_class_named(handle_class, c"UnityEngine.ResourceManagement.AsyncOperations", c"AsyncOperationHandle`1", true, "locale")?;
            if (self.type_code)((self.class_type)(handle_class)) != raw::type_code::GENERICINST {
                return Err(unsupported("locale closed operation handle"));
            }
            let mut alignment = 0;
            if (api.class_value_size)(handle_class, &mut alignment) != 24 || alignment != 8 {
                return Err(unsupported("locale operation handle layout"));
            }
            let handle = self.boxed_in(attached, &settings, settings_class, c"m_SelectedLocaleAsync", handle_class, "locale")?;
            let operation_field = self.field(handle_class, c"m_InternalOp")?;
            let operation_class = self.field_class(operation_field, "locale")?;
            self.check_class_named(operation_class, c"UnityEngine.ResourceManagement.AsyncOperations", c"AsyncOperationBase`1", false, "locale")?;
            if (self.type_code)((self.class_type)(operation_class)) != raw::type_code::GENERICINST {
                return Err(unsupported("locale closed operation base"));
            }
            // This field's exact Locale identity proves the closed generic graph's result type.
            self.instance_field_in(operation_class, c"<Result>k__BackingField", locale_class, "locale")?;
            let operation = self
                .reference_in(attached, &handle, handle_class, c"m_InternalOp", operation_class, "locale")?
                .ok_or(GameError::NotReady("selected locale handle is empty"))?;
            let handle_version = self.primitive_in::<i32>(&handle, handle_class, c"m_Version", "locale")?;
            let operation_version = self.primitive_in::<i32>(&operation, operation_class, c"m_Version", "locale")?;
            let references = self.primitive_in::<i32>(&operation, operation_class, c"m_referenceCount", "locale")?;
            let state_field = self.field(operation_class, c"m_Status")?;
            let state_class = self.field_class(state_field, "locale")?;
            self.locale_state_class(state_class)?;
            self.instance_field_in(operation_class, c"m_Status", state_class, "locale")?;
            let succeeded = self.field(state_class, c"Succeeded")?;
            if (self.field_flags)(succeeded) & (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL) != (raw::FIELD_ATTRIBUTE_STATIC | raw::FIELD_ATTRIBUTE_LITERAL)
                || self.field_class(succeeded, "locale")? != state_class
            {
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
                .reference_in(attached, &operation, operation_class, c"<Result>k__BackingField", locale_class, "locale")?
                .ok_or(GameError::NotReady("selected locale result is empty"))?;
            self.locale_alive(&locale, locale_class)?;
            // These are the only managed bodies authorized by this narrowly scoped operation.
            self.check_no_class_initializer(identifier_class, "locale")?;
            let string_class = self.system_class(c"String")?;
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
    use super::*;
    use crate::game::{Type, test_fakes::token};
    use std::cell::Cell;
    const CLASS: usize = 0x4000;
    thread_local! {
        static ENUM_BASE_CALLS: Cell<usize> = const { Cell::new(0) };
    }
    unsafe extern "C-unwind" fn class_value(_: *const raw::Class) -> bool {
        true
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

    #[test]
    fn a_struct_named_async_status_is_rejected_without_calling_enum_base() {
        let mut access = RuntimeAccess::test_stubs();
        access.class_value = class_value;
        access.class_name = class_name;
        access.class_namespace = class_namespace;
        access.class_enum = not_enum;
        access.enum_base = enum_base;
        // SAFETY: the fake class is a valid value-type metadata token, deliberately not an enum.
        assert!(unsafe { access.locale_state_class(token(CLASS)) }.is_err());
        assert_eq!(ENUM_BASE_CALLS.get(), 0);
    }
    #[test]
    fn only_successful_live_matching_operation_versions_are_ready() {
        completed(7, 7, 1, 1).unwrap();
        for (handle, operation, state, references) in [(7, 8, 1, 1), (7, 7, 0, 1), (7, 7, 2, 1), (7, 7, 1, 0), (7, 7, 1, -1)] {
            assert!(matches!(completed(handle, operation, state, references), Err(GameError::NotReady(_))));
        }
    }
}
