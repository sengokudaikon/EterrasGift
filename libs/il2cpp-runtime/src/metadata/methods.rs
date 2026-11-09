//! Finding a class's methods: one lookup ([`Exports::find_method`]) that takes the method a [`MethodQuery`] names
//! and checks it against the query's contract, and the walk to the nearest class that declares a method
//! ([`Exports::nearest_declaring`]). Every finder here and in callers is a query over these two. Nothing here
//! calls a managed method.

use super::{Class, Exports, MetadataError, Method, Type, metadata_text, unsupported};
use crate::raw;
use std::{ffi::CStr, ptr::NonNull};

/// How far up its parents a class is searched for a method; a deliberate bound.
pub const MAX_CLASS_DEPTH: usize = 256;

/// Which of a class's methods a lookup takes, and what the one it takes must be. Its name and parameters choose the
/// candidates; the rest is a contract the one chosen must meet, or the lookup fails.
#[derive(Clone, Copy)]
pub struct MethodQuery<'a> {
    pub name: &'a CStr,
    pub params: Params<'a>,
    /// What another method of that name, with other parameters, does to the lookup.
    pub others: Others,
    /// Whether it takes a receiver; `None` when either will do.
    pub instance: Option<bool>,
    pub returns: Returns<'a>,
    /// Method attributes it may not have (`raw::METHOD_ATTRIBUTE_*`).
    pub refused: u32,
    /// Whether a generic method fails it.
    pub refuse_generic: bool,
    /// Whether an inflated method fails it.
    pub refuse_inflated: bool,
    /// Whether the class searched must be the one that declares it.
    pub declared: bool,
}

impl<'a> MethodQuery<'a> {
    /// The method named `name` with `count` parameters, of any types: one with another count is passed over, and there
    /// is no contract.
    pub const fn named(name: &'a CStr, count: usize) -> Self {
        Self {
            name,
            params: Params::Count(count),
            others: Others::OtherCounts,
            instance: None,
            returns: Returns::Any,
            refused: 0,
            refuse_generic: false,
            refuse_inflated: false,
            declared: false,
        }
    }
}

/// The parameters a query's method has.
#[derive(Clone, Copy)]
pub enum Params<'a> {
    /// This many, of any types.
    Count(usize),
    /// These types, by name; a trailing `&` is by reference.
    Named(&'a [&'a str]),
    /// These classes, each by value.
    Classes(&'a [Class]),
}

impl Params<'_> {
    const fn count(&self) -> usize {
        match self {
            Self::Count(count) => *count,
            Self::Named(names) => names.len(),
            Self::Classes(classes) => classes.len(),
        }
    }
}

/// What another method of the query's name does when its parameters are not the query's.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Others {
    /// It fails the lookup.
    Fail,
    /// One with another count is passed over; one with the same count and other types fails the lookup.
    OtherCounts,
    /// It is passed over: an overload.
    Overloads,
}

/// The return type a query's method has.
#[derive(Clone, Copy)]
pub enum Returns<'a> {
    Any,
    /// A type by this name, by value.
    Named(&'a str),
    /// This class, by value.
    Class(Class),
    /// A type with this type code (`raw::type_code`), by value.
    Code(i32),
    /// A string, class or generic instance, by value.
    Reference,
}

/// Why a lookup failed, for the caller to say in its own words.
#[derive(Debug)]
pub enum MethodError {
    /// The class's methods could not be read.
    Metadata(MetadataError),
    /// One of the class's methods has no name.
    Unnamed,
    /// A candidate's parameter has no type.
    Untyped,
    /// No method is the one asked for.
    Missing,
    /// Two are.
    Twice,
    /// The one chosen breaks the query's contract, or [`Others::Fail`] met another of its name.
    Signature,
    /// The one chosen has the query's count but other parameter types.
    Parameters,
}

impl From<MetadataError> for MethodError {
    fn from(error: MetadataError) -> Self {
        Self::Metadata(error)
    }
}

impl Exports {
    /// The one method of `owner` that `query` names, checked against its contract. Only the exports the query needs
    /// are called.
    ///
    /// # Safety
    /// The lookup contract ([`crate::metadata`]).
    pub unsafe fn find_method(&self, owner: Class, query: &MethodQuery<'_>) -> Result<Method, MethodError> {
        // SAFETY: the owner and its method, parameter and return descriptors are retained immutable metadata; no method
        // is called.
        unsafe {
            let mut found = None;
            for method in self.methods(owner) {
                let method = method?;
                let name = (self.method_name)(method);
                if name.is_null() {
                    return Err(MethodError::Unnamed);
                }
                if metadata_text(name) != query.name {
                    continue;
                }
                if (self.method_params)(method) as usize != query.params.count() {
                    match query.others {
                        Others::Fail => return Err(MethodError::Signature),
                        Others::OtherCounts | Others::Overloads => continue,
                    }
                }
                // An overload is chosen by its parameters; otherwise they are part of the contract, checked last.
                if query.others == Others::Overloads && !self.params_match(method, &query.params)? {
                    continue;
                }
                if found.is_some() {
                    return Err(MethodError::Twice);
                }
                if !self.meets(owner, method, query)? {
                    return Err(MethodError::Signature);
                }
                if query.others != Others::Overloads && !self.params_match(method, &query.params)? {
                    return Err(MethodError::Parameters);
                }
                found = Some(method);
            }
            found.ok_or(MethodError::Missing)
        }
    }

    /// `class` or the nearest of its parents that declares a method named `name` with `count` parameters, up to
    /// [`MAX_CLASS_DEPTH`] classes up.
    ///
    /// # Safety
    /// The lookup contract ([`crate::metadata`]).
    pub unsafe fn nearest_declaring(&self, mut class: Class, name: &CStr, count: usize) -> Result<Class, MethodError> {
        // SAFETY: each class and its method descriptors are retained immutable metadata; the walk is bounded.
        unsafe {
            for _ in 0..MAX_CLASS_DEPTH {
                if class.is_null() {
                    break;
                }
                for method in self.methods(class) {
                    let method = method?;
                    let actual = (self.method_name)(method);
                    if actual.is_null() {
                        return Err(MethodError::Unnamed);
                    }
                    if metadata_text(actual) == name && (self.method_params)(method) as usize == count {
                        return Ok(class);
                    }
                }
                class = (self.class_parent)(class);
            }
            Err(MethodError::Missing)
        }
    }

    /// Whether `method` has `params`' types; a parameter without a type fails the lookup.
    unsafe fn params_match(&self, method: Method, params: &Params<'_>) -> Result<bool, MethodError> {
        // SAFETY: the caller's method descriptor is retained metadata with at least `params`' count of parameters.
        unsafe {
            let parameter = |index: usize| -> Result<Type, MethodError> {
                let ty = (self.method_param)(method, index as u32);
                if ty.is_null() { Err(MethodError::Untyped) } else { Ok(ty) }
            };
            match params {
                Params::Count(_) => Ok(true),
                Params::Named(names) => {
                    for (index, expected) in names.iter().enumerate() {
                        let ty = parameter(index)?;
                        if (self.type_byref)(ty) != expected.ends_with('&') || self.type_name(ty)? != *expected {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                Params::Classes(classes) => {
                    for (index, expected) in classes.iter().enumerate() {
                        let ty = parameter(index)?;
                        if (self.type_byref)(ty) || (self.type_class)(ty) != *expected {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
            }
        }
    }

    /// Whether `method`, found on `owner`, meets the rest of `query`'s contract.
    unsafe fn meets(&self, owner: Class, method: Method, query: &MethodQuery<'_>) -> Result<bool, MetadataError> {
        // SAFETY: the caller's method descriptor and its return type are retained metadata.
        unsafe {
            if query.instance.is_some_and(|instance| (self.method_instance)(method) != instance)
                || query.refuse_generic && (self.method_generic)(method)
                || query.refuse_inflated && (self.method_inflated)(method)
                || query.declared && (self.method_class)(method) != owner
            {
                return Ok(false);
            }
            if query.refused != 0 {
                let mut implementation = 0;
                if (self.method_flags)(method, &mut implementation) & query.refused != 0 {
                    return Ok(false);
                }
            }
            if let Returns::Any = query.returns {
                return Ok(true);
            }
            let returns = (self.method_return)(method);
            if returns.is_null() || (self.type_byref)(returns) {
                return Ok(false);
            }
            Ok(match query.returns {
                Returns::Any => true,
                Returns::Named(name) => self.type_name(returns)? == name,
                Returns::Class(class) => (self.type_class)(returns) == class,
                Returns::Code(code) => (self.type_code)(returns) == code,
                Returns::Reference => matches!((self.type_code)(returns), raw::type_code::STRING | raw::type_code::CLASS | raw::type_code::GENERICINST),
            })
        }
    }

    /// The one nonvirtual, nonabstract method `name` declared by `owner` that takes no argument, is or is not an
    /// instance method as `instance` says, and returns exactly `returns`. Another method of that name fails it.
    ///
    /// # Safety
    /// The lookup contract ([`crate::metadata`]).
    pub unsafe fn zero_arg_method(&self, owner: Class, name: &CStr, returns: Class, instance: bool) -> Result<NonNull<raw::MethodInfo>, MetadataError> {
        let query = MethodQuery {
            others: Others::Fail,
            instance: Some(instance),
            returns: Returns::Class(returns),
            refused: raw::METHOD_ATTRIBUTE_ABSTRACT | raw::METHOD_ATTRIBUTE_VIRTUAL,
            declared: true,
            ..MethodQuery::named(name, 0)
        };
        let name = name.to_string_lossy();
        // SAFETY: as this function's.
        let method = unsafe { self.find_method(owner, &query) }.map_err(|error| match error {
            MethodError::Metadata(error) => error,
            MethodError::Unnamed => unsupported("zero-argument getter name"),
            MethodError::Missing => unsupported(format!("zero-argument getter missing: {name}")),
            MethodError::Untyped | MethodError::Twice | MethodError::Signature | MethodError::Parameters => unsupported(format!("zero-argument getter contract: {name}")),
        })?;
        NonNull::new(method.cast_mut()).ok_or_else(|| unsupported(format!("zero-argument getter missing: {name}")))
    }

    /// The one direct (nonvirtual, nonabstract, non-P/Invoke, nongeneric) `System.Void` method `name` of `class` whose
    /// parameter types are named `parameters`. One of that name with another count is passed over.
    ///
    /// # Safety
    /// The lookup contract ([`crate::metadata`]).
    pub unsafe fn method(&self, class: Class, name: &CStr, parameters: &[&str], instance: bool) -> Result<Method, MetadataError> {
        let query = MethodQuery {
            params: Params::Named(parameters),
            instance: Some(instance),
            returns: Returns::Named("System.Void"),
            refused: raw::METHOD_ATTRIBUTE_PINVOKE_IMPL | raw::METHOD_ATTRIBUTE_ABSTRACT | raw::METHOD_ATTRIBUTE_VIRTUAL,
            refuse_generic: true,
            refuse_inflated: true,
            ..MethodQuery::named(name, parameters.len())
        };
        // SAFETY: as this function's.
        unsafe { self.find_method(class, &query) }.map_err(|error| {
            let name = name.to_string_lossy();
            match error {
                MethodError::Metadata(error) => error,
                MethodError::Unnamed => unsupported("null method name"),
                MethodError::Missing => unsupported(format!("missing method {name}")),
                MethodError::Untyped | MethodError::Parameters => unsupported(format!("method parameter {name}")),
                MethodError::Twice | MethodError::Signature => unsupported(format!("method contract {name}")),
            }
        })
    }
}

#[cfg(test)]
mod tests;
