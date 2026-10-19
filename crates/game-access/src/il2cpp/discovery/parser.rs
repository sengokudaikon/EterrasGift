//! Parse displayed scalar using the exact initialized game BCL and captured culture.
use super::{Class, RuntimeAccess};
use crate::il2cpp::{Argument, AttachedThread, InvocationContext, InvocationOutcome, ReturnValue, Scalar, StrongRoot, ValueTypeContract, raw};
use std::{ffi::CStr, ptr::NonNull};
const MAX_UNITS: usize = 4096;
const MAX_BYTES: usize = MAX_UNITS * 4;
const MAX_METHODS: usize = 4096;
#[derive(Debug, PartialEq, Eq)]
pub enum DisplayScalarUnavailable {
    Input,
    Unit,
    Culture,
    Scalar,
    Contract(&'static str),
    Runtime(&'static str),
}
impl std::fmt::Display for DisplayScalarUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Display scalar unavailable: {self:?}")
    }
}
impl std::error::Error for DisplayScalarUnavailable {}
type Unavailable = DisplayScalarUnavailable;
fn scalar_text<'a>(language: &str, display: &'a str, unit: &str) -> Result<&'a str, Unavailable> {
    if [language, display, unit]
        .iter()
        .any(|s| s.len() > MAX_BYTES || s.contains('\0') || s.encode_utf16().count() > MAX_UNITS)
    {
        return Err(Unavailable::Input);
    }
    if unit.is_empty() {
        Ok(display)
    } else {
        display.strip_suffix(unit).ok_or(Unavailable::Unit)
    }
}
impl RuntimeAccess {
    // Internal helpers inherit parse_display_scalar's initialized-runtime/metadata contract.
    unsafe fn parser_class(&self, api: raw::InvocationApi, ns: &CStr, name: &CStr) -> Result<NonNull<raw::Class>, Unavailable> {
        unsafe {
            let image = (api.get_corlib)();
            if image.is_null() {
                return Err(Unavailable::Contract("missing corlib"));
            }
            let class = NonNull::new((api.class_from_name)(image, ns.as_ptr(), name.as_ptr())).ok_or(Unavailable::Contract("missing exact corlib class"))?;
            if (self.class_image)(class.as_ptr()).cast_const() != image {
                return Err(Unavailable::Contract("class outside exact corlib"));
            }
            Ok(class)
        }
    }
    // Exact static overload identity prevents selecting the same-arity Span overload.
    unsafe fn parser_method(
        &self,
        api: raw::InvocationApi,
        owner: NonNull<raw::Class>,
        name: &CStr,
        result: NonNull<raw::Class>,
        parameters: &[(NonNull<raw::Class>, bool)],
    ) -> Result<NonNull<raw::MethodInfo>, Unavailable> {
        unsafe {
            let mut iter = std::ptr::null_mut();
            let mut found = None;
            for _ in 0..MAX_METHODS {
                let Some(method) = NonNull::new((self.class_methods)(owner.as_ptr(), &mut iter).cast_mut()) else {
                    return found.ok_or(Unavailable::Contract("missing exact BCL overload"));
                };
                let m = method.as_ptr();
                let actual = (self.method_name)(m);
                if actual.is_null() {
                    return Err(Unavailable::Contract("null method name"));
                }
                if CStr::from_ptr(actual) != name || (api.method_get_param_count)(m) as usize != parameters.len() {
                    continue;
                }
                if (api.method_is_instance)(m) || (api.method_get_class)(m) != owner.as_ptr() || (api.method_is_generic)(m) || (api.method_is_inflated)(m) {
                    continue;
                }
                let ty = (api.method_get_return_type)(m);
                if ty.is_null() || (api.type_is_byref)(ty) || (api.class_from_type)(ty) != result.as_ptr() {
                    continue;
                }
                let mut matching = true;
                for (i, (class, byref)) in parameters.iter().enumerate() {
                    let ty = (api.method_get_param)(m, i as u32);
                    if ty.is_null() || (api.type_is_byref)(ty) != *byref || (api.class_from_type)(ty) != class.as_ptr() {
                        matching = false;
                        break;
                    }
                }
                if matching && found.replace(method).is_some() {
                    return Err(Unavailable::Contract("ambiguous exact BCL overload"));
                }
            }
            Err(Unavailable::Contract("BCL method enumeration limit"))
        }
    }
    unsafe fn parser_string<'a, 'r>(
        &self,
        attached: &'a AttachedThread<'r>,
        api: raw::InvocationApi,
        factory: raw::StringFactoryApi,
        string: Class,
        text: &str,
    ) -> Result<StrongRoot<'a, 'r>, Unavailable> {
        unsafe {
            let units: Vec<u16> = text.encode_utf16().collect();
            let object = NonNull::new((factory.string_new_utf16)(units.as_ptr(), units.len() as i32)).ok_or(Unavailable::Runtime("string allocation returned null"))?;
            // Root immediately before metadata or any other managed allocation/call.
            let root = attached.root(object).map_err(|_| Unavailable::Runtime("string root creation failed"))?;
            let target = root.target().ok_or(Unavailable::Runtime("string root lost target"))?;
            if (api.object_get_class)(target.as_ptr()) != string {
                return Err(Unavailable::Contract("allocator returned non-String object"));
            }
            Ok(root)
        }
    }
    /// Parse display scale, removing only an exact final unit. No normalization, Culture.Name
    /// equality requirement, host parsing or Rust culture fallback is added.
    ///
    /// # Safety
    /// All tables, metadata and attachment must belong to the same initialized retained Unity6000.4
    /// runtime with nonmoving GC. Caller authorizes the exact source-audited BCL body closure:
    /// class initializers, globalization/native-table lookup, culture/number-format cache writes,
    /// locks, allocations and profiler/GC callbacks. These change framework state, not gameplay.
    /// Honor engine-thread/synchronization conditions and exclude shutdown/reentrancy. A containing
    /// game operation owns offline/target/generation/graph admission; this BCL-only parser accesses
    /// no gameplay graph. The UTF16 allocator copies synchronously without retaining native slots.
    /// RuntimeInvoke obeys CheckedMethod's scalar-out contract, retaining no F64 pointer. Managed
    /// exceptions are rooted data; OOM, arbitrary native fault and SEH recovery are not promised.
    pub unsafe fn parse_display_scalar(
        &self,
        attached: &AttachedThread<'_>,
        api: raw::InvocationApi,
        factory: raw::StringFactoryApi,
        language: &str,
        display: &str,
        unit: &str,
    ) -> Result<f64, Unavailable> {
        let scalar = scalar_text(language, display, unit)?;
        unsafe {
            let string = self.parser_class(api, c"System", c"String")?;
            let culture = self.parser_class(api, c"System.Globalization", c"CultureInfo")?;
            let styles = self.parser_class(api, c"System.Globalization", c"NumberStyles")?;
            let number = self.parser_class(api, c"System", c"Double")?;
            let boolean = self.parser_class(api, c"System", c"Boolean")?;
            let provider = self.parser_class(api, c"System", c"IFormatProvider")?;
            let cm = self.parser_method(api, culture, c"GetCultureInfo", culture, &[(string, false)])?;
            let pm = self.parser_method(api, number, c"TryParse", boolean, &[(string, false), (styles, false), (provider, false), (number, true)])?;
            let calls = InvocationContext::new(attached, api);
            let cm = calls.bind(cm).map_err(|_| Unavailable::Contract("CultureInfo checked signature"))?;
            let pm = calls
                .bind_with_value_types(pm, &[ValueTypeContract::I32Enum { class: styles }])
                .map_err(|_| Unavailable::Contract("TryParse checked signature"))?;
            let language = self.parser_string(attached, api, factory, string.as_ptr(), language)?;
            let scalar = self.parser_string(attached, api, factory, string.as_ptr(), scalar)?;
            let culture = match cm
                .invoke(None, &mut [Argument::Reference(Some(&language))])
                .map_err(|_| Unavailable::Contract("CultureInfo invocation/result"))?
            {
                InvocationOutcome::Returned(ReturnValue::Reference(Some(root))) => root,
                InvocationOutcome::Exception(_root) => return Err(Unavailable::Culture),
                _ => return Err(Unavailable::Contract("CultureInfo nonnull reference return")),
            };
            let mut out = Scalar::F64(0.0);
            let result = pm
                .invoke(
                    None,
                    &mut [
                        Argument::Reference(Some(&scalar)),
                        Argument::EnumI32 { class: styles, value: 231 },
                        Argument::Reference(Some(&culture)),
                        Argument::ByRef(&mut out),
                    ],
                )
                .map_err(|_| Unavailable::Contract("TryParse invocation/result"))?;
            match result {
                InvocationOutcome::Returned(ReturnValue::Scalar {
                    value: Scalar::Boolean(true),
                    root: _root,
                }) => match out {
                    Scalar::F64(v) if v.is_finite() => Ok(v),
                    _ => Err(Unavailable::Scalar),
                },
                InvocationOutcome::Returned(ReturnValue::Scalar {
                    value: Scalar::Boolean(false),
                    root: _root,
                })
                | InvocationOutcome::Exception(_root) => Err(Unavailable::Scalar),
                _ => Err(Unavailable::Contract("TryParse Boolean return")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Ptr, TestExportStub};
    use super::*;
    use crate::il2cpp::ThreadContext;
    use std::{
        cell::{Cell, RefCell},
        ffi::{c_char, c_void},
    };
    const OWNER: usize = 0x1000;
    const RESULT: usize = 0x2000;
    const PARAM: usize = 0x3000;
    const METHOD: usize = 0x4000;
    const OTHER: usize = 0x4100;
    const TY: usize = 0x5000;
    const OBJECT: usize = 0x6000;
    const STRING: usize = 0x7000;
    thread_local! {
        static MODE:Cell<u8>=const{Cell::new(0)};
        static ALLOCATIONS:Cell<usize>=const{Cell::new(0)};
        static ROOTS:Cell<usize>=const{Cell::new(0)};
        static FREE:Cell<usize>=const{Cell::new(0)};
        static FAIL_ROOT:Cell<bool>=const{Cell::new(false)};
        static NULL_ALLOC:Cell<bool>=const{Cell::new(false)};
        static WRONG_CLASS:Cell<bool>=const{Cell::new(false)};
        static COPIED:RefCell<Vec<Vec<u16>>>=const{RefCell::new(Vec::new())};
    }
    fn ptr<T>(p: usize) -> *mut T {
        std::ptr::without_provenance_mut(p)
    }
    fn nn<T>(p: usize) -> NonNull<T> {
        NonNull::new(ptr(p)).unwrap()
    }
    unsafe extern "C-unwind" fn unexpected_invoke(_: *const raw::MethodInfo, _: *mut c_void, _: *mut *mut c_void, _: *mut *mut raw::Exception) -> *mut raw::Object {
        panic!("unexpected runtime invocation")
    }
    fn api() -> raw::InvocationApi {
        macro_rules! stub {
            ($t:ty) => {
                <$t as TestExportStub>::test_stub()
            };
        }
        raw::InvocationApi {
            runtime_invoke: unexpected_invoke,
            method_get_return_type: returns,
            method_get_class: owner,
            method_is_generic: generic,
            method_is_inflated: inflated,
            method_is_instance: instance,
            method_get_param_count: count,
            method_get_param: parameter,
            method_get_flags: stub!(raw::MethodGetFlags),
            type_get_type: stub!(raw::TypeGetType),
            type_is_byref: byref,
            class_from_type: class_type,
            class_is_valuetype: stub!(raw::ClassIsValueType),
            class_is_generic: stub!(raw::ClassIsGeneric),
            class_is_inflated: stub!(raw::ClassIsInflated),
            class_is_assignable_from: stub!(raw::ClassIsAssignableFrom),
            object_get_class: object_class,
            object_unbox: stub!(raw::ObjectUnbox),
            object_get_virtual_method: stub!(raw::ObjectGetVirtualMethod),
            get_corlib: stub!(raw::GetCorlib),
            class_from_name: stub!(raw::ClassFromName),
            class_is_enum: stub!(raw::ClassIsEnum),
            class_enum_basetype: stub!(raw::ClassEnumBasetype),
            class_value_size: stub!(raw::ClassValueSize),
        }
    }
    unsafe extern "C-unwind" fn methods(_: Class, iter: *mut Ptr) -> *const raw::MethodInfo {
        // SAFETY: parser_method supplies one live writable iterator slot.
        unsafe {
            let i = *iter as usize;
            *iter = ptr(i + 1);
            ptr(if MODE.get() == 10 {
                METHOD
            } else {
                match i {
                    0 => OTHER,
                    1 => METHOD,
                    2 if MODE.get() == 1 => METHOD + 1,
                    _ => 0,
                }
            })
        }
    }
    unsafe extern "C-unwind" fn name(_: *const raw::MethodInfo) -> *const c_char {
        c"TryParse".as_ptr()
    }
    unsafe extern "C-unwind" fn owner(_: *const raw::MethodInfo) -> Class {
        ptr(if MODE.get() == 2 { OWNER + 1 } else { OWNER })
    }
    unsafe extern "C-unwind" fn generic(_: *const raw::MethodInfo) -> bool {
        MODE.get() == 3
    }
    unsafe extern "C-unwind" fn inflated(_: *const raw::MethodInfo) -> bool {
        MODE.get() == 4
    }
    unsafe extern "C-unwind" fn instance(_: *const raw::MethodInfo) -> bool {
        MODE.get() == 5
    }
    unsafe extern "C-unwind" fn count(_: *const raw::MethodInfo) -> u32 {
        1
    }
    unsafe extern "C-unwind" fn returns(_: *const raw::MethodInfo) -> *const raw::Type {
        ptr(TY + 1)
    }
    unsafe extern "C-unwind" fn parameter(m: *const raw::MethodInfo, _: u32) -> *const raw::Type {
        ptr(if m as usize == OTHER { TY + 3 } else { TY + 2 })
    }
    unsafe extern "C-unwind" fn byref(t: *const raw::Type) -> bool {
        if t as usize == TY + 1 { MODE.get() == 6 } else { MODE.get() != 7 }
    }
    unsafe extern "C-unwind" fn class_type(t: *const raw::Type) -> Class {
        ptr(match t as usize {
            p if p == TY + 1 => {
                if MODE.get() == 8 {
                    RESULT + 1
                } else {
                    RESULT
                }
            }
            p if p == TY + 2 => {
                if MODE.get() == 9 {
                    PARAM + 1
                } else {
                    PARAM
                }
            }
            _ => PARAM + 2,
        })
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
    unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> Ptr {
        assert!(!pinned);
        if FAIL_ROOT.get() {
            return std::ptr::null_mut();
        }
        ROOTS.set(ROOTS.get() + 1);
        object.cast()
    }
    unsafe extern "C-unwind" fn target(handle: Ptr) -> *mut raw::Object {
        handle.cast()
    }
    unsafe extern "C-unwind" fn free(_: Ptr) {
        ROOTS.set(ROOTS.get() - 1);
        FREE.set(FREE.get() + 1);
    }
    unsafe extern "C-unwind" fn object_class(_: *mut raw::Object) -> Class {
        assert!(ROOTS.get() > 0, "must root before querying metadata");
        ptr(if WRONG_CLASS.get() { STRING + 1 } else { STRING })
    }
    unsafe extern "C-unwind" fn new_string(data: *const u16, len: i32) -> *mut raw::Object {
        assert!((0..=4096).contains(&len));
        ALLOCATIONS.set(ALLOCATIONS.get() + 1);
        // SAFETY: bounded exact UTF16 buffer is readable through this synchronous copy only.
        COPIED.with_borrow_mut(|v| v.push(unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec()));
        if NULL_ALLOC.get() { std::ptr::null_mut() } else { ptr(OBJECT + ALLOCATIONS.get() * 16) }
    }
    fn fixture() -> (RuntimeAccess, ThreadContext) {
        MODE.set(0);
        ALLOCATIONS.set(0);
        assert_eq!(ROOTS.get(), 0);
        FREE.set(0);
        FAIL_ROOT.set(false);
        NULL_ALLOC.set(false);
        WRONG_CLASS.set(false);
        COPIED.with_borrow_mut(Vec::clear);
        let mut a = RuntimeAccess::test_stubs();
        a.class_methods = methods;
        a.method_name = name;
        // SAFETY: authored APIs model this already-attached thread and fake opaque root identities.
        let ctx = unsafe {
            ThreadContext::new(raw::Api {
                domain_get: domain,
                thread_current: current,
                thread_attach: attach,
                thread_detach: detach,
                gchandle_new: root,
                gchandle_free: free,
                gchandle_get_target: target,
            })
        };
        (a, ctx)
    }
    #[test]
    fn bounded_nul_and_exact_suffix_refuse_before_any_export_is_reached() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        let factory = raw::StringFactoryApi { string_new_utf16: new_string };
        for (l, d, u, e) in [
            ("en\0", "42", "", Unavailable::Input),
            ("en", "42\0", "", Unavailable::Input),
            ("en", "42%", "%\0", Unavailable::Input),
            ("en", "42 % ", "%", Unavailable::Unit),
            ("en", "42", "%", Unavailable::Unit),
        ] {
            // SAFETY: malformed inputs must refuse before any unprovided metadata export.
            assert_eq!(unsafe { a.parse_display_scalar(&attached, api(), factory, l, d, u) }, Err(e));
        }
        assert_eq!(scalar_text("en", &"🌿".repeat(2049), ""), Err(Unavailable::Input));
        assert_eq!(scalar_text("en", &"1".repeat(4097), ""), Err(Unavailable::Input));
        assert_eq!(scalar_text("en", &"1".repeat(4096), "").unwrap().len(), 4096);
        assert_eq!(scalar_text("en-Ω-🌿", "12.5%", "%"), Ok("12.5"));
        assert_eq!(scalar_text("", " +42.5 ", ""), Ok(" +42.5 "));
        assert_eq!(ALLOCATIONS.get(), 0);
        assert_eq!(ROOTS.get(), 0);
    }
    #[test]
    fn wrong_owner_generic_inflated_instance_return_and_byref_never_select_an_overload() {
        let (a, _) = fixture();
        // SAFETY: these fake metadata identities are immutable tokens handled by the exact fake ABI.
        assert_eq!(
            unsafe { a.parser_method(api(), nn(OWNER), c"TryParse", nn(RESULT), &[(nn(PARAM), true)]) }.unwrap(),
            nn(METHOD)
        );
        for mode in 1..=10 {
            MODE.set(mode);
            // SAFETY: same fake metadata contract, with one deliberately incompatible signature fact.
            let e = unsafe { a.parser_method(api(), nn(OWNER), c"TryParse", nn(RESULT), &[(nn(PARAM), true)]) }.unwrap_err();
            assert_eq!(
                e,
                Unavailable::Contract(match mode {
                    1 => "ambiguous exact BCL overload",
                    10 => "ambiguous exact BCL overload",
                    _ => "missing exact BCL overload",
                })
            );
        }
        assert_eq!(ALLOCATIONS.get(), 0);
    }
    #[test]
    fn fresh_utf16_allocation_is_rooted_before_metadata_and_prior_roots_survive_failure() {
        let (a, ctx) = fixture();
        let attached = ctx.attach().unwrap();
        let f = raw::StringFactoryApi { string_new_utf16: new_string };
        // SAFETY: exact authored UTF16 allocator and live borrowed test attachment.
        let first = unsafe { a.parser_string(&attached, api(), f, ptr(STRING), "en-Ω-🌿") }.unwrap();
        assert_eq!(ROOTS.get(), 1);
        COPIED.with_borrow(|v| assert_eq!(String::from_utf16(&v[0]).unwrap(), "en-Ω-🌿"));
        FAIL_ROOT.set(true);
        // SAFETY: fake handle creation deliberately fails while the earlier language root remains held.
        assert!(matches!(
            unsafe { a.parser_string(&attached, api(), f, ptr(STRING), "12.5") },
            Err(Unavailable::Runtime("string root creation failed"))
        ));
        assert_eq!(ROOTS.get(), 1);
        assert_eq!(FREE.get(), 0);
        drop(first);
        assert_eq!(ROOTS.get(), 0);
        assert_eq!(FREE.get(), 1);
    }
    #[test]
    fn null_allocator_and_wrong_actual_string_class_are_typed_failures_without_leaks() {
        for null in [true, false] {
            let (a, ctx) = fixture();
            let t = ctx.attach().unwrap();
            NULL_ALLOC.set(null);
            WRONG_CLASS.set(!null);
            // SAFETY: authored allocator/classifier deliberately violates one return contract.
            let result = unsafe { a.parser_string(&t, api(), raw::StringFactoryApi { string_new_utf16: new_string }, ptr(STRING), "value") };
            assert!(matches!(result, Err(Unavailable::Runtime(_)) | Err(Unavailable::Contract(_))));
            assert_eq!(ROOTS.get(), 0);
            assert_eq!(FREE.get(), usize::from(!null));
        }
    }
}
