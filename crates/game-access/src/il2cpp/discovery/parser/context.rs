//! Operation-local numeric culture observation; roots and identity checks are not synchronization.
use super::RuntimeAccess;
use crate::il2cpp::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, StrongRoot, raw};
use std::{ffi::CStr, ptr::NonNull};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericFormatContextPart {
    Thread,
    Culture,
    Provider,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NumericFormatContextUnavailable {
    Contract(&'static str),
    Runtime(&'static str),
    GetterException(&'static str),
    Changed(NumericFormatContextPart),
}
impl std::fmt::Display for NumericFormatContextUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Numeric format context unavailable: {self:?}")
    }
}
impl std::error::Error for NumericFormatContextUnavailable {}
type Error = NumericFormatContextUnavailable;

/// Three rooted BCL observations for one synchronous admitted operation.
/// This carries no historical selection, synchronization or cross-request authority.
pub struct NumericFormatContext<'a, 'r> {
    thread: StrongRoot<'a, 'r>,
    culture: StrongRoot<'a, 'r>,
    provider: StrongRoot<'a, 'r>,
}
impl<'a, 'r> NumericFormatContext<'a, 'r> {
    /// Borrow the exact observed provider. Its use still requires the parser's unsafe contract.
    pub fn provider(&self) -> &StrongRoot<'a, 'r> {
        &self.provider
    }
}

struct Shape {
    thread: NonNull<raw::Class>,
    culture: NonNull<raw::Class>,
    provider: NonNull<raw::Class>,
    current_thread: NonNull<raw::MethodInfo>,
    current_culture: NonNull<raw::MethodInfo>,
    current_info: NonNull<raw::MethodInfo>,
    num_info: NonNull<raw::FieldInfo>,
}

fn root_target(root: &StrongRoot<'_, '_>) -> Result<NonNull<raw::Object>, Error> {
    root.target().ok_or(Error::Runtime("context root lost target"))
}

// Only called beneath the public capture/recheck runtime/body/exclusion contract.
unsafe fn exact_root(root: &StrongRoot<'_, '_>, api: raw::InvocationApi, expected: NonNull<raw::Class>) -> Result<(), Error> {
    let object = root_target(root)?;
    // SAFETY: root belongs to this live runtime; metadata is immutable and initialized.
    if unsafe { (api.object_get_class)(object.as_ptr()) } != expected.as_ptr() {
        return Err(Error::Contract("context result must have exact corlib runtime class"));
    }
    Ok(())
}

impl RuntimeAccess {
    unsafe fn numeric_context_class(&self, api: raw::InvocationApi, ns: &CStr, name: &CStr) -> Result<NonNull<raw::Class>, Error> {
        // SAFETY: caller retains initialized immutable exact runtime metadata.
        unsafe {
            let class = self.parser_class(api, ns, name).map_err(|_| Error::Contract("missing exact context corlib class"))?;
            if (api.class_is_valuetype)(class.as_ptr()) || (api.class_is_enum)(class.as_ptr()) || (api.class_is_generic)(class.as_ptr()) || (api.class_is_inflated)(class.as_ptr())
            {
                return Err(Error::Contract("context class must be a closed reference type"));
            }
            Ok(class)
        }
    }

    // Deliberately separate from the original parser's static-overload selector.
    unsafe fn numeric_context_getter(
        &self,
        api: raw::InvocationApi,
        owner: NonNull<raw::Class>,
        name: &CStr,
        result: NonNull<raw::Class>,
        instance: bool,
    ) -> Result<NonNull<raw::MethodInfo>, Error> {
        // SAFETY: bounded metadata iteration beneath retained runtime contract.
        unsafe {
            let mut iterator = std::ptr::null_mut();
            let mut found = None;
            for _ in 0..super::MAX_METHODS {
                let Some(method) = NonNull::new((self.class_methods)(owner.as_ptr(), &mut iterator).cast_mut()) else {
                    return found.ok_or(Error::Contract("missing exact context getter"));
                };
                let m = method.as_ptr();
                let actual = (self.method_name)(m);
                if actual.is_null() {
                    return Err(Error::Contract("null context method name"));
                }
                if CStr::from_ptr(actual) != name {
                    continue;
                }
                let mut implementation = 0;
                let flags = (api.method_get_flags)(m, &mut implementation);
                let ty = (api.method_get_return_type)(m);
                if (api.method_get_class)(m) != owner.as_ptr()
                    || (api.method_is_instance)(m) != instance
                    || (flags & 0x10 != 0) == instance
                    || flags & (0x40 | 0x400) != 0
                    || (api.method_is_generic)(m)
                    || (api.method_is_inflated)(m)
                    || (api.method_get_param_count)(m) != 0
                    || ty.is_null()
                    || (api.type_is_byref)(ty)
                    || (api.type_get_type)(ty) != 0x12
                    || (api.class_from_type)(ty) != result.as_ptr()
                {
                    return Err(Error::Contract("context getter shape changed"));
                }
                if found.replace(method).is_some() {
                    return Err(Error::Contract("ambiguous context getter"));
                }
            }
            Err(Error::Contract("context method enumeration limit"))
        }
    }

    unsafe fn numeric_context_shape(&self, api: raw::InvocationApi) -> Result<Shape, Error> {
        // SAFETY: same immutable metadata/runtime contract as callers.
        unsafe {
            let thread = self.numeric_context_class(api, c"System.Threading", c"Thread")?;
            let culture = self.numeric_context_class(api, c"System.Globalization", c"CultureInfo")?;
            let provider = self.numeric_context_class(api, c"System.Globalization", c"NumberFormatInfo")?;
            let current_thread = self.numeric_context_getter(api, thread, c"get_CurrentThread", thread, false)?;
            let current_culture = self.numeric_context_getter(api, thread, c"get_CurrentCulture", culture, true)?;
            let current_info = self.numeric_context_getter(api, provider, c"get_CurrentInfo", provider, false)?;
            let num_info =
                NonNull::new((self.class_field)(culture.as_ptr(), c"numInfo".as_ptr()).cast::<raw::FieldInfo>()).ok_or(Error::Contract("missing CultureInfo.numInfo field"))?;
            let field = num_info.as_ptr().cast();
            let ty = (self.field_type)(field);
            if (self.field_parent)(field) != culture.as_ptr()
                || (self.field_flags)(field) & 0x50 != 0
                || (self.field_offset)(field) == usize::MAX
                || ty.is_null()
                || (api.type_is_byref)(ty)
                || (api.type_get_type)(ty) != 0x12
                || (api.class_from_type)(ty) != provider.as_ptr()
            {
                return Err(Error::Contract("CultureInfo.numInfo declared reference shape"));
            }
            Ok(Shape {
                thread,
                culture,
                provider,
                current_thread,
                current_culture,
                current_info,
                num_info,
            })
        }
    }

    unsafe fn observe_numeric_context<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        previous: Option<&NumericFormatContext<'_, '_>>,
    ) -> Result<NumericFormatContext<'a, 'r>, Error> {
        // SAFETY: public entrypoints establish exact initialized runtime and exclusive context span.
        unsafe {
            let shape = self.numeric_context_shape(api)?;
            let calls = InvocationContext::new(attached, api);
            let thread_getter = calls.bind(shape.current_thread).map_err(|_| Error::Contract("checked Thread getter"))?;
            let culture_getter = calls.bind(shape.current_culture).map_err(|_| Error::Contract("checked Culture getter"))?;
            let info_getter = calls.bind(shape.current_info).map_err(|_| Error::Contract("checked CurrentInfo getter"))?;
            let returned = |outcome, name| match outcome {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => Ok(root),
                InvocationOutcome::Exception(_root) => Err(Error::GetterException(name)),
                _ => Err(Error::Contract("context getter nonnull reference return")),
            };
            let thread = returned(
                thread_getter.invoke(None, &mut []).map_err(|_| Error::Runtime("Thread getter invocation/root"))?,
                "CurrentThread",
            )?;
            exact_root(&thread, api, shape.thread)?;
            if previous.is_some_and(|old| old.thread.target() != thread.target()) {
                return Err(Error::Changed(NumericFormatContextPart::Thread));
            }
            let culture = returned(
                culture_getter
                    .invoke(Some(&thread), &mut [])
                    .map_err(|_| Error::Runtime("Culture getter invocation/root"))?,
                "CurrentCulture",
            )?;
            // Refuse derived CultureInfo before CurrentInfo can dispatch to GetFormat/NumberFormat.
            exact_root(&culture, api, shape.culture)?;
            if previous.is_some_and(|old| old.culture.target() != culture.target()) {
                return Err(Error::Changed(NumericFormatContextPart::Culture));
            }
            let provider = returned(info_getter.invoke(None, &mut []).map_err(|_| Error::Runtime("CurrentInfo invocation/root"))?, "CurrentInfo")?;
            exact_root(&provider, api, shape.provider)?;
            let mut copied = std::ptr::null_mut::<raw::Object>();
            (self.field_get)(
                root_target(&culture)?.as_ptr(),
                shape.num_info.as_ptr().cast(),
                (&mut copied as *mut *mut raw::Object).cast(),
            );
            let copied = NonNull::new(copied).ok_or(Error::Changed(NumericFormatContextPart::Provider))?;
            // Root copied reference before any object metadata or other managed call.
            let copied = attached.root(copied).map_err(|_| Error::Runtime("numInfo root creation failed"))?;
            exact_root(&copied, api, shape.provider)?;
            if root_target(&copied)? != root_target(&provider)? || previous.is_some_and(|old| old.provider.target() != provider.target()) {
                return Err(Error::Changed(NumericFormatContextPart::Provider));
            }
            Ok(NumericFormatContext { thread, culture, provider })
        }
    }

    /// Observe the exact current BCL Thread, CultureInfo and NumberFormatInfo under roots.
    /// No culture name, selected game locale or readonly flag is used to infer equivalence.
    ///
    /// # Safety
    /// All tables/metadata/attachment belong to the same initialized retained source-bound
    /// Unity6000.4 runtime with nonmoving GC. This is one synchronous admitted engine-thread
    /// operation: no yield, await, UI wait, reentry or shutdown through capture/format/parse/recheck.
    /// The caller independently owns game/offline/target/graph admission and explicitly excludes
    /// outside current/default/global culture, ExecutionContext/AsyncLocal and provider/settings
    /// mutation, including other threads, reflection/native actors and profiler/GC callbacks.
    /// Authorize exact Thread/CurrentCulture/CurrentInfo/GetFormat/NumberFormat BCL bodies and
    /// lazy framework current/default/UserDefault culture initialization, globalization/native
    /// lookups, locks, allocation, cache publication/barriers and GC/profiler effects. Capture may
    /// lazily create culture.numInfo before issuing the context; thereafter the complete provider
    /// settings/reference graph stays unchanged through the containing operation. These permitted
    /// framework effects are not gameplay mutation and are not side-effect-free getters.
    /// Exact CultureInfo bounds transitive virtual dispatch only under that nonmutation contract.
    /// A context root, identity match, Name or IsReadOnly cannot prove synchronization or detect
    /// same-object settings mutation or restore-to-same ABA. A context is not authority for a later
    /// request. Native faults/SEH/OOM recovery, rollback or retry safety are not provided.
    pub unsafe fn capture_numeric_format_context<'a, 'r>(&self, attached: &'a AttachedThread<'r>, api: raw::InvocationApi) -> Result<NumericFormatContext<'a, 'r>, Error> {
        // SAFETY: caller supplies the complete synchronous runtime/body/exclusion contract above.
        unsafe { self.observe_numeric_context(attached, api, None) }
    }

    /// Diagnose currently observed identity drift; success is not a stability or ABA proof.
    /// # Safety
    /// The entire capture_numeric_format_context contract continues through this call and the
    /// context's use. Retain the same initialized runtime, attachment and synchronous operation;
    /// establish exclusion before observation, not by relying on the result of this check.
    pub unsafe fn recheck_numeric_format_context(&self, attached: &AttachedThread<'_>, api: raw::InvocationApi, context: &NumericFormatContext<'_, '_>) -> Result<(), Error> {
        if [&context.thread, &context.culture, &context.provider].iter().any(|root| !root.belongs_to(attached)) {
            return Err(Error::Contract("context belongs to another attachment"));
        }
        // Reject lost roots before any new metadata, allocation or invocation.
        for root in [&context.thread, &context.culture, &context.provider] {
            root_target(root)?;
        }
        // SAFETY: caller supplies the complete synchronous runtime/body/exclusion contract.
        unsafe { self.observe_numeric_context(attached, api, Some(context)) }.map(|_| ())
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
