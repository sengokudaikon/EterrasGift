//! The game's IL2CPP build read as data: `global-metadata.dat` for names, types and fields, `GameAssembly.dll` for the
//! types those fields have. Together they say what a class looks like, which is what [`crate::layout`] needs to rebuild
//! the serialized layout of objects stored without type trees.

mod binary;
mod metadata;

use crate::error::{Error, Result};
use binary::{Binary, RawType, kind};
pub(crate) use metadata::type_attributes;
use metadata::{FieldDefinition, Metadata, TypeDefinition};
use std::path::Path;

/// A field's type, resolved from the binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    /// `bool`, `char`, the integers, `float`, `double`, `IntPtr`, `UIntPtr`: the `Il2CppTypeEnum` kind.
    Primitive(u8),
    String,
    Object,
    Definition {
        index: u32,
    },
    Generic {
        definition: u32,
        arguments: Vec<Ty>,
    },
    /// A one-dimensional, zero-based array (`T[]`).
    Vector(Box<Ty>),
    /// A multi-dimensional array.
    Array(Box<Ty>, u8),
    /// A generic parameter of the enclosing type, by position.
    Parameter(u16),
    /// Anything a serialized field cannot be: pointers, method parameters, by-refs.
    Other(u8),
}

/// `FieldAttributes` bits used here.
pub(crate) mod field_attributes {
    pub(crate) const ACCESS: u16 = 0x7;
    pub(crate) const PUBLIC: u16 = 0x6;
    pub(crate) const STATIC: u16 = 0x10;
    pub(crate) const INIT_ONLY: u16 = 0x20;
    pub(crate) const LITERAL: u16 = 0x40;
    pub(crate) const NOT_SERIALIZED: u16 = 0x80;
}

pub(crate) struct Field<'a> {
    pub(crate) name: &'a str,
    pub(crate) ty: Ty,
    pub(crate) attributes: u16,
    token: u32,
}

pub(crate) struct Il2Cpp {
    metadata: Metadata,
    binary: Binary,
}

impl Il2Cpp {
    pub(crate) fn open(game_assembly: &Path, metadata: &Path) -> Result<Il2Cpp> {
        let metadata = Metadata::open(metadata).map_err(|e| e.context("global-metadata.dat"))?;
        let binary = Binary::open(game_assembly, metadata.types.len()).map_err(|e| e.context("GameAssembly.dll"))?;
        Ok(Il2Cpp { metadata, binary })
    }

    pub(crate) fn definition(&self, index: u32) -> Result<&TypeDefinition> {
        self.metadata.type_definition(index)
    }

    fn resolve(&self, raw: RawType) -> Result<Ty> {
        if raw.by_ref {
            return Ok(Ty::Other(kind::PTR));
        }
        Ok(match raw.kind {
            kind::BOOLEAN | kind::CHAR | kind::I1 | kind::U1 | kind::I2 | kind::U2 | kind::I4 | kind::U4 | kind::I8 | kind::U8 | kind::R4 | kind::R8 | kind::I | kind::U => {
                Ty::Primitive(raw.kind)
            }
            kind::STRING => Ty::String,
            kind::OBJECT => Ty::Object,
            kind::CLASS | kind::VALUE_TYPE => Ty::Definition {
                index: self.definition_index(raw)?,
            },
            kind::GENERIC_INSTANCE => {
                let (definition, arguments) = self.binary.generic_class(raw.data)?;
                Ty::Generic {
                    definition: self.definition_index(definition)?,
                    arguments: arguments.into_iter().map(|a| self.resolve(a)).collect::<Result<_>>()?,
                }
            }
            kind::SZ_ARRAY => Ty::Vector(Box::new(self.resolve(self.binary.type_from(raw.data)?)?)),
            kind::ARRAY => {
                let (element, rank) = self.binary.array(raw.data)?;
                Ty::Array(Box::new(self.resolve(element)?), rank)
            }
            kind::VAR => Ty::Parameter(
                self.metadata
                    .generic_position(u32::try_from(raw.data).map_err(|_| Error::malformed("generic parameter index out of range"))?)?,
            ),
            other => Ty::Other(other),
        })
    }

    fn definition_index(&self, raw: RawType) -> Result<u32> {
        match raw.kind {
            kind::CLASS | kind::VALUE_TYPE => {
                let index = u32::try_from(raw.data).map_err(|_| Error::malformed("type definition index out of range"))?;
                self.metadata.type_definition(index)?;
                Ok(index)
            }
            other => Err(Error::malformed(format!("expected a class or value type, found kind {other:#x}"))),
        }
    }

    /// The type a metadata type index names.
    pub(crate) fn type_at(&self, index: u32) -> Result<Ty> {
        self.resolve(self.binary.type_at(index)?)
    }

    /// `Namespace.Name`, with `Outer/Inner` for nested types, as Cecil names them.
    pub(crate) fn full_name(&self, index: u32) -> Result<String> {
        let definition = self.definition(index)?;
        if let Some(declaring) = definition.declaring_type {
            let Ty::Definition { index: outer } = self.type_at(declaring)? else {
                return Err(Error::malformed(format!("{} has a declaring type that is not a definition", definition.name)));
            };
            return Ok(format!("{}/{}", self.full_name(outer)?, definition.name));
        }
        Ok(if definition.namespace.is_empty() {
            definition.name.clone()
        } else {
            format!("{}.{}", definition.namespace, definition.name)
        })
    }

    /// A type definition's own fields, in declaration order, with their types.
    pub(crate) fn fields(&self, index: u32) -> Result<Vec<Field<'_>>> {
        let definition = self.definition(index)?;
        self.metadata
            .fields_of(definition)
            .iter()
            .map(|f: &FieldDefinition| {
                let raw = self.binary.type_at(f.type_index)?;
                Ok(Field {
                    name: &f.name,
                    ty: self.resolve(raw).map_err(|e| e.context(format!("{}.{}", definition.name, f.name)))?,
                    attributes: raw.attributes,
                    token: f.token,
                })
            })
            .collect()
    }

    /// The full names of the custom attributes on a type definition, or on one of its fields.
    pub(crate) fn attributes(&self, definition: u32, field: Option<&Field<'_>>) -> Result<Vec<String>> {
        let image = self
            .metadata
            .image_of(definition)
            .ok_or_else(|| Error::malformed(format!("type definition {definition} belongs to no image")))?;
        let token = field.map_or(self.definition(definition)?.token, |f| f.token);
        self.metadata.attribute_types(image, token)?.into_iter().map(|owner| self.full_name(owner)).collect()
    }

    /// The value of an `Int32` constant (`const int`) of `assembly`'s `namespace.class`, from its metadata default.
    pub(crate) fn int_constant(&self, assembly: &str, namespace: &str, class: &str, field: &str) -> Result<i32> {
        let found = self.find(assembly, namespace, class);
        let [index] = found.as_slice() else {
            return Err(Error::data(format!("{} definitions of {namespace}.{class} in {assembly}", found.len())));
        };
        let definition = self.definition(*index)?;
        let field_index = self
            .metadata
            .field_index(definition, field)
            .ok_or_else(|| Error::data(format!("{class} has no field {field}")))?;
        let raw = self.binary.type_at(self.metadata.fields[field_index as usize].type_index)?;
        if raw.kind != kind::I4 || raw.attributes & field_attributes::LITERAL == 0 || raw.attributes & field_attributes::STATIC == 0 {
            return Err(Error::data(format!("{class}.{field} is not a const int")));
        }
        let (type_index, value) = self.metadata.int_default(field_index)?;
        if self.binary.type_at(type_index)?.kind != kind::I4 {
            return Err(Error::data(format!("{class}.{field}'s default is not an Int32")));
        }
        Ok(value)
    }

    /// The type definitions named `namespace.name` (not nested) in the image `assembly`.dll.
    pub(crate) fn find(&self, assembly: &str, namespace: &str, name: &str) -> Vec<u32> {
        // MonoScripts name most assemblies bare (`LE`), some with their extension (`DOTweenPro.dll`).
        let image_name = if assembly.ends_with(".dll") { assembly.to_owned() } else { format!("{assembly}.dll") };
        let Some(image) = self.metadata.images.iter().find(|i| i.name == image_name) else {
            return Vec::new();
        };
        (image.first_type..image.first_type + image.type_count)
            .filter(|&i| {
                self.metadata
                    .types
                    .get(i as usize)
                    .is_some_and(|t| t.declaring_type.is_none() && t.namespace == namespace && t.name == name)
            })
            .collect()
    }
}
