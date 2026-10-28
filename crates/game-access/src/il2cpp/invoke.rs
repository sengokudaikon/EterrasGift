//! Metadata-checked, deliberately bounded invocation. Metadata is not a proof of method-body safety.
use super::raw::{self, Class, InvocationApi, MethodInfo, Object, Type};
use super::{AttachedThread, RuntimeError, StrongRoot};
use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};

macro_rules! scalars {
    ($($variant:ident: $ty:ty = $code:literal),+ $(,)?) => {
        /// Exact IL2CPP primitive kinds. `Char` is a UTF-16 code unit, not a Rust `char`.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub enum Scalar { Boolean(bool), $($variant($ty)),+ }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum ScalarKind { Boolean, $($variant),+ }

        // Boolean storage is a byte: foreign writes can never manufacture an invalid Rust bool.
        enum ScalarStorage { Boolean(u8), $($variant($ty)),+ }

        impl Scalar {
            fn kind(self) -> ScalarKind {
                match self { Self::Boolean(_) => ScalarKind::Boolean, $(Self::$variant(_) => ScalarKind::$variant),+ }
            }
        }
        impl ScalarKind {
            fn from_code(code: i32) -> Option<Self> {
                match code { 0x02 => Some(Self::Boolean), $($code => Some(Self::$variant)),+, _ => None }
            }
            unsafe fn read_box(self, pointer: NonNull<c_void>) -> Scalar {
                // SAFETY: caller checks the exact boxed primitive class, whose payload has this
                // primitive width. Unaligned loads avoid assuming an object-header alignment.
                unsafe {
                    match self {
                        Self::Boolean => Scalar::Boolean(pointer.cast::<u8>().as_ptr().read_unaligned() != 0),
                        $(Self::$variant => Scalar::$variant(pointer.cast::<$ty>().as_ptr().read_unaligned())),+
                    }
                }
            }
        }
        impl ScalarStorage {
            fn new(value: Scalar) -> Self {
                match value { Scalar::Boolean(value) => Self::Boolean(u8::from(value)), $(Scalar::$variant(value) => Self::$variant(value)),+ }
            }
            fn pointer(&mut self) -> *mut c_void {
                match self { Self::Boolean(value) => std::ptr::from_mut(value).cast(), $(Self::$variant(value) => std::ptr::from_mut(value).cast()),+ }
            }
            fn value(&self) -> Scalar {
                match self { Self::Boolean(value) => Scalar::Boolean(*value != 0), $(Self::$variant(value) => Scalar::$variant(*value)),+ }
            }
        }
    };
}

scalars! {
    Char: u16 = 0x03,
    I8: i8 = 0x04, U8: u8 = 0x05, I16: i16 = 0x06, U16: u16 = 0x07,
    I32: i32 = 0x08, U32: u32 = 0x09, I64: i64 = 0x0a, U64: u64 = 0x0b,
    F32: f32 = 0x0c, F64: f64 = 0x0d, NativeInt: isize = 0x18, NativeUInt: usize = 0x19,
}

/// A scalar is passed by value; `ByRef` is initialized scalar storage for either `ref` or `out`.
/// Reference arguments are the object pointer itself, not a pointer to a temporary object slot.
/// Managed `ref`/`out` references are unsupported because their native storage would need GC tracing.
pub enum Argument<'value, 'attachment, 'runtime> {
    Scalar(Scalar),
    ByRef(&'value mut Scalar),
    Reference(Option<&'value StrongRoot<'attachment, 'runtime>>),
    /// Exact enum identity plus its signed Int32 storage. The class token is only compared with
    /// validated signature metadata; invoke never dereferences this argument-supplied pointer.
    EnumI32 {
        class: NonNull<Class>,
        value: i32,
    },
}

/// Explicit runtime identities accepted only by unsafe `bind_with_value_types`.
/// This grants no permission to copy struct payloads or pass struct/ref-enum arguments.
#[derive(Clone, Copy, Debug)]
pub enum ValueTypeContract {
    BoxedReturn { class: NonNull<Class>, size: i32, alignment: u32 },
    I32Enum { class: NonNull<Class> },
}

impl ValueTypeContract {
    fn class(self) -> NonNull<Class> {
        match self {
            Self::BoxedReturn { class, .. } | Self::I32Enum { class } => class,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePosition {
    Return,
    Parameter(usize),
}

/// Validation or runtime/result-handling failure. An error does not guarantee that the managed body
/// was skipped: rooting/return validation can fail after it ran. Never infer rollback or retry safety
/// from this error alone. The operation/session layer must preserve uncertain-write handling.
#[derive(Debug, PartialEq, Eq)]
pub enum InvokeError {
    Runtime(RuntimeError),
    InvalidMetadata,
    GenericMethod,
    AbstractMethod,
    GenericDeclaringType,
    InvalidValueTypeContract,
    InvalidClosedReference,
    TooManyParameters,
    UnsupportedType { position: SignaturePosition, type_code: i32, by_ref: bool },
    ArgumentCount { expected: usize, actual: usize },
    ArgumentType { index: usize },
    ReceiverRequired,
    UnexpectedReceiver,
    ContextMismatch,
    ObjectUnavailable,
    ObjectTypeMismatch,
    UnexpectedReturn,
    NullUnbox,
}

impl std::fmt::Display for InvokeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IL2CPP invocation rejected: {self:?}")
    }
}
impl std::error::Error for InvokeError {}
impl From<RuntimeError> for InvokeError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

/// Every non-null result remains rooted, including the box from which a primitive was decoded.
pub enum ReturnValue<'attachment, 'runtime> {
    Void,
    Reference(Option<StrongRoot<'attachment, 'runtime>>),
    Scalar {
        value: Scalar,
        root: StrongRoot<'attachment, 'runtime>,
    },
    /// An exact approved value-type box, retained without exposing or copying its payload.
    Boxed(StrongRoot<'attachment, 'runtime>),
    EnumI32 {
        value: i32,
        root: StrongRoot<'attachment, 'runtime>,
    },
}

/// Managed exceptions returned through `il2cpp_runtime_invoke` are data, retained by a strong root.
/// This does not catch arbitrary C++/SEH faults or promise that Rust `catch_unwind` can catch them.
pub enum InvocationOutcome<'attachment, 'runtime> {
    Returned(ReturnValue<'attachment, 'runtime>),
    Exception(StrongRoot<'attachment, 'runtime>),
}

#[derive(Clone, Copy)]
enum ValueKind {
    Void,
    Scalar(ScalarKind),
    Reference,
    Boxed,
    EnumI32,
}

#[derive(Clone, Copy)]
struct SignatureType {
    kind: ValueKind,
    class: Option<NonNull<Class>>,
    by_ref: bool,
}

/// Invocation permissions and runtime identity bound to one attachment's thread.
/// This context and its methods inherit the attachment's non-Send/non-Sync lifetime.
pub struct InvocationContext<'attachment, 'runtime> {
    attachment: &'attachment AttachedThread<'runtime>,
    api: InvocationApi,
}

impl<'attachment, 'runtime> InvocationContext<'attachment, 'runtime> {
    /// Bind this API table to the attachment's live runtime.
    ///
    /// # Safety
    /// All exports must belong to the attachment's runtime, obey the declared Unity 6000.4 ABI,
    /// and remain loaded and valid throughout this context. The runtime must use a nonmoving
    /// collector (the evaluated Boehm configuration); roots alone do not stabilize interior pointers
    /// with a moving collector. Metadata/classes must remain immutable and live. No concurrent
    /// shutdown, metadata replacement or external thread detach is permitted. Runtime invocation
    /// must follow the evaluated argument/boxing/exception conventions, including fresh primitive
    /// return boxes. Method execution permissions are a separate unsafe `bind` obligation.
    pub unsafe fn new(attachment: &'attachment AttachedThread<'runtime>, api: InvocationApi) -> Self {
        Self { attachment, api }
    }

    /// Check a directly invoked method's metadata. There is no implicit virtual/interface dispatch.
    /// To invoke an override, resolve it with `il2cpp_object_get_virtual_method`, then independently
    /// authorize and bind that returned method. This avoids silently authorizing an unknown body.
    ///
    /// # Safety
    /// `method` must be live immutable metadata from this runtime through the returned method's
    /// lifetime. Its body (including callees, class initializers and runtime callbacks) must be
    /// trusted for *every* value/receiver accepted by this checked signature (including null references,
    /// any scalar value, byref mutation, and aliased references), may not retain native
    /// argument pointers, and must not violate Rust memory/lifetime invariants. The caller must
    /// authorize execution on this OS thread for that entire lifetime, coordinate engine/object
    /// state and aliasing (including any native Unity object initialization/liveness prerequisites),
    /// and uphold required main-thread/external synchronization. Metadata does
    /// not prove any of these properties; managed code can contain unsafe operations or native calls.
    pub unsafe fn bind(&self, method: NonNull<MethodInfo>) -> Result<CheckedMethod<'_, 'attachment, 'runtime>, InvokeError> {
        // SAFETY: the caller supplies the same method/runtime permissions as bind_checked. Empty
        // contracts keep the default primitive/reference-only signature support.
        unsafe { self.bind_checked(method, &[], &[]) }
    }

    /// Authorize exact boxed value returns and/or exact Int32 enum signatures in addition to the
    /// default bounded signature set. All allowlist entries are validated, copied and reduced to
    /// owned signature facts; the checked method never borrows the caller's allowlist.
    ///
    /// # Safety
    /// Every obligation of `bind` applies, including trusted callees/class initializers, runtime
    /// identity/lifetime, main-thread permissions, native object life and aliasing. Each contract's
    /// class must be live immutable metadata from this same runtime. The caller must independently
    /// establish the expected size/alignment of each allowed boxed-return class for this artifact;
    /// names or matching size alone do not establish identity. Approved struct returns are opaque
    /// runtime-owned GC boxes, possibly containing managed references: no Rust payload layout,
    /// blittability, or raw byte-copy permission is inferred. Nullable/generic/inflated classes,
    /// arbitrary struct arguments, struct byrefs and enum byrefs remain unsupported.
    /// Authorizing an Int32 enum method authorizes *every* signed Int32 value, including undefined
    /// enum values and flag combinations. Bodies must not assume Rust enum validity or retain
    /// native argument pointers. Result-validation/rooting errors can follow an executed body.
    pub unsafe fn bind_with_value_types(&self, method: NonNull<MethodInfo>, contracts: &[ValueTypeContract]) -> Result<CheckedMethod<'_, 'attachment, 'runtime>, InvokeError> {
        let contracts = contracts.to_vec();
        // SAFETY: the caller establishes all metadata identities and method-body permissions.
        unsafe {
            self.validate_value_types(&contracts)?;
            self.bind_checked(method, &contracts, &[])
        }
    }

    /// Opt in to exact closed reference classes (for example List<ItemAffix>) as
    /// receivers, parameters or returns. Ordinary bind still rejects all generics.
    /// This permits inflated methods only on the explicitly listed declaring classes.
    /// Open generic classes, value types and byref references remain unsupported.
    ///
    /// # Safety
    /// All bind obligations apply. Every listed class must be immutable metadata
    /// from this runtime, with its concrete type arguments independently established.
    /// The exact method body, including an inflated body, must be authorized by the
    /// caller; membership in the allowlist grants no method-body safety guarantee.
    pub unsafe fn bind_with_closed_references(&self, method: NonNull<MethodInfo>, classes: &[NonNull<Class>]) -> Result<CheckedMethod<'_, 'attachment, 'runtime>, InvokeError> {
        // SAFETY: caller supplies live metadata identities from this context's runtime.
        unsafe {
            for class in classes {
                if (self.api.class_is_valuetype)(class.as_ptr()) || (self.api.class_is_generic)(class.as_ptr()) || !(self.api.class_is_inflated)(class.as_ptr()) {
                    return Err(InvokeError::InvalidClosedReference);
                }
            }
            self.bind_checked(method, &[], classes)
        }
    }

    unsafe fn validate_value_types(&self, contracts: &[ValueTypeContract]) -> Result<(), InvokeError> {
        let api = &self.api;
        // SAFETY: bind_with_value_types receives live class metadata from this context's runtime.
        unsafe {
            for (index, contract) in contracts.iter().copied().enumerate() {
                let class = contract.class();
                if contracts[..index].iter().any(|prior| prior.class() == class)
                    || !(api.class_is_valuetype)(class.as_ptr())
                    || (api.class_is_generic)(class.as_ptr())
                    || (api.class_is_inflated)(class.as_ptr())
                {
                    return Err(InvokeError::InvalidValueTypeContract);
                }
                match contract {
                    ValueTypeContract::BoxedReturn { size, alignment, .. } => {
                        if !(1..=65_536).contains(&size) || !alignment.is_power_of_two() || (api.class_is_enum)(class.as_ptr()) {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                        let mut actual_alignment = 0;
                        if (api.class_value_size)(class.as_ptr(), &mut actual_alignment) != size || actual_alignment != alignment {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                    }
                    ValueTypeContract::I32Enum { .. } => {
                        if !(api.class_is_enum)(class.as_ptr()) {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                        let underlying = (api.class_enum_basetype)(class.as_ptr());
                        if underlying.is_null() || (api.type_is_byref)(underlying) || (api.type_get_type)(underlying) != 0x08 {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                        let corlib = (api.get_corlib)();
                        if corlib.is_null() {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                        let int32 = NonNull::new((api.class_from_name)(corlib, c"System".as_ptr(), c"Int32".as_ptr())).ok_or(InvokeError::InvalidValueTypeContract)?;
                        if (api.class_from_type)(underlying) != int32.as_ptr() {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                        let mut underlying_alignment = 0;
                        let mut enum_alignment = 0;
                        if (api.class_value_size)(int32.as_ptr(), &mut underlying_alignment) != 4
                            || !underlying_alignment.is_power_of_two()
                            || (api.class_value_size)(class.as_ptr(), &mut enum_alignment) != 4
                            || enum_alignment != underlying_alignment
                        {
                            return Err(InvokeError::InvalidValueTypeContract);
                        }
                    }
                }
            }
            Ok(())
        }
    }

    unsafe fn bind_checked(
        &self,
        method: NonNull<MethodInfo>,
        contracts: &[ValueTypeContract],
        closed_references: &[NonNull<Class>],
    ) -> Result<CheckedMethod<'_, 'attachment, 'runtime>, InvokeError> {
        let api = &self.api;
        // SAFETY: caller provides live metadata from the runtime bound by new.
        unsafe {
            if (api.method_is_generic)(method.as_ptr()) {
                return Err(InvokeError::GenericMethod);
            }
            let class = NonNull::new((api.method_get_class)(method.as_ptr())).ok_or(InvokeError::InvalidMetadata)?;
            if (api.method_is_inflated)(method.as_ptr()) && !closed_references.contains(&class) {
                return Err(InvokeError::GenericMethod);
            }
            let mut implementation_flags = 0;
            if (api.method_get_flags)(method.as_ptr(), &raw mut implementation_flags) & raw::METHOD_ATTRIBUTE_ABSTRACT != 0 {
                return Err(InvokeError::AbstractMethod);
            }
            if (api.class_is_generic)(class.as_ptr()) || ((api.class_is_inflated)(class.as_ptr()) && !closed_references.contains(&class)) {
                return Err(InvokeError::GenericDeclaringType);
            }
            let count = (api.method_get_param_count)(method.as_ptr()) as usize;
            // A deliberate support bound, not a claim about the runtime's maximum signature size.
            if count > 64 {
                return Err(InvokeError::TooManyParameters);
            }
            let returns = self.signature_type((api.method_get_return_type)(method.as_ptr()), SignaturePosition::Return, contracts, closed_references)?;
            let parameters = (0..count)
                .map(|index| {
                    self.signature_type(
                        (api.method_get_param)(method.as_ptr(), index as u32),
                        SignaturePosition::Parameter(index),
                        contracts,
                        closed_references,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CheckedMethod {
                context: self,
                method,
                class,
                value_receiver: (api.class_is_valuetype)(class.as_ptr()),
                instance: (api.method_is_instance)(method.as_ptr()),
                parameters,
                returns,
            })
        }
    }

    unsafe fn signature_type(
        &self,
        metadata: *const Type,
        position: SignaturePosition,
        contracts: &[ValueTypeContract],
        closed_references: &[NonNull<Class>],
    ) -> Result<SignatureType, InvokeError> {
        if metadata.is_null() {
            return Err(InvokeError::InvalidMetadata);
        }
        let api = &self.api;
        // SAFETY: bind receives live method metadata; signature types belong to it.
        unsafe {
            let code = (api.type_get_type)(metadata);
            let by_ref = (api.type_is_byref)(metadata);
            let unsupported = || InvokeError::UnsupportedType {
                position,
                type_code: code,
                by_ref,
            };
            if by_ref && position == SignaturePosition::Return {
                return Err(unsupported());
            }
            let kind = if code == 0x01 && position == SignaturePosition::Return {
                ValueKind::Void
            } else if let Some(scalar) = ScalarKind::from_code(code) {
                ValueKind::Scalar(scalar)
            } else if !by_ref
                && (matches!(code, 0x0e | 0x12 | 0x14 | 0x1c | 0x1d) || (code == 0x15 && closed_references.iter().any(|class| class.as_ptr() == (api.class_from_type)(metadata))))
            {
                ValueKind::Reference
            } else if code == 0x11 && !by_ref && !contracts.is_empty() {
                let class = NonNull::new((api.class_from_type)(metadata)).ok_or(InvokeError::InvalidMetadata)?;
                match contracts.iter().find(|contract| contract.class() == class) {
                    Some(ValueTypeContract::BoxedReturn { .. }) if position == SignaturePosition::Return => ValueKind::Boxed,
                    Some(ValueTypeContract::I32Enum { .. }) => ValueKind::EnumI32,
                    _ => return Err(unsupported()),
                }
            } else {
                // Includes unapproved structs/enums, pointers, function pointers, generics, typed references,
                // ref reference arguments, and any future/unrecognized metadata code.
                return Err(unsupported());
            };
            let class = if matches!(kind, ValueKind::Void) {
                None
            } else {
                let class = NonNull::new((api.class_from_type)(metadata)).ok_or(InvokeError::InvalidMetadata)?;
                if (api.class_is_generic)(class.as_ptr()) || ((api.class_is_inflated)(class.as_ptr()) && !closed_references.contains(&class)) {
                    return Err(unsupported());
                }
                if matches!(kind, ValueKind::Reference) && (api.class_is_valuetype)(class.as_ptr()) {
                    return Err(unsupported());
                }
                if matches!(kind, ValueKind::Boxed | ValueKind::EnumI32) && !(api.class_is_valuetype)(class.as_ptr()) {
                    return Err(unsupported());
                }
                Some(class)
            };
            Ok(SignatureType { kind, class, by_ref })
        }
    }

    fn object(&self, root: &StrongRoot<'_, '_>) -> Result<NonNull<Object>, InvokeError> {
        if !root.belongs_to(self.attachment) {
            return Err(InvokeError::ContextMismatch);
        }
        root.target().ok_or(InvokeError::ObjectUnavailable)
    }

    fn check_object(&self, object: NonNull<Object>, expected: NonNull<Class>, exact: bool) -> Result<(), InvokeError> {
        // SAFETY: all callers obtain this live object through a checked root or newly rooted result.
        unsafe {
            let actual = NonNull::new((self.api.object_get_class)(object.as_ptr())).ok_or(InvokeError::InvalidMetadata)?;
            let valid = if exact {
                actual == expected
            } else {
                (self.api.class_is_assignable_from)(expected.as_ptr(), actual.as_ptr())
            };
            if !valid {
                return Err(InvokeError::ObjectTypeMismatch);
            }
        }
        Ok(())
    }
}

/// A caller-authorized method with a validated bounded signature. Calls use this exact MethodInfo.
pub struct CheckedMethod<'context, 'attachment, 'runtime> {
    context: &'context InvocationContext<'attachment, 'runtime>,
    method: NonNull<MethodInfo>,
    class: NonNull<Class>,
    value_receiver: bool,
    instance: bool,
    parameters: Vec<SignatureType>,
    returns: SignatureType,
}

impl<'attachment, 'runtime> CheckedMethod<'_, 'attachment, 'runtime> {
    /// Invoke after checking all receiver/argument types and provenance, before any managed body runs.
    /// Scalar byrefs are copied back even when the runtime returns a managed exception. Their values
    /// then reflect whatever writes the body completed before throwing. Boolean bytes normalize to
    /// `false` for zero and `true` otherwise. There is no implicit numeric conversion or virtual dispatch.
    /// An error during result rooting/validation can follow an executed body; it does not imply rollback.
    pub fn invoke(&self, receiver: Option<&StrongRoot<'_, '_>>, arguments: &mut [Argument<'_, '_, '_>]) -> Result<InvocationOutcome<'attachment, 'runtime>, InvokeError> {
        if arguments.len() != self.parameters.len() {
            return Err(InvokeError::ArgumentCount {
                expected: self.parameters.len(),
                actual: arguments.len(),
            });
        }
        let context = self.context;
        let object = match (self.instance, receiver) {
            (true, Some(root)) => {
                let object = context.object(root)?;
                context.check_object(object, self.class, self.value_receiver)?;
                Some(object)
            }
            (true, None) => return Err(InvokeError::ReceiverRequired),
            (false, Some(_)) => return Err(InvokeError::UnexpectedReceiver),
            (false, None) => None,
        };
        // Finish validation before producing any pointers for runtime invocation.
        for (index, (parameter, argument)) in self.parameters.iter().zip(arguments.iter()).enumerate() {
            let valid = match (parameter.kind, parameter.by_ref, argument) {
                (ValueKind::Scalar(kind), false, Argument::Scalar(value)) => value.kind() == kind,
                (ValueKind::Scalar(kind), true, Argument::ByRef(value)) => value.kind() == kind,
                (ValueKind::EnumI32, false, Argument::EnumI32 { class, .. }) => Some(*class) == parameter.class,
                (ValueKind::Reference, false, Argument::Reference(root)) => {
                    if let Some(root) = root {
                        context.check_object(context.object(root)?, parameter.class.ok_or(InvokeError::InvalidMetadata)?, false)?;
                    }
                    true
                }
                _ => false,
            };
            if !valid {
                return Err(InvokeError::ArgumentType { index });
            }
        }
        let mut storage: Vec<Option<ScalarStorage>> = arguments
            .iter()
            .map(|argument| match argument {
                Argument::Scalar(value) => Some(ScalarStorage::new(*value)),
                Argument::ByRef(value) => Some(ScalarStorage::new(**value)),
                Argument::Reference(_) => None,
                Argument::EnumI32 { value, .. } => Some(ScalarStorage::new(Scalar::I32(*value))),
            })
            .collect();
        let mut pointers = Vec::with_capacity(arguments.len());
        for (argument, storage) in arguments.iter().zip(storage.iter_mut()) {
            pointers.push(match argument {
                Argument::Reference(Some(root)) => context.object(root)?.as_ptr().cast(),
                Argument::Reference(None) => null_mut(),
                _ => storage.as_mut().ok_or(InvokeError::InvalidMetadata)?.pointer(),
            });
        }
        let receiver = if let Some(object) = object {
            if self.value_receiver {
                // SAFETY: exact value-type box checked above; retained root + nonmoving collector.
                NonNull::new(unsafe { (context.api.object_unbox)(object.as_ptr()) }).ok_or(InvokeError::NullUnbox)?.as_ptr()
            } else {
                object.as_ptr().cast()
            }
        } else {
            null_mut()
        };
        let mut exception = null_mut();
        // SAFETY: unsafe bind authorizes this body on this thread. Types, storage widths, byrefs,
        // receiver and root provenance were checked. Storage/roots remain live until it returns.
        let returned = unsafe {
            (context.api.runtime_invoke)(
                self.method.as_ptr(),
                receiver,
                if pointers.is_empty() { null_mut() } else { pointers.as_mut_ptr() },
                &raw mut exception,
            )
        };
        // Do not call metadata or allocate managed objects before rooting the returned object.
        let outcome = if let Some(exception) = NonNull::new(exception) {
            // SAFETY: runtime returned a live exception from this invocation on this attachment.
            unsafe { context.attachment.root(exception.cast()) }
                .map(InvocationOutcome::Exception)
                .map_err(InvokeError::from)
        } else {
            self.returned(returned).map(InvocationOutcome::Returned)
        };
        for (argument, storage) in arguments.iter_mut().zip(storage.iter()) {
            if let (Argument::ByRef(destination), Some(storage)) = (argument, storage) {
                **destination = storage.value();
            }
        }
        outcome
    }

    fn returned(&self, pointer: *mut Object) -> Result<ReturnValue<'attachment, 'runtime>, InvokeError> {
        let Some(pointer) = NonNull::new(pointer) else {
            return match self.returns.kind {
                ValueKind::Void => Ok(ReturnValue::Void),
                ValueKind::Reference => Ok(ReturnValue::Reference(None)),
                ValueKind::Scalar(_) | ValueKind::Boxed | ValueKind::EnumI32 => Err(InvokeError::UnexpectedReturn),
            };
        };
        // SAFETY: a successful runtime invocation returned a live result from this runtime. Root
        // immediately, before querying its metadata or decoding its fresh primitive box.
        let root = unsafe { self.context.attachment.root(pointer) }?;
        match self.returns.kind {
            ValueKind::Void => Err(InvokeError::UnexpectedReturn),
            ValueKind::Reference => {
                self.context.check_object(pointer, self.returns.class.ok_or(InvokeError::InvalidMetadata)?, false)?;
                Ok(ReturnValue::Reference(Some(root)))
            }
            ValueKind::Scalar(kind) => {
                self.context.check_object(pointer, self.returns.class.ok_or(InvokeError::InvalidMetadata)?, true)?;
                // SAFETY: exact primitive result box checked above, with nonmoving collector.
                let payload = NonNull::new(unsafe { (self.context.api.object_unbox)(pointer.as_ptr()) }).ok_or(InvokeError::NullUnbox)?;
                // SAFETY: the fresh box holds precisely this primitive kind and has no exposed
                // alias yet. Read the concrete primitive, never transmute arbitrary user types.
                let value = unsafe { kind.read_box(payload) };
                Ok(ReturnValue::Scalar { value, root })
            }
            ValueKind::Boxed => {
                self.context.check_object(pointer, self.returns.class.ok_or(InvokeError::InvalidMetadata)?, true)?;
                Ok(ReturnValue::Boxed(root))
            }
            ValueKind::EnumI32 => {
                self.context.check_object(pointer, self.returns.class.ok_or(InvokeError::InvalidMetadata)?, true)?;
                // SAFETY: bind_with_value_types checked this exact enum class has Int32 size and
                // backing identity. The newly returned exact enum box remains rooted/nonmoving.
                let payload = NonNull::new(unsafe { (self.context.api.object_unbox)(pointer.as_ptr()) }).ok_or(InvokeError::NullUnbox)?;
                // SAFETY: the approved exact enum payload contains one initialized signed Int32.
                let value = unsafe { payload.cast::<i32>().as_ptr().read_unaligned() };
                Ok(ReturnValue::EnumI32 { value, root })
            }
        }
    }
}
